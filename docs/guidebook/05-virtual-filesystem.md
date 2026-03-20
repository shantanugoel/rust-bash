# Chapter 5: Virtual Filesystem

## Overview

The VFS is the core sandboxing mechanism. Every file operation in rust-bash goes through the `VirtualFs` trait. No command, no interpreter path, and no redirect handler ever calls `std::fs` directly. This is the fundamental guarantee that makes rust-bash a sandbox.

## The VirtualFs Trait

```rust
pub trait VirtualFs: Send + Sync {
    // File CRUD
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError>;
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError>;
    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError>;
    fn remove_file(&self, path: &Path) -> Result<(), VfsError>;

    // Directory operations
    fn mkdir(&self, path: &Path) -> Result<(), VfsError>;
    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError>;
    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError>;
    fn remove_dir(&self, path: &Path) -> Result<(), VfsError>;
    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError>;

    // Metadata and permissions
    fn exists(&self, path: &Path) -> bool;
    fn stat(&self, path: &Path) -> Result<Metadata, VfsError>;
    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError>;
    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError>;
    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError>;

    // Links
    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError>;
    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError>;
    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError>;

    // Path resolution
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError>;

    // File operations
    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError>;
    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError>;

    // Glob expansion
    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError>;

    // Subshell isolation
    fn deep_clone(&self) -> Arc<dyn VirtualFs>;
}
```

All paths passed to VFS methods are resolved to absolute paths by the caller. The VFS itself does not track a "current directory" — that is the interpreter's responsibility.

## Backend Implementations

### InMemoryFs (Default)

The default backend. All data lives in memory. Zero `std::fs` calls.

**Data structure**: A tree of `FsNode` variants:

```rust
enum FsNode {
    File {
        content: Vec<u8>,
        mode: u32,
        mtime: SystemTime,
    },
    Directory {
        children: BTreeMap<String, FsNode>,
        mode: u32,
        mtime: SystemTime,
    },
    Symlink {
        target: PathBuf,
        mtime: SystemTime,
    },
}
```

**Thread safety**: The tree is wrapped in `Arc<parking_lot::RwLock<FsNode>>`. This enables:
- Cheap `Clone` (just Arc increment) — needed for subshell state cloning
- `Send + Sync` — needed for the `VirtualFs` trait bounds
- Non-poisoning locks (parking_lot) — a panicking command doesn't permanently kill the VFS

**Path normalization**: All paths go through normalization that:
- Resolves `.` and `..` components
- Handles absolute and relative paths (relative resolved against provided cwd)
- Strips trailing slashes
- Rejects empty paths

**Internal navigation helpers**:
- `with_node(path, f)` — read-lock, navigate to node, apply closure
- `with_node_mut(path, f)` — write-lock, navigate to node, apply closure
- `with_parent_mut(path, f)` — write-lock, navigate to parent, apply closure with child name

### OverlayFs (Copy-on-Write)

Reads from a real directory, writes to an in-memory layer. Changes never touch disk.

```rust
struct OverlayFs {
    lower: PathBuf,                              // real directory (read-only source)
    upper: InMemoryFs,                           // in-memory writes
    whiteouts: Arc<RwLock<HashSet<PathBuf>>>,    // tracks deletions
}
```

**Resolution order**:
1. Check if path is in `whiteouts` → return "not found"
2. Check `upper` (in-memory) → return if found
3. Check `lower` (real FS) → return if found
4. Return "not found"

**Write operations**: Always go to `upper`. The `lower` directory is never modified.

**Delete operations**: Add path to `whiteouts`. If the file exists in `upper`, also remove it from there.

