mod memory;
mod mountable;

#[cfg(feature = "native-fs")]
mod overlay;
#[cfg(feature = "native-fs")]
mod readwrite;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "native-fs"))]
mod readwrite_tests;

#[cfg(all(test, feature = "native-fs"))]
mod overlay_tests;

#[cfg(test)]
mod mountable_tests;

pub use memory::InMemoryFs;
pub use mountable::MountableFs;

#[cfg(feature = "native-fs")]
pub use overlay::OverlayFs;
#[cfg(feature = "native-fs")]
pub use readwrite::ReadWriteFs;

use crate::error::VfsError;
use crate::platform::SystemTime;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// VFS paths always use Unix-style `/` separators. `std::path::Path::is_absolute()`
/// is platform-dependent and returns `false` on `wasm32-unknown-unknown` even for
/// `/home/user`, so we roll our own check.
pub(crate) fn vfs_path_is_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(|s| s.starts_with('/'))
}

/// Metadata for a filesystem node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub node_type: NodeType,
    pub size: u64,
    pub mode: u32,
    pub mtime: SystemTime,
    pub file_id: u64,
}

/// The type of a filesystem node (without content).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    File,
    Directory,
    Symlink,
}

/// An entry returned by `readdir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub node_type: NodeType,
}

/// In-memory representation of a filesystem node.
#[derive(Debug, Clone)]
pub enum FsNode {
    File {
        content: Vec<u8>,
        mode: u32,
        mtime: SystemTime,
        file_id: u64,
    },
    Directory {
        children: std::collections::BTreeMap<String, FsNode>,
        mode: u32,
        mtime: SystemTime,
    },
    Symlink {
        target: PathBuf,
        mtime: SystemTime,
    },
}

/// Options that modify glob expansion behavior.
#[derive(Debug, Clone)]
pub struct GlobOptions {
    /// Include dot-files even when the pattern doesn't start with `.`.
    pub dotglob: bool,
    /// Use case-insensitive matching for filenames.
    pub nocaseglob: bool,
    /// Treat `**` as recursive directory match (globstar).
    /// When false, `**` is treated as `*`.
    pub globstar: bool,
    /// Enable extended glob patterns: `@(...)`, `+(...)`, `*(...)`, `?(...)`, `!(...)`.
    pub extglob: bool,
    /// When true (default), `.` and `..` are excluded from glob results.
    pub globskipdots: bool,
}

impl Default for GlobOptions {
    fn default() -> Self {
        Self {
            dotglob: false,
            nocaseglob: false,
            globstar: false,
            extglob: false,
            globskipdots: true,
        }
    }
}

/// Trait abstracting all filesystem operations.
///
/// All methods take `&self` — implementations use interior mutability.
/// All paths are expected to be absolute.
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

    // Glob expansion (stub for now)
    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError>;

    /// Glob expansion with shopt-controlled options (dotglob, nocaseglob, globstar).
    ///
    /// The default implementation ignores options and delegates to `glob()`.
    /// Override in backends that can honor the options.
    fn glob_with_opts(
        &self,
        pattern: &str,
        cwd: &Path,
        _opts: &GlobOptions,
    ) -> Result<Vec<PathBuf>, VfsError> {
        self.glob(pattern, cwd)
    }

    /// Create an independent deep copy for subshell isolation.
    ///
    /// Subshells `( ... )` and command substitutions `$(...)` need an isolated
    /// filesystem so their mutations don't leak back to the parent. Each backend
    /// decides what "independent copy" means:
    /// - InMemoryFs: clones the entire tree
    /// - OverlayFs: clones the upper layer and whiteouts; lower is shared
    /// - ReadWriteFs: no isolation (returns Arc::clone — writes hit real FS)
    /// - MountableFs: recursively deep-clones each mount
    fn deep_clone(&self) -> Arc<dyn VirtualFs>;
}
