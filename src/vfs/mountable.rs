//! MountableFs — composite filesystem that delegates to different backends
//! based on longest-prefix mount point matching.
//!
//! Each mount point maps an absolute path to a `VirtualFs` backend. When an
//! operation arrives, MountableFs finds the longest mount prefix that matches
//! the path, strips the prefix, re-roots the remainder as absolute, and
//! delegates to that backend.
//!
//! Mounting at `"/"` provides a default fallback for all paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::platform::SystemTime;

use parking_lot::RwLock;

use super::{DirEntry, Metadata, NodeType, VirtualFs};
use crate::error::VfsError;
use crate::interpreter::pattern::glob_match;

/// Result of resolving two paths to their respective mounts.
struct MountPair {
    src_fs: Arc<dyn VirtualFs>,
    src_rel: PathBuf,
    dst_fs: Arc<dyn VirtualFs>,
    dst_rel: PathBuf,
    same: bool,
}

/// A composite filesystem that delegates to mounted backends via longest-prefix
/// matching.
///
/// # Example
///
/// ```ignore
/// use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, OverlayFs};
/// use std::sync::Arc;
///
/// let mountable = MountableFs::new()
///     .mount("/", Arc::new(InMemoryFs::new()))
///     .mount("/project", Arc::new(OverlayFs::new("./myproject").unwrap()));
///
/// let mut shell = RustBashBuilder::new()
///     .fs(Arc::new(mountable))
///     .cwd("/")
///     .build()
///     .unwrap();
/// ```
pub struct MountableFs {
    mounts: Arc<RwLock<BTreeMap<PathBuf, Arc<dyn VirtualFs>>>>,
}