**Subshell isolation** (`deep_clone`): Clones the upper layer and whiteout set. The lower directory reference is shared (it's read-only anyway).

**Use case**: Let an agent read a real project's files while sandboxing all writes. Perfect for code analysis tools, linters, or build system simulations.

**Example**:
```rust
use rust_bash::{RustBashBuilder, OverlayFs};
use std::sync::Arc;

let overlay = OverlayFs::new("./my_project").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(overlay))
    .cwd("/")
    .build()
    .unwrap();

// Reads come from disk
let result = shell.exec("cat /src/main.rs").unwrap();

// Writes stay in memory — disk is never modified
shell.exec("echo modified > /src/main.rs").unwrap();
```

### ReadWriteFs (Passthrough)

Thin wrapper over `std::fs` implementing the `VirtualFs` trait. For trusted execution where you want real filesystem access.

```rust
struct ReadWriteFs {
    root: Option<PathBuf>,  // optional chroot-like restriction
}
```

If `root` is set, all paths are resolved relative to it and path traversal beyond the root is rejected with `PermissionDenied`. Symlink-based escape attempts are also caught.

**Subshell isolation** (`deep_clone`): Creates a new `ReadWriteFs` with the same root — both instances point to the same real filesystem. There is no isolation since writes go directly to disk.

**Example**:
```rust
use rust_bash::{RustBashBuilder, ReadWriteFs};
use std::sync::Arc;

// Restricted to a directory (chroot-like)
let rwfs = ReadWriteFs::with_root("/tmp/sandbox").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(rwfs))
    .cwd("/")
    .build()
    .unwrap();

// Operations hit the real filesystem under /tmp/sandbox
shell.exec("echo hello > /output.txt").unwrap();  // writes to /tmp/sandbox/output.txt
```

### MountableFs (Composite)

Combines multiple backends at different mount points.

```rust
pub struct MountableFs {
    mounts: Arc<RwLock<BTreeMap<PathBuf, Arc<dyn VirtualFs>>>>,
}
```

**Resolution**: Find the longest matching mount prefix, delegate to that backend with the path stripped of the prefix. Uses `BTreeMap` reverse iteration for efficient longest-prefix lookup.

**Cross-mount operations**: `copy` and `rename` across mount boundaries use read+write (and delete for rename). `hardlink` across mounts returns an error, matching Unix behavior.

**Directory listings**: Mount points appear as synthetic directory entries in their parent's listing, even if the parent filesystem doesn't contain them.

**Subshell isolation** (`deep_clone`): Recursively deep-clones each mounted backend. Each mount gets its own independent copy.

**Example configuration**:
```rust
use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, OverlayFs};
use std::sync::Arc;

let mountable = MountableFs::new()
    .mount("/", Arc::new(InMemoryFs::new()))                          // default: in-memory
    .mount("/project", Arc::new(OverlayFs::new("./myproject").unwrap()))  // read real project
    .mount("/tmp", Arc::new(InMemoryFs::new()));                      // separate temp space

let mut shell = RustBashBuilder::new()
    .fs(Arc::new(mountable))
    .cwd("/")
    .build()
    .unwrap();
```

## VfsError

```rust
pub enum VfsError {
    NotFound(PathBuf),
    AlreadyExists(PathBuf),
    NotADirectory(PathBuf),
    NotAFile(PathBuf),
    IsADirectory(PathBuf),
    PermissionDenied(PathBuf),
    DirectoryNotEmpty(PathBuf),
    SymlinkLoop(PathBuf),
    InvalidPath(String),
    IoError(String),
}
```

VFS errors map to conventional Unix errno values for commands that check them. For example, `cat nonexistent.txt` maps `VfsError::NotFound` to the stderr message `cat: nonexistent.txt: No such file or directory` and exit code 1.

## Glob Implementation

The VFS glob walks the in-memory tree and matches paths against shell glob patterns:

- `*` matches any sequence of characters within a path component
- `?` matches exactly one character
- `[abc]` matches any character in the set
- `[a-z]` matches any character in the range
- `[!abc]` or `[^abc]` matches any character NOT in the set
- `**` matches any number of path components (recursive)

Glob results are limited by `ExecutionLimits::max_glob_results` to prevent patterns like `/**/*` from generating unbounded results.

## Design Decisions

**Why `&self` instead of `&mut self` on mutating methods?** Using `&self` allows a single `VirtualFs` instance to be shared by reference across the interpreter and command contexts without requiring exclusive borrow tracking at the call site. Implementations use interior mutability (`parking_lot::RwLock`) internally. The trade-off is that custom `VirtualFs` implementors must also use interior mutability.

**Why a trait instead of an enum?** The trait enables user-defined backends. A consumer of rust-bash can implement `VirtualFs` for their own storage system (e.g., S3-backed, database-backed) without modifying our codebase.

**Why `parking_lot::RwLock` instead of `std::sync::RwLock`?** Standard `RwLock` poisons on panic. If any command panics while holding the lock, all subsequent VFS operations fail with a poison error. `parking_lot::RwLock` doesn't poison — a panic releases the lock normally.

**Why `Vec<u8>` instead of `String` for file content?** Files can contain arbitrary bytes. Binary files, files with mixed encodings, and files with invalid UTF-8 must all be representable. Commands that operate on text (`grep`, `sort`, etc.) handle the UTF-8 conversion themselves.
