# Filesystem Backends

## Goal

Choose and configure the right virtual filesystem backend for your use case: fully sandboxed, copy-on-write over real files, direct host access, or a composite of all three.

## Overview

| Backend | Reads from | Writes to | Host access | Best for |
|---------|-----------|-----------|-------------|----------|
| `InMemoryFs` | Memory | Memory | None | Sandboxed execution, testing, AI agents |
| `OverlayFs` | Disk (lower) + Memory (upper) | Memory only | Read-only | Code analysis, safe experimentation |
| `ReadWriteFs` | Disk | Disk | Full (or chroot-restricted) | Trusted scripts, build tools |
| `MountableFs` | Delegated per mount | Delegated per mount | Depends on mounts | Composite environments |

## InMemoryFs (Default)

This is what you get with `RustBashBuilder::new().build()`. All data lives in memory; the host filesystem is never touched.

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/src/main.rs".into(), b"fn main() {}".to_vec()),
        ("/src/lib.rs".into(), b"pub fn hello() {}".to_vec()),
    ]))
    .build()
    .unwrap();

// Files exist only in memory
let result = shell.exec("find / -name '*.rs'").unwrap();
assert!(result.stdout.contains("/src/main.rs"));
assert!(result.stdout.contains("/src/lib.rs"));

// Writes stay in memory — no host files are created
shell.exec("echo new > /src/new.rs").unwrap();
```

## OverlayFs — Read Real Files, Sandbox Writes

Reads from a real directory on disk but all mutations stay in memory. The disk is never modified.

```rust
use rust_bash::{RustBashBuilder, OverlayFs};
use std::sync::Arc;

// Point at a real directory on the host
let overlay = OverlayFs::new("./my_project").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(overlay))
    .cwd("/")
    .build()
    .unwrap();

// Read files from disk (paths are relative to the overlay root)
let result = shell.exec("cat /Cargo.toml").unwrap();
println!("{}", result.stdout); // actual Cargo.toml contents

// Writes go to the in-memory upper layer
shell.exec("echo modified > /Cargo.toml").unwrap();
let result = shell.exec("cat /Cargo.toml").unwrap();
assert_eq!(result.stdout, "modified\n"); // reads the in-memory version

// Disk file is untouched:
// assert_eq!(std::fs::read_to_string("./my_project/Cargo.toml"), original)
```

### Deletions are tracked with whiteouts

```rust
use rust_bash::{RustBashBuilder, OverlayFs};
use std::sync::Arc;

let overlay = OverlayFs::new("./my_project").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(overlay))
    .cwd("/")
    .build()
    .unwrap();

// Delete a file that exists on disk — it becomes invisible but the disk file remains
shell.exec("rm /README.md").unwrap();
let result = shell.exec("cat /README.md").unwrap();
assert_ne!(result.exit_code, 0); // file appears deleted
// But on disk: std::fs::metadata("./my_project/README.md").is_ok() == true
```

## ReadWriteFs — Direct Filesystem Access

For trusted scripts that need real filesystem access. Use `with_root()` for chroot-like confinement.

```rust
use rust_bash::{RustBashBuilder, ReadWriteFs};
use std::sync::Arc;

// Unrestricted access to the entire filesystem:
// let rwfs = ReadWriteFs::new();

// Confined to a subtree (recommended):
let rwfs = ReadWriteFs::with_root("/tmp/sandbox").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(rwfs))
    .cwd("/")
    .build()
    .unwrap();

// All paths are resolved relative to the root
shell.exec("mkdir -p /output && echo hello > /output/result.txt").unwrap();
// This actually writes to /tmp/sandbox/output/result.txt on disk

// Path traversal beyond the root is blocked
let result = shell.exec("cat /../../etc/passwd").unwrap();
assert_ne!(result.exit_code, 0); // PermissionDenied
```

## MountableFs — Combine Backends

Delegate different path prefixes to different backends. Longest-prefix matching determines which backend handles each operation.

```rust
use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, OverlayFs};
use std::sync::Arc;

let mountable = MountableFs::new()
    .mount("/", Arc::new(InMemoryFs::new()))                             // in-memory root
    .mount("/project", Arc::new(OverlayFs::new("./myproject").unwrap())) // overlay on real project
    .mount("/tmp", Arc::new(InMemoryFs::new()));                         // separate temp space