impl MountableFs {
    /// Create an empty MountableFs with no mount points.
    pub fn new() -> Self {
        Self {
            mounts: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Mount a filesystem backend at the given absolute path.
    ///
    /// Paths must be absolute. Mounting at `"/"` provides the default fallback.
    /// Later mounts at the same path replace earlier ones.
    pub fn mount(self, path: impl Into<PathBuf>, fs: Arc<dyn VirtualFs>) -> Self {
        let path = path.into();
        assert!(
            super::vfs_path_is_absolute(&path),
            "mount path must be absolute: {path:?}"
        );
        self.mounts.write().insert(path, fs);
        self
    }

    /// Find the mount that owns the given path.
    ///
    /// Returns the mount's filesystem and the path relative to the mount point,
    /// re-rooted as absolute (prepended with `/`).
    ///
    /// BTreeMap sorts lexicographically, so `/project/src` > `/project`.
    /// Iterating in reverse gives longest-prefix first.
    fn resolve_mount(&self, path: &Path) -> Result<(Arc<dyn VirtualFs>, PathBuf), VfsError> {
        let mounts = self.mounts.read();
        for (mount_point, fs) in mounts.iter().rev() {
            if path.starts_with(mount_point) {
                let relative = path.strip_prefix(mount_point).unwrap_or(Path::new(""));
                let resolved = if relative.as_os_str().is_empty() {
                    PathBuf::from("/")
                } else {
                    PathBuf::from("/").join(relative)
                };
                return Ok((Arc::clone(fs), resolved));
            }
        }
        Err(VfsError::NotFound(path.to_path_buf()))
    }

    /// Resolve mount for two paths (used by copy/rename/hardlink).
    fn resolve_two(&self, src: &Path, dst: &Path) -> Result<MountPair, VfsError> {
        let mounts = self.mounts.read();
        let resolve_one =
            |path: &Path| -> Result<(Arc<dyn VirtualFs>, PathBuf, PathBuf), VfsError> {
                for (mount_point, fs) in mounts.iter().rev() {
                    if path.starts_with(mount_point) {
                        let relative = path.strip_prefix(mount_point).unwrap_or(Path::new(""));
                        let resolved = if relative.as_os_str().is_empty() {
                            PathBuf::from("/")
                        } else {
                            PathBuf::from("/").join(relative)
                        };
                        return Ok((Arc::clone(fs), resolved, mount_point.clone()));
                    }
                }
                Err(VfsError::NotFound(path.to_path_buf()))
            };

        let (src_fs, src_rel, src_mount) = resolve_one(src)?;
        let (dst_fs, dst_rel, dst_mount) = resolve_one(dst)?;
        let same = src_mount == dst_mount;
        Ok(MountPair {
            src_fs,
            src_rel,
            dst_fs,
            dst_rel,
            same,
        })
    }

    /// Collect synthetic directory entries from mount points that are direct
    /// children of `dir_path`. For example, if mounts exist at `/project` and
    /// `/project/src`, listing `/` should include `project` and listing
    /// `/project` should include `src`.
    fn synthetic_mount_entries(&self, dir_path: &Path) -> Vec<DirEntry> {
        let mounts = self.mounts.read();
        let mut entries = Vec::new();
        let dir_str = dir_path.to_string_lossy();
        let prefix = if dir_str == "/" {
            "/".to_string()
        } else {
            format!("{}/", dir_str.trim_end_matches('/'))
        };

        for mount_point in mounts.keys() {
            // Skip the mount if it IS the directory itself.
            if mount_point == dir_path {
                continue;
            }
            let mp_str = mount_point.to_string_lossy();
            if let Some(rest) = mp_str.strip_prefix(&prefix)
                && !rest.is_empty()
            {
                // Take only the first path component (handles deep mounts
                // like /a/b/c when listing /a).
                let first_component = rest.split('/').next().unwrap();
                if !entries.iter().any(|e: &DirEntry| e.name == first_component) {
                    entries.push(DirEntry {
                        name: first_component.to_string(),
                        node_type: NodeType::Directory,
                    });
                }
            }
        }
        entries
    }

    /// Recursive glob walker that spans mount boundaries.
    fn glob_walk(
        &self,
        dir: &Path,
        components: &[&str],
        current_path: PathBuf,
        results: &mut Vec<PathBuf>,
        max: usize,
    ) {
        if results.len() >= max || components.is_empty() {
            if components.is_empty() {
                results.push(current_path);
            }
            return;
        }

        let pattern = components[0];
        let rest = &components[1..];

        // Get entries from the mounted fs (if any) merged with synthetic mount entries.
        let entries = self.merged_readdir_for_glob(dir);

        if pattern == "**" {
            // Zero directories — advance past **
            self.glob_walk(dir, rest, current_path.clone(), results, max);

            for entry in &entries {
                if results.len() >= max {
                    return;
                }
                if entry.name.starts_with('.') {
                    continue;
                }
                let child_path = current_path.join(&entry.name);
                let child_dir = dir.join(&entry.name);
                if entry.node_type == NodeType::Directory || entry.node_type == NodeType::Symlink {
                    self.glob_walk(&child_dir, components, child_path, results, max);
                }
            }
        } else {
            for entry in &entries {
                if results.len() >= max {
                    return;
                }
                if entry.name.starts_with('.') && !pattern.starts_with('.') {
                    continue;
                }
                if glob_match(pattern, &entry.name) {
                    let child_path = current_path.join(&entry.name);
                    let child_dir = dir.join(&entry.name);
                    if rest.is_empty() {
                        results.push(child_path);
                    } else if entry.node_type == NodeType::Directory
                        || entry.node_type == NodeType::Symlink
                    {
                        self.glob_walk(&child_dir, rest, child_path, results, max);
                    }
                }
            }
        }
    }

    /// Get directory entries for glob walking: real entries from the mount
    /// merged with synthetic mount-point entries.
    fn merged_readdir_for_glob(&self, dir: &Path) -> Vec<DirEntry> {
        let mut entries = match self.resolve_mount(dir) {
            Ok((fs, rel)) => fs.readdir(&rel).unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        // Add synthetic entries for child mount points.
        let synthetics = self.synthetic_mount_entries(dir);
        let existing_names: std::collections::HashSet<String> =
            entries.iter().map(|e| e.name.clone()).collect();
        for s in synthetics {
            if !existing_names.contains(&s.name) {
                entries.push(s);
            }
        }
        entries
    }
}

impl Default for MountableFs {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// VirtualFs implementation
// ---------------------------------------------------------------------------

impl VirtualFs for MountableFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.read_file(&rel)
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.write_file(&rel, content)
    }

    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.append_file(&rel, content)
    }

    fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.remove_file(&rel)
    }

