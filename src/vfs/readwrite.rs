//! ReadWriteFs — thin `std::fs` passthrough implementing `VirtualFs`.
//!
//! When `root` is set, all paths are resolved relative to it and path
//! traversal beyond the root is rejected with `PermissionDenied`.
//!
//! # Safety (TOCTOU)
//!
//! Between path resolution and the actual `std::fs` operation, symlinks
//! could theoretically be swapped. This is inherent to real-FS operations
//! and matches the behavior of other chroot-like implementations.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::platform::SystemTime;

use crate::error::VfsError;
use crate::interpreter::pattern::glob_match;

use super::{DirEntry, Metadata, NodeType, VirtualFs};

/// A passthrough filesystem backed by `std::fs`.
///
/// With no root restriction, all operations delegate directly to the real
/// filesystem. When a root is set, paths are confined to the subtree under
/// that root — acting like a lightweight chroot.
///
/// # Example
///
/// ```ignore
/// use rust_bash::{RustBashBuilder, ReadWriteFs};
/// use std::sync::Arc;
///
/// let rwfs = ReadWriteFs::with_root("/tmp/sandbox").unwrap();
/// let mut shell = RustBashBuilder::new()
///     .fs(Arc::new(rwfs))
///     .cwd("/")
///     .build()
///     .unwrap();
///
/// shell.exec("echo hello > /output.txt").unwrap(); // writes to /tmp/sandbox/output.txt
/// ```
pub struct ReadWriteFs {
    root: Option<PathBuf>,
}

impl ReadWriteFs {
    /// Create a ReadWriteFs with unrestricted access to the real filesystem.
    pub fn new() -> Self {
        Self { root: None }
    }

    /// Create a ReadWriteFs restricted to paths under `root`.
    ///
    /// All paths are resolved relative to `root`. Path traversal beyond
    /// `root` (via `..` or symlinks) is rejected with `PermissionDenied`.
    /// The root directory must exist and is canonicalized on construction.
    pub fn with_root(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into().canonicalize()?;
        Ok(Self { root: Some(root) })
    }

    /// Resolve a virtual path to a real filesystem path (does not follow the
    /// final path component if it is a symlink).
    ///
    /// When `root` is None, paths are returned as-is.
    /// When `root` is set:
    /// 1. Strip leading `/` from path, join with root
    /// 2. Logically normalize (resolve `.` and `..`)
    /// 3. Canonicalize the *parent* of the final component (follows symlinks
    ///    in intermediate directories for security)
    /// 4. Append the final component without following it
    /// 5. Verify result starts with root
    fn resolve(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let Some(root) = &self.root else {
            return Ok(path.to_path_buf());
        };

        // Strip leading '/' to make the path relative to root.
        let lossy = path.to_string_lossy();
        let rel_str = lossy.trim_start_matches('/');
        let joined = if rel_str.is_empty() {
            root.clone()
        } else {
            root.join(rel_str)
        };

        // Logically resolve . and .. without touching the filesystem.
        let normalized = logical_normalize(&joined);

        // Quick check: after logical normalization, must still be under root.
        if !normalized.starts_with(root) {
            return Err(VfsError::PermissionDenied(path.to_path_buf()));
        }

        // If the normalized path IS root (e.g., virtual "/"), canonicalize it.
        if normalized == *root {
            return Ok(root.clone());
        }

        // Split into parent and final component.
        let name = normalized
            .file_name()
            .expect("normalized path has a filename")
            .to_owned();
        let parent = normalized.parent().unwrap_or(root);

        // Canonicalize the parent (follows symlinks in intermediate dirs).
        let canonical_parent = canonicalize_existing(parent, path, root)?;

        // Security check on the parent.
        if !canonical_parent.starts_with(root) {
            return Err(VfsError::PermissionDenied(path.to_path_buf()));
        }

        Ok(canonical_parent.join(name))
    }