let mut shell = RustBashBuilder::new()
    .fs(Arc::new(mountable))
    .cwd("/")
    .build()
    .unwrap();

// /project reads from disk via OverlayFs
shell.exec("cat /project/Cargo.toml").unwrap();

// /tmp is a separate in-memory space
shell.exec("echo scratch > /tmp/work.txt").unwrap();

// / is the default in-memory backend
shell.exec("echo hello > /root-file.txt").unwrap();
```

### Real-world example: isolated build environment

```rust
use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, ReadWriteFs};
use std::sync::Arc;

let mountable = MountableFs::new()
    .mount("/", Arc::new(InMemoryFs::new()))
    .mount("/output", Arc::new(ReadWriteFs::with_root("/tmp/build-output").unwrap()));

let mut shell = RustBashBuilder::new()
    .fs(Arc::new(mountable))
    .cwd("/")
    .build()
    .unwrap();

// Script can write real files only under /output
shell.exec("echo 'build artifact' > /output/result.txt").unwrap();
// /output/result.txt is a real file at /tmp/build-output/result.txt

// Everything else is sandboxed in memory
shell.exec("echo 'temp data' > /scratch.txt").unwrap();
// /scratch.txt exists only in memory
```

## Seeding Files from a Host Directory

The builder's `.files()` method accepts a `HashMap<String, Vec<u8>>`. To load files from a host directory:

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;
use std::path::Path;

fn load_dir(dir: &Path, prefix: &str) -> HashMap<String, Vec<u8>> {
    let mut files = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
            if path.is_file() {
                if let Ok(data) = std::fs::read(&path) {
                    files.insert(name, data);
                }
            } else if path.is_dir() {
                files.extend(load_dir(&path, &name));
            }
        }
    }
    files
}

let files = load_dir(Path::new("./test-fixtures"), "");
let mut shell = RustBashBuilder::new()
    .files(files)
    .build()
    .unwrap();
```

This copies files into the InMemoryFs at build time. For large directories, prefer `OverlayFs` to avoid the upfront memory cost.

---

## TypeScript: Virtual Filesystem

The `@rust-bash/core` npm package provides file seeding at creation time and direct VFS access.

### Seeding Files

```typescript
import { Bash } from '@rust-bash/core';

const bash = await Bash.create(createBackend, {
  files: {
    '/src/main.rs': 'fn main() {}',
    '/src/lib.rs': 'pub fn hello() {}',
    '/config.json': '{"debug": true}',
  },
});

const result = await bash.exec('find / -name "*.rs"');
// result.stdout includes /src/main.rs and /src/lib.rs
```

### Lazy File Loading

File values can be functions — resolved concurrently at `Bash.create()` time via `Promise.all`. This keeps the config declarative while deferring expensive I/O until the instance is actually created:

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    // Eager — written immediately
    '/data.txt': 'hello world',

    // Lazy sync — resolved at Bash.create() time
    '/config.json': () => JSON.stringify(getConfig()),

    // Lazy async — resolved at Bash.create() time (awaited)
    '/remote.txt': async () => {
      const res = await fetch('https://example.com/data');
      return await res.text();
    },
  },
});

// /remote.txt is only fetched when a command reads it:
await bash.exec('cat /remote.txt');
```

### Direct VFS Access

The `bash.fs` proxy provides synchronous filesystem operations:

```typescript
// Write files
bash.fs.writeFileSync('/output.txt', 'content');

// Read files
const data = bash.fs.readFileSync('/output.txt');

// Check existence
const exists = bash.fs.existsSync('/output.txt');

// Create directories
bash.fs.mkdirSync('/dir/subdir', { recursive: true });

// List directory contents
const entries = bash.fs.readdirSync('/');

// File stats
const stat = bash.fs.statSync('/output.txt');
console.log(stat.isFile, stat.size);

// Remove files
bash.fs.rmSync('/output.txt');
bash.fs.rmSync('/dir', { recursive: true });
```

### Browser Example

In the browser, only `InMemoryFs` is available (no host filesystem access):

```typescript
import { Bash, initWasm, createWasmBackend } from '@rust-bash/core/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend, {
  files: {
    '/index.html': '<h1>Hello</h1>',
    '/style.css': 'body { color: red; }',
  },
});

// All operations are in-memory
await bash.exec('cat /index.html | grep -o "Hello"');
bash.fs.writeFileSync('/output.txt', 'generated');
```
