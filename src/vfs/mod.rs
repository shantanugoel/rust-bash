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

/// Metadata for a filesystem node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub node_type: NodeType,
    pub size: u64,
    pub mode: u32,
    pub mtime: SystemTime,
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