    /// Like `resolve`, but also verifies that the *final* component (if it
    /// is a symlink) doesn't escape the root.  Use for operations that follow
    /// symlinks (read_file, stat, write_file, etc.).
    ///
    /// When the final component is a symlink, returns the canonical (target)
    /// path to close the TOCTOU gap for the last component.
    fn resolve_follow(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let resolved = self.resolve(path)?;
        if let Some(root) = &self.root {
            match std::fs::symlink_metadata(&resolved) {
                Ok(meta) if meta.is_symlink() => {
                    let canonical =
                        std::fs::canonicalize(&resolved).map_err(|e| map_io_error(e, path))?;
                    if !canonical.starts_with(root) {
                        return Err(VfsError::PermissionDenied(path.to_path_buf()));
                    }
                    return Ok(canonical);
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(map_io_error(e, path)),
            }
        }
        Ok(resolved)
    }

    /// Check whether a real path is within the root (for glob walking).
    fn is_within_root(&self, real_path: &Path) -> bool {
        let Some(root) = &self.root else {
            return true;
        };
        match std::fs::canonicalize(real_path) {
            Ok(canonical) => canonical.starts_with(root),
            Err(_) => real_path.starts_with(root),
        }
    }

    /// Recursive glob walker over the real directory tree.
    fn glob_walk(
        &self,
        real_dir: &Path,
        components: &[&str],
        virtual_path: PathBuf,
        results: &mut Vec<PathBuf>,
        max: usize,
    ) {
        if results.len() >= max || components.is_empty() {
            if components.is_empty() {
                results.push(virtual_path);
            }
            return;
        }

        let pattern = components[0];
        let rest = &components[1..];

        if pattern == "**" {
            // Zero directories — advance past **
            self.glob_walk(real_dir, rest, virtual_path.clone(), results, max);

            // One or more directories — recurse into each child
            let Ok(entries) = std::fs::read_dir(real_dir) else {
                return;
            };
            for entry in entries.flatten() {
                if results.len() >= max {
                    return;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let child_real = real_dir.join(&name);
                let child_virtual = virtual_path.join(&name);

                let is_dir = entry
                    .file_type()
                    .is_ok_and(|ft| ft.is_dir() || ft.is_symlink());
                if is_dir && self.is_within_root(&child_real) {
                    // Continue with ** (recurse deeper)
                    self.glob_walk(&child_real, components, child_virtual, results, max);
                }
            }
        } else {
            let Ok(entries) = std::fs::read_dir(real_dir) else {
                return;
            };
            for entry in entries.flatten() {
                if results.len() >= max {
                    return;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                // Skip hidden files unless pattern explicitly starts with '.'
                if name.starts_with('.') && !pattern.starts_with('.') {
                    continue;
                }
                if glob_match(pattern, &name) {
                    let child_real = real_dir.join(&name);
                    let child_virtual = virtual_path.join(&name);
                    if rest.is_empty() {
                        results.push(child_virtual);
                    } else {
                        let is_dir = entry
                            .file_type()
                            .is_ok_and(|ft| ft.is_dir() || ft.is_symlink());
                        if is_dir && self.is_within_root(&child_real) {
                            self.glob_walk(&child_real, rest, child_virtual, results, max);
                        }
                    }
                }
            }
        }
    }
}

impl Default for ReadWriteFs {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// VirtualFs implementation
// ---------------------------------------------------------------------------

impl VirtualFs for ReadWriteFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let resolved = self.resolve_follow(path)?;
        std::fs::read(&resolved).map_err(|e| map_io_error(e, path))
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let resolved = self.resolve_follow(path)?;
        std::fs::write(&resolved, content).map_err(|e| map_io_error(e, path))
    }

    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let resolved = self.resolve_follow(path)?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&resolved)
            .map_err(|e| map_io_error(e, path))?;
        file.write_all(content).map_err(|e| map_io_error(e, path))
    }

    fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let resolved = self.resolve(path)?;
        std::fs::remove_file(&resolved).map_err(|e| map_io_error(e, path))
    }

    fn mkdir(&self, path: &Path) -> Result<(), VfsError> {
        let resolved = self.resolve(path)?;
        std::fs::create_dir(&resolved).map_err(|e| map_io_error(e, path))
    }

    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError> {
        let resolved = self.resolve(path)?;
        std::fs::create_dir_all(&resolved).map_err(|e| map_io_error(e, path))
    }

    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let resolved = self.resolve_follow(path)?;
        let entries = std::fs::read_dir(&resolved).map_err(|e| map_io_error(e, path))?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| map_io_error(e, path))?;
            let ft = entry.file_type().map_err(|e| map_io_error(e, path))?;
            let node_type = if ft.is_dir() {
                NodeType::Directory
            } else if ft.is_symlink() {
                NodeType::Symlink
            } else {
                NodeType::File
            };
            result.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                node_type,
            });
        }
        Ok(result)
    }

    fn remove_dir(&self, path: &Path) -> Result<(), VfsError> {
        let resolved = self.resolve(path)?;
        std::fs::remove_dir(&resolved).map_err(|e| map_io_error(e, path))
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        let resolved = self.resolve(path)?;
        std::fs::remove_dir_all(&resolved).map_err(|e| map_io_error(e, path))
    }

    fn exists(&self, path: &Path) -> bool {
        match self.resolve(path) {
            Ok(resolved) => resolved.exists(),
            Err(_) => false,
        }
    }

    fn stat(&self, path: &Path) -> Result<Metadata, VfsError> {
        let resolved = self.resolve_follow(path)?;
        let meta = std::fs::metadata(&resolved).map_err(|e| map_io_error(e, path))?;
        Ok(map_metadata(&meta))
    }

    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError> {
        let resolved = self.resolve(path)?;
        let meta = std::fs::symlink_metadata(&resolved).map_err(|e| map_io_error(e, path))?;
        Ok(map_metadata(&meta))
    }

    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError> {
        let resolved = self.resolve_follow(path)?;
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&resolved, perms).map_err(|e| map_io_error(e, path))
    }

    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError> {
        let resolved = self.resolve_follow(path)?;
        let file = std::fs::File::options()
            .write(true)
            .open(&resolved)
            .map_err(|e| map_io_error(e, path))?;
        file.set_times(std::fs::FileTimes::new().set_modified(mtime))
            .map_err(|e| map_io_error(e, path))
    }

    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError> {
        let resolved_link = self.resolve(link)?;
        // If rooted and target is absolute, resolve it too so the on-disk
        // symlink points to the correct real location.
        let actual_target = if target.is_absolute() && self.root.is_some() {
            self.resolve(target)?
        } else {
            target.to_path_buf()
        };
        std::os::unix::fs::symlink(&actual_target, &resolved_link)
            .map_err(|e| map_io_error(e, link))
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let resolved_src = self.resolve_follow(src)?;
        let resolved_dst = self.resolve(dst)?;
        std::fs::hard_link(&resolved_src, &resolved_dst).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                map_io_error(e, src)
            } else {
                map_io_error(e, dst)
            }
        })
    }

    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let resolved = self.resolve(path)?;
        let target = std::fs::read_link(&resolved).map_err(|e| map_io_error(e, path))?;
        // If rooted and target is absolute, convert back to virtual.
        if let Some(root) = &self.root
            && target.is_absolute()
            && let Ok(rel) = target.strip_prefix(root)
        {
            return Ok(PathBuf::from("/").join(rel));
        }
        Ok(target)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let resolved = self.resolve(path)?;
        let canonical = std::fs::canonicalize(&resolved).map_err(|e| map_io_error(e, path))?;
        if let Some(root) = &self.root {
            if !canonical.starts_with(root) {
                return Err(VfsError::PermissionDenied(path.to_path_buf()));
            }
            let rel = canonical.strip_prefix(root).unwrap();
            Ok(PathBuf::from("/").join(rel))
        } else {
            Ok(canonical)
        }
    }

    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let resolved_src = self.resolve_follow(src)?;
        let resolved_dst = self.resolve(dst)?;
        std::fs::copy(&resolved_src, &resolved_dst).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                map_io_error(e, src)
            } else {
                map_io_error(e, dst)
            }
        })?;
        Ok(())
    }

    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let resolved_src = self.resolve(src)?;
        let resolved_dst = self.resolve(dst)?;
        std::fs::rename(&resolved_src, &resolved_dst).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                map_io_error(e, src)
            } else {
                map_io_error(e, dst)
            }
        })
    }

    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let is_absolute = pattern.starts_with('/');
        let abs_pattern = if is_absolute {
            pattern.to_string()
        } else {
            let cwd_str = cwd.to_str().unwrap_or("/").trim_end_matches('/');
            format!("{cwd_str}/{pattern}")
        };

        let components: Vec<&str> = abs_pattern.split('/').filter(|s| !s.is_empty()).collect();

        // Always walk from the real root — pattern components are absolute.
        let real_root = self.resolve(Path::new("/"))?;

        let mut results = Vec::new();
        let max = 100_000;
        self.glob_walk(
            &real_root,
            &components,
            PathBuf::from("/"),
            &mut results,
            max,
        );

        results.sort();
        results.dedup();

        if !is_absolute {
            results = results
                .into_iter()
                .filter_map(|p| p.strip_prefix(cwd).ok().map(|r| r.to_path_buf()))
                .collect();
        }

        Ok(results)
    }

    fn deep_clone(&self) -> Arc<dyn VirtualFs> {
        // ReadWriteFs is a passthrough — there's no in-memory state to isolate.
        // Subshell writes hit the real filesystem, same as the parent.
        Arc::new(Self {
            root: self.root.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Logically normalize a path by resolving `.` and `..` without filesystem access.
fn logical_normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::RootDir | Component::Prefix(_) => {
                parts.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(c) => parts.push(c),
        }
    }
    let mut result = PathBuf::from("/");
    for part in parts {
        result.push(part);
    }
    result
}

/// Canonicalize a path, walking up to find the deepest existing ancestor
/// when the full path doesn't exist.  Non-existent tail components are
/// appended back after canonicalizing the existing prefix.
fn canonicalize_existing(path: &Path, original: &Path, root: &Path) -> Result<PathBuf, VfsError> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_owned());
                existing.pop();
            }
            None => break,
        }
    }
    let canonical = if existing.exists() {
        std::fs::canonicalize(&existing).map_err(|e| map_io_error(e, original))?
    } else {
        existing
    };

    // Security check on the canonicalized existing portion.
    if !canonical.starts_with(root) {
        return Err(VfsError::PermissionDenied(original.to_path_buf()));
    }

    let mut result = canonical;
    for component in tail.into_iter().rev() {
        result.push(component);
    }
    Ok(result)
}

/// Map `std::io::Error` to `VfsError`.
fn map_io_error(err: std::io::Error, path: &Path) -> VfsError {
    let p = path.to_path_buf();
    match err.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound(p),
        std::io::ErrorKind::AlreadyExists => VfsError::AlreadyExists(p),
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied(p),
        std::io::ErrorKind::DirectoryNotEmpty => VfsError::DirectoryNotEmpty(p),
        std::io::ErrorKind::NotADirectory => VfsError::NotADirectory(p),
        std::io::ErrorKind::IsADirectory => VfsError::IsADirectory(p),
        _ => VfsError::IoError(err.to_string()),
    }
}

/// Map `std::fs::Metadata` to our `vfs::Metadata`.
fn map_metadata(meta: &std::fs::Metadata) -> Metadata {
    let node_type = if meta.is_symlink() {
        NodeType::Symlink
    } else if meta.is_dir() {
        NodeType::Directory
    } else {
        NodeType::File
    };
    Metadata {
        node_type,
        size: meta.len(),
        mode: meta.permissions().mode(),
        mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    }
}