    fn mkdir(&self, path: &Path) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.mkdir(&rel)
    }

    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.mkdir_p(&rel)
    }

    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        // Track whether the underlying mount confirmed this directory exists.
        let (mut entries, mount_ok) = match self.resolve_mount(path) {
            Ok((fs, rel)) => match fs.readdir(&rel) {
                Ok(e) => (e, true),
                Err(_) => (Vec::new(), false),
            },
            Err(_) => (Vec::new(), false),
        };

        // Merge in synthetic entries from child mount points.
        let synthetics = self.synthetic_mount_entries(path);
        let existing_names: std::collections::HashSet<String> =
            entries.iter().map(|e| e.name.clone()).collect();
        for s in synthetics {
            if !existing_names.contains(&s.name) {
                entries.push(s);
            }
        }

        // Only return NotFound when the mount itself errored AND there are no
        // synthetic entries from child mounts. An empty directory that the
        // mount confirmed is legitimate.
        if !mount_ok && entries.is_empty() {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        Ok(entries)
    }

    fn remove_dir(&self, path: &Path) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.remove_dir(&rel)
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.remove_dir_all(&rel)
    }

    fn exists(&self, path: &Path) -> bool {
        // A path exists if its owning mount says so, OR if it is itself a
        // mount point (mount points are treated as existing directories).
        if let Ok((fs, rel)) = self.resolve_mount(path)
            && fs.exists(&rel)
        {
            return true;
        }
        // Check if this exact path is a mount point.
        let mounts = self.mounts.read();
        if mounts.contains_key(path) {
            return true;
        }
        // Check if any mount is a descendant (making this a synthetic parent).
        let prefix = if path == Path::new("/") {
            "/".to_string()
        } else {
            format!("{}/", path.to_string_lossy().trim_end_matches('/'))
        };
        mounts
            .keys()
            .any(|mp| mp.to_string_lossy().starts_with(&prefix))
    }

    fn stat(&self, path: &Path) -> Result<Metadata, VfsError> {
        // Try the owning mount first.
        if let Ok((fs, rel)) = self.resolve_mount(path)
            && let Ok(m) = fs.stat(&rel)
        {
            return Ok(m);
        }
        // If this path is a mount point or has child mounts, return synthetic
        // directory metadata.
        if self.is_mount_point_or_ancestor(path) {
            return Ok(Metadata {
                node_type: NodeType::Directory,
                size: 0,
                mode: 0o755,
                mtime: SystemTime::UNIX_EPOCH,
                file_id: 0,
            });
        }
        Err(VfsError::NotFound(path.to_path_buf()))
    }

    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError> {
        if let Ok((fs, rel)) = self.resolve_mount(path)
            && let Ok(m) = fs.lstat(&rel)
        {
            return Ok(m);
        }
        if self.is_mount_point_or_ancestor(path) {
            return Ok(Metadata {
                node_type: NodeType::Directory,
                size: 0,
                mode: 0o755,
                mtime: SystemTime::UNIX_EPOCH,
                file_id: 0,
            });
        }
        Err(VfsError::NotFound(path.to_path_buf()))
    }

    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.chmod(&rel, mode)
    }

    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        fs.utimes(&rel, mtime)
    }

    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError> {
        let (link_fs, link_rel) = self.resolve_mount(link)?;
        // If the target is absolute and resolves to the same mount as the link,
        // remap it into the mount's namespace so the underlying FS can follow it.
        let remapped_target = if target.is_absolute() {
            if let Ok((_, target_rel)) = self.resolve_mount(target) {
                // Find mount point for the link to compare
                let link_mount = self.mount_point_for(link);
                let target_mount = self.mount_point_for(target);
                if link_mount == target_mount {
                    target_rel
                } else {
                    target.to_path_buf()
                }
            } else {
                target.to_path_buf()
            }
        } else {
            target.to_path_buf()
        };
        link_fs.symlink(&remapped_target, &link_rel)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let pair = self.resolve_two(src, dst)?;
        if !pair.same {
            return Err(VfsError::IoError(
                "hard links across mount boundaries are not supported".to_string(),
            ));
        }
        pair.src_fs.hardlink(&pair.src_rel, &pair.dst_rel)
    }

    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        let target = fs.readlink(&rel)?;
        // If the target is absolute and the link lives at a non-root mount,
        // remap the target back to the global namespace.
        if target.is_absolute() {
            let mount_point = self.mount_point_for(path);
            if mount_point != Path::new("/") {
                let inner_rel = target.strip_prefix("/").unwrap_or(&target);
                if inner_rel.as_os_str().is_empty() {
                    return Ok(mount_point);
                }
                return Ok(mount_point.join(inner_rel));
            }
        }
        Ok(target)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let (fs, rel) = self.resolve_mount(path)?;
        let canonical_in_mount = fs.canonicalize(&rel)?;
        // Re-root back to global namespace: find what mount we used, prepend
        // the mount point.
        let mounts = self.mounts.read();
        for (mount_point, _) in mounts.iter().rev() {
            if path.starts_with(mount_point) {
                if mount_point == Path::new("/") {
                    return Ok(canonical_in_mount);
                }
                let inner_rel = canonical_in_mount
                    .strip_prefix("/")
                    .unwrap_or(&canonical_in_mount);
                if inner_rel.as_os_str().is_empty() {
                    return Ok(mount_point.clone());
                }
                return Ok(mount_point.join(inner_rel));
            }
        }
        Ok(canonical_in_mount)
    }

    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let pair = self.resolve_two(src, dst)?;
        if pair.same {
            pair.src_fs.copy(&pair.src_rel, &pair.dst_rel)
        } else {
            let content = pair.src_fs.read_file(&pair.src_rel)?;
            pair.dst_fs.write_file(&pair.dst_rel, &content)
        }
    }

    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let pair = self.resolve_two(src, dst)?;
        if pair.same {
            pair.src_fs.rename(&pair.src_rel, &pair.dst_rel)
        } else {
            // Check if source is a directory — cross-mount directory rename
            // is not supported (would need recursive copy).
            if let Ok(m) = pair.src_fs.stat(&pair.src_rel)
                && m.node_type == NodeType::Directory
            {
                return Err(VfsError::IoError(
                    "rename of directories across mount boundaries is not supported".to_string(),
                ));
            }
            let content = pair.src_fs.read_file(&pair.src_rel)?;
            pair.dst_fs.write_file(&pair.dst_rel, &content)?;
            pair.src_fs.remove_file(&pair.src_rel)
        }
    }

    // TODO: MountableFs::glob does not yet honor GlobOptions (dotglob, nocaseglob, globstar).
    // Its glob_walk traversal needs refactoring to accept options.
    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let is_absolute = pattern.starts_with('/');
        let abs_pattern = if is_absolute {
            pattern.to_string()
        } else {
            let cwd_str = cwd.to_str().unwrap_or("/").trim_end_matches('/');
            format!("{cwd_str}/{pattern}")
        };

        let components: Vec<&str> = abs_pattern.split('/').filter(|s| !s.is_empty()).collect();
        let mut results = Vec::new();
        let max = 100_000;
        self.glob_walk(
            Path::new("/"),
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
        let mounts = self.mounts.read();
        let cloned_mounts: BTreeMap<PathBuf, Arc<dyn VirtualFs>> = mounts
            .iter()
            .map(|(path, fs)| (path.clone(), fs.deep_clone()))
            .collect();
        Arc::new(MountableFs {
            mounts: Arc::new(RwLock::new(cloned_mounts)),
        })
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

impl MountableFs {
    /// Returns true if `path` is a mount point or an ancestor of one.
    fn is_mount_point_or_ancestor(&self, path: &Path) -> bool {
        let mounts = self.mounts.read();
        if mounts.contains_key(path) {
            return true;
        }
        let prefix = if path == Path::new("/") {
            "/".to_string()
        } else {
            format!("{}/", path.to_string_lossy().trim_end_matches('/'))
        };
        mounts
            .keys()
            .any(|mp| mp.to_string_lossy().starts_with(&prefix))
    }

    /// Return the mount point that owns `path` (longest-prefix match).
    fn mount_point_for(&self, path: &Path) -> PathBuf {
        let mounts = self.mounts.read();
        for mount_point in mounts.keys().rev() {
            if path.starts_with(mount_point) {
                return mount_point.clone();
            }
        }
        PathBuf::from("/")
    }
}
