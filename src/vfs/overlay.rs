//! OverlayFs — copy-on-write filesystem backed by a real directory (lower)
//! and an in-memory write layer (upper).
//!
//! Reads resolve through: whiteouts → upper → lower.
//! Writes always go to the upper layer. The lower directory is never modified.
//! Deletions insert a "whiteout" entry so the file appears removed even though
//! it still exists on disk.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::platform::SystemTime;

use parking_lot::RwLock;

use super::{DirEntry, InMemoryFs, Metadata, NodeType, VirtualFs};
use crate::error::VfsError;
use crate::interpreter::pattern::glob_match;

const MAX_SYMLINK_DEPTH: u32 = 40;

/// A copy-on-write filesystem: reads from a real directory, writes to memory.
///
/// The lower layer (a real directory on disk) is treated as read-only.
/// All mutations go to the upper `InMemoryFs` layer. Deletions are tracked
/// via a whiteout set so deleted lower-layer entries appear as removed.
///
/// # Example
///
/// ```ignore
/// use rust_bash::{RustBashBuilder, OverlayFs};
/// use std::sync::Arc;
///
/// let overlay = OverlayFs::new("./my_project").unwrap();
/// let mut shell = RustBashBuilder::new()
///     .fs(Arc::new(overlay))
///     .cwd("/")
///     .build()
///     .unwrap();
///
/// let result = shell.exec("cat /src/main.rs").unwrap(); // reads from disk
/// shell.exec("echo new > /src/main.rs").unwrap();       // writes to memory only
/// ```
pub struct OverlayFs {
    lower: PathBuf,
    upper: InMemoryFs,
    whiteouts: Arc<RwLock<HashSet<PathBuf>>>,
}

/// Where a path resolved to during layer lookup.
enum LayerResult {
    Whiteout,
    Upper,
    Lower,
    NotFound,
}

impl OverlayFs {
    /// Create an overlay filesystem with `lower` as the read-only base.
    ///
    /// The lower directory must exist and be a directory. It is canonicalized
    /// on construction so symlinks in the lower path itself are resolved once.
    pub fn new(lower: impl Into<PathBuf>) -> std::io::Result<Self> {
        let lower = lower.into();
        if !lower.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("{} is not a directory", lower.display()),
            ));
        }
        let lower = lower.canonicalize()?;
        Ok(Self {
            lower,
            upper: InMemoryFs::new(),
            whiteouts: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    // ------------------------------------------------------------------
    // Whiteout helpers
    // ------------------------------------------------------------------

    /// Check if `path` or any ancestor is whiteout-ed.
    fn is_whiteout(&self, path: &Path) -> bool {
        let whiteouts = self.whiteouts.read();
        let mut current = path.to_path_buf();
        loop {
            if whiteouts.contains(&current) {
                return true;
            }
            if !current.pop() {
                return false;
            }
        }
    }

    /// Insert a whiteout for `path`.
    fn add_whiteout(&self, path: &Path) {
        self.whiteouts.write().insert(path.to_path_buf());
    }

    /// Remove a whiteout for exactly `path` (not ancestors).
    fn remove_whiteout(&self, path: &Path) {
        self.whiteouts.write().remove(path);
    }

    // ------------------------------------------------------------------
    // Layer entry checks (no symlink following)
    // ------------------------------------------------------------------

    /// Check if a node exists in the upper layer at `path` without
    /// following symlinks. This is critical because `InMemoryFs::exists`
    /// follows symlinks — a symlink whose target is only in lower would
    /// return false.
    fn upper_has_entry(&self, path: &Path) -> bool {
        self.upper.lstat(path).is_ok()
    }

    // ------------------------------------------------------------------
    // Layer resolution
    // ------------------------------------------------------------------

    /// Determine which layer `path` lives in (after normalization).
    fn resolve_layer(&self, path: &Path) -> LayerResult {
        if self.is_whiteout(path) {
            return LayerResult::Whiteout;
        }
        if self.upper_has_entry(path) {
            return LayerResult::Upper;
        }
        if self.lower_exists(path) {
            return LayerResult::Lower;
        }
        LayerResult::NotFound
    }

    // ------------------------------------------------------------------
    // Lower-layer reading helpers (3j)
    // ------------------------------------------------------------------

    /// Map a VFS absolute path to the corresponding real path under `lower`.
    fn lower_path(&self, vfs_path: &Path) -> PathBuf {
        let rel = vfs_path.strip_prefix("/").unwrap_or(vfs_path.as_ref());
        self.lower.join(rel)
    }

    /// Read a file from the lower layer.
    fn read_lower_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let real = self.lower_path(path);
        std::fs::read(&real).map_err(|e| map_io_error(e, path))
    }

    /// Get metadata for a path in the lower layer (follows symlinks).
    fn stat_lower(&self, path: &Path) -> Result<Metadata, VfsError> {
        let real = self.lower_path(path);
        let meta = std::fs::metadata(&real).map_err(|e| map_io_error(e, path))?;
        Ok(map_std_metadata(&meta))
    }

    /// Get metadata for a path in the lower layer (does NOT follow symlinks).
    fn lstat_lower(&self, path: &Path) -> Result<Metadata, VfsError> {
        let real = self.lower_path(path);
        let meta = std::fs::symlink_metadata(&real).map_err(|e| map_io_error(e, path))?;
        Ok(map_std_metadata(&meta))
    }

    /// List entries in a lower-layer directory.
    fn readdir_lower(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let real = self.lower_path(path);
        let entries = std::fs::read_dir(&real).map_err(|e| map_io_error(e, path))?;
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

    /// Read a symlink target from the lower layer.
    fn readlink_lower(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let real = self.lower_path(path);
        std::fs::read_link(&real).map_err(|e| map_io_error(e, path))
    }

    /// Check whether a path exists in the lower layer (symlink_metadata).
    fn lower_exists(&self, path: &Path) -> bool {
        let real = self.lower_path(path);
        real.symlink_metadata().is_ok()
    }

    // ------------------------------------------------------------------
    // Copy-up helpers (3c)
    // ------------------------------------------------------------------

    /// Ensure a file is present in the upper layer. If it only exists in the
    /// lower layer, copy its content and metadata up.
    fn copy_up_if_needed(&self, path: &Path) -> Result<(), VfsError> {
        if self.upper_has_entry(path) {
            return Ok(());
        }
        debug_assert!(
            !self.is_whiteout(path),
            "copy_up_if_needed called on whiteout-ed path"
        );
        // Ensure the parent directory exists in upper
        if let Some(parent) = path.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        let content = self.read_lower_file(path)?;
        let meta = self.stat_lower(path)?;
        self.upper.write_file(path, &content)?;
        self.upper.chmod(path, meta.mode)?;
        self.upper.utimes(path, meta.mtime)?;
        Ok(())
    }

    /// Ensure all components of `path` exist as directories in the upper layer,
    /// creating them if they only exist in the lower layer. Also clears any
    /// whiteouts on each component so previously-deleted paths become visible
    /// again.
    fn ensure_upper_dir_path(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let parts = path_components(&norm);
        let mut built = PathBuf::from("/");
        for name in parts {
            built.push(name);
            self.remove_whiteout(&built);
            if self.upper_has_entry(&built) {
                continue;
            }
            // Try to pick up metadata from lower; fallback to defaults
            let mode = if let Ok(m) = self.stat_lower(&built) {
                m.mode
            } else {
                0o755
            };
            self.upper.mkdir_p(&built)?;
            self.upper.chmod(&built, mode)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Recursive whiteout for remove_dir_all (3d)
    // ------------------------------------------------------------------

    /// Collect all visible paths under `dir` from both layers, then whiteout them.
    fn whiteout_recursive(&self, dir: &Path) -> Result<(), VfsError> {
        // Gather all visible children (merged from upper + lower, minus whiteouts)
        let entries = self.readdir_merged(dir)?;
        for entry in &entries {
            let child = dir.join(&entry.name);
            if entry.node_type == NodeType::Directory {
                self.whiteout_recursive(&child)?;
            }
            self.add_whiteout(&child);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Merged readdir helper (3e)
    // ------------------------------------------------------------------

    /// Merge directory listings from upper and lower, excluding whiteouts.
    fn readdir_merged(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let mut entries: std::collections::BTreeMap<String, DirEntry> =
            std::collections::BTreeMap::new();

        // Lower entries first
        if self.lower_exists(path)
            && let Ok(lower_entries) = self.readdir_lower(path)
        {
            for e in lower_entries {
                let child_path = path.join(&e.name);
                if !self.is_whiteout(&child_path) {
                    entries.insert(e.name.clone(), e);
                }
            }
        }

        // Upper entries override lower entries (dedup by name)
        if self.upper_has_entry(path)
            && let Ok(upper_entries) = self.upper.readdir(path)
        {
            for e in upper_entries {
                entries.insert(e.name.clone(), e);
            }
        }

        Ok(entries.into_values().collect())
    }

    // ------------------------------------------------------------------
    // Canonicalize helper (3g)
    // ------------------------------------------------------------------

    /// Step-by-step path resolution through both layers with symlink following.
    fn resolve_path(&self, path: &Path, follow_final: bool) -> Result<PathBuf, VfsError> {
        self.resolve_path_depth(path, follow_final, MAX_SYMLINK_DEPTH)
    }

    fn resolve_path_depth(
        &self,
        path: &Path,
        follow_final: bool,
        depth: u32,
    ) -> Result<PathBuf, VfsError> {
        if depth == 0 {
            return Err(VfsError::SymlinkLoop(path.to_path_buf()));
        }

        let norm = normalize(path)?;
        let parts = path_components(&norm);
        let mut resolved = PathBuf::from("/");

        for (i, name) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;
            let candidate = resolved.join(name);

            if self.is_whiteout(&candidate) {
                return Err(VfsError::NotFound(path.to_path_buf()));
            }

            // Check if this component is a symlink (upper takes precedence)
            let is_symlink_in_upper = self
                .upper
                .lstat(&candidate)
                .is_ok_and(|m| m.node_type == NodeType::Symlink);
            let is_symlink_in_lower = !is_symlink_in_upper
                && self
                    .lstat_lower(&candidate)
                    .is_ok_and(|m| m.node_type == NodeType::Symlink);

            if is_symlink_in_upper || is_symlink_in_lower {
                if is_last && !follow_final {
                    resolved = candidate;
                } else {
                    // Read the symlink target
                    let target = if is_symlink_in_upper {
                        self.upper.readlink(&candidate)?
                    } else {
                        self.readlink_lower(&candidate)?
                    };
                    // Resolve target (absolute or relative to parent)
                    let abs_target = if target.is_absolute() {
                        target
                    } else {
                        resolved.join(&target)
                    };
                    resolved = self.resolve_path_depth(&abs_target, true, depth - 1)?;
                }
            } else {
                resolved = candidate;
            }
        }
        Ok(resolved)
    }

    // ------------------------------------------------------------------
    // Glob helpers (3h)
    // ------------------------------------------------------------------

    /// Walk directories in both layers for glob matching.
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

        if pattern == "**" {
            // Zero directories — advance past **
            self.glob_walk(dir, rest, current_path.clone(), results, max);

            // One or more directories — recurse
            if let Ok(entries) = self.readdir_merged(dir) {
                for entry in entries {
                    if results.len() >= max {
                        return;
                    }
                    if entry.name.starts_with('.') {
                        continue;
                    }
                    let child_path = current_path.join(&entry.name);
                    let child_dir = dir.join(&entry.name);
                    if entry.node_type == NodeType::Directory
                        || entry.node_type == NodeType::Symlink
                    {
                        self.glob_walk(&child_dir, components, child_path, results, max);
                    }
                }
            }
        } else if let Ok(entries) = self.readdir_merged(dir) {
            for entry in entries {
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
}

// ---------------------------------------------------------------------------
// VirtualFs implementation
// ---------------------------------------------------------------------------

impl VirtualFs for OverlayFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self.upper.read_file(&resolved),
            LayerResult::Lower => self.read_lower_file(&resolved),
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        // Ensure parent directories exist in upper
        if let Some(parent) = norm.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        // Remove whiteout if any (we're creating/overwriting the file)
        self.remove_whiteout(&norm);
        self.upper.write_file(&norm, content)
    }

    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self.upper.append_file(&resolved, content),
            LayerResult::Lower => {
                self.copy_up_if_needed(&resolved)?;
                self.upper.append_file(&resolved, content)
            }
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        let in_upper = self.upper_has_entry(&norm);
        let in_lower = self.lower_exists(&norm);
        if !in_upper && !in_lower {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        // Verify it's not a directory
        if in_upper {
            if let Ok(m) = self.upper.lstat(&norm)
                && m.node_type == NodeType::Directory
            {
                return Err(VfsError::IsADirectory(path.to_path_buf()));
            }
        } else if let Ok(m) = self.lstat_lower(&norm)
            && m.node_type == NodeType::Directory
        {
            return Err(VfsError::IsADirectory(path.to_path_buf()));
        }
        if in_upper {
            self.upper.remove_file(&norm)?;
        }
        self.add_whiteout(&norm);
        Ok(())
    }

    fn mkdir(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        if self.is_whiteout(&norm) {
            // Path was deleted — we can re-create it
            self.remove_whiteout(&norm);
        } else {
            // Check if it already exists in either layer
            let in_upper = self.upper_has_entry(&norm);
            let in_lower = self.lower_exists(&norm);
            if in_upper || in_lower {
                return Err(VfsError::AlreadyExists(path.to_path_buf()));
            }
        }
        // Ensure parents exist in upper
        if let Some(parent) = norm.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        // Check if it now exists in upper (ensure_upper_dir_path might have created it)
        if self.upper_has_entry(&norm) {
            return Err(VfsError::AlreadyExists(path.to_path_buf()));
        }
        self.upper.mkdir(&norm)
    }

    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let parts = path_components(&norm);
        if parts.is_empty() {
            return Ok(());
        }

        let mut built = PathBuf::from("/");
        for name in parts {
            built.push(name);

            // Skip if the whiteout was for this exact path but we want to recreate
            if self.is_whiteout(&built) {
                self.remove_whiteout(&built);
                // Need to create this component in upper
                self.ensure_single_dir_in_upper(&built)?;
                continue;
            }

            // If it exists in upper, verify it's a directory
            if self.upper_has_entry(&built) {
                let m = self.upper.lstat(&built)?;
                if m.node_type != NodeType::Directory {
                    return Err(VfsError::NotADirectory(path.to_path_buf()));
                }
                continue;
            }

            // If it exists in lower, verify it's a directory — no need to copy up
            if self.lower_exists(&built) {
                let m = self.lstat_lower(&built)?;
                if m.node_type != NodeType::Directory {
                    return Err(VfsError::NotADirectory(path.to_path_buf()));
                }
                continue;
            }

            // Doesn't exist anywhere — create in upper
            self.ensure_single_dir_in_upper(&built)?;
        }
        Ok(())
    }

    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let norm = normalize(path)?;
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }

        let in_upper = self.upper_has_entry(&norm);
        let in_lower = self.lower_exists(&norm);

        if !in_upper && !in_lower {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }

        // Validate directory through overlay (handles symlinks across layers)
        let m = self.stat(&norm)?;
        if m.node_type != NodeType::Directory {
            return Err(VfsError::NotADirectory(path.to_path_buf()));
        }

        self.readdir_merged(&norm)
    }

    fn remove_dir(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }

        // Check that it exists and is a directory
        let m = self.lstat_overlay(&norm, path)?;
        if m.node_type != NodeType::Directory {
            return Err(VfsError::NotADirectory(path.to_path_buf()));
        }

        // Check that it's empty (merged view)
        let entries = self.readdir_merged(&norm)?;
        if !entries.is_empty() {
            return Err(VfsError::DirectoryNotEmpty(path.to_path_buf()));
        }

        // Remove from upper if present
        if self.upper_has_entry(&norm) {
            self.upper.remove_dir(&norm).ok();
        }
        self.add_whiteout(&norm);
        Ok(())
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }

        // Check that it exists
        let m = self.lstat_overlay(&norm, path)?;
        if m.node_type != NodeType::Directory {
            return Err(VfsError::NotADirectory(path.to_path_buf()));
        }

        // Recursively whiteout all children
        self.whiteout_recursive(&norm)?;

        // Remove the directory subtree from upper if present
        if self.upper_has_entry(&norm) {
            self.upper.remove_dir_all(&norm).ok();
        }

        // Whiteout the directory itself
        self.add_whiteout(&norm);
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        let norm = match normalize(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        if self.is_whiteout(&norm) {
            return false;
        }
        // Check if entry exists in either layer without following symlinks first
        if !self.upper_has_entry(&norm) && !self.lower_exists(&norm) {
            return false;
        }
        // If it's a symlink, verify the target exists through the overlay
        let meta = match self.lstat_overlay(&norm, &norm) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if meta.node_type == NodeType::Symlink {
            return self.stat(&norm).is_ok();
        }
        true
    }

    fn stat(&self, path: &Path) -> Result<Metadata, VfsError> {
        let norm = normalize(path)?;
        // Try to resolve symlinks
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self.upper.stat(&resolved),
            LayerResult::Lower => self.stat_lower(&resolved),
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError> {
        let norm = normalize(path)?;
        self.lstat_overlay(&norm, path)
    }

    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self.upper.chmod(&resolved, mode),
            LayerResult::Lower => {
                // Need to copy up the file/dir to apply chmod
                let meta = self.lstat_lower(&resolved)?;
                match meta.node_type {
                    NodeType::File => {
                        self.copy_up_if_needed(&resolved)?;
                        self.upper.chmod(&resolved, mode)
                    }
                    NodeType::Directory => {
                        self.ensure_upper_dir_path(&resolved)?;
                        self.upper.chmod(&resolved, mode)
                    }
                    NodeType::Symlink => {
                        Err(VfsError::IoError("cannot chmod a symlink directly".into()))
                    }
                }
            }
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self.upper.utimes(&resolved, mtime),
            LayerResult::Lower => {
                let meta = self.lstat_lower(&resolved)?;
                match meta.node_type {
                    NodeType::File => {
                        self.copy_up_if_needed(&resolved)?;
                        self.upper.utimes(&resolved, mtime)
                    }
                    NodeType::Directory => {
                        self.ensure_upper_dir_path(&resolved)?;
                        self.upper.utimes(&resolved, mtime)
                    }
                    NodeType::Symlink => {
                        self.copy_up_symlink_if_needed(&resolved)?;
                        self.upper.utimes(&resolved, mtime)
                    }
                }
            }
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError> {
        let norm_link = normalize(link)?;
        // Ensure parent in upper
        if let Some(parent) = norm_link.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        // If there's a whiteout, remove it to allow re-creation
        self.remove_whiteout(&norm_link);
        self.upper.symlink(target, &norm_link)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let norm_src = normalize(src)?;
        let norm_dst = normalize(dst)?;
        // Read source from whichever layer has it
        let content = self.read_file(&norm_src)?;
        let meta = self.stat(&norm_src)?;
        // Ensure parent for dst in upper
        if let Some(parent) = norm_dst.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        self.remove_whiteout(&norm_dst);
        self.upper.write_file(&norm_dst, &content)?;
        self.upper.chmod(&norm_dst, meta.mode)?;
        self.upper.utimes(&norm_dst, meta.mtime)?;
        Ok(())
    }

    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let norm = normalize(path)?;
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        if self.upper_has_entry(&norm) {
            return self.upper.readlink(&norm);
        }
        if self.lower_exists(&norm) {
            return self.readlink_lower(&norm);
        }
        Err(VfsError::NotFound(path.to_path_buf()))
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        // Make sure the resolved path actually exists
        if self.is_whiteout(&resolved) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        if !self.upper_has_entry(&resolved) && !self.lower_exists(&resolved) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        Ok(resolved)
    }

    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let norm_src = normalize(src)?;
        let norm_dst = normalize(dst)?;
        // Read from resolved layer
        let content = self.read_file(&norm_src)?;
        let meta = self.stat(&norm_src)?;
        // Write to upper
        self.write_file(&norm_dst, &content)?;
        self.chmod(&norm_dst, meta.mode)?;
        Ok(())
    }

    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let norm_src = normalize(src)?;
        let norm_dst = normalize(dst)?;

        if self.is_whiteout(&norm_src) {
            return Err(VfsError::NotFound(src.to_path_buf()));
        }

        // Copy-up src if only in lower
        let in_upper = self.upper_has_entry(&norm_src);
        let in_lower = self.lower_exists(&norm_src);
        if !in_upper && !in_lower {
            return Err(VfsError::NotFound(src.to_path_buf()));
        }

        // Read content and metadata
        let meta = self.lstat_overlay(&norm_src, src)?;
        match meta.node_type {
            NodeType::File => {
                let content = self.read_file(&norm_src)?;
                // Ensure dst parent exists in upper
                if let Some(parent) = norm_dst.parent()
                    && parent != Path::new("/")
                {
                    self.ensure_upper_dir_path(parent)?;
                }
                self.remove_whiteout(&norm_dst);
                self.upper.write_file(&norm_dst, &content)?;
                self.upper.chmod(&norm_dst, meta.mode)?;
            }
            NodeType::Symlink => {
                let target = self.readlink(&norm_src)?;
                if let Some(parent) = norm_dst.parent()
                    && parent != Path::new("/")
                {
                    self.ensure_upper_dir_path(parent)?;
                }
                self.remove_whiteout(&norm_dst);
                self.upper.symlink(&target, &norm_dst)?;
            }
            NodeType::Directory => {
                // For directory rename, copy all children recursively
                if let Some(parent) = norm_dst.parent()
                    && parent != Path::new("/")
                {
                    self.ensure_upper_dir_path(parent)?;
                }
                self.remove_whiteout(&norm_dst);
                self.upper.mkdir_p(&norm_dst)?;
                let entries = self.readdir_merged(&norm_src)?;
                for entry in entries {
                    let child_src = norm_src.join(&entry.name);
                    let child_dst = norm_dst.join(&entry.name);
                    self.rename(&child_src, &child_dst)?;
                }
            }
        }

        // Remove from upper if it was there
        if in_upper {
            match meta.node_type {
                NodeType::Directory => {
                    self.upper.remove_dir_all(&norm_src).ok();
                }
                _ => {
                    self.upper.remove_file(&norm_src).ok();
                }
            }
        }
        // Whiteout the source to hide from lower
        self.add_whiteout(&norm_src);
        Ok(())
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
        Arc::new(OverlayFs {
            lower: self.lower.clone(),
            upper: self.upper.deep_clone(),
            whiteouts: Arc::new(RwLock::new(self.whiteouts.read().clone())),
        })
    }
}

// ---------------------------------------------------------------------------
// Private OverlayFs helpers
// ---------------------------------------------------------------------------

impl OverlayFs {
    /// lstat through the overlay (no symlink following on final component).
    fn lstat_overlay(&self, norm: &Path, orig: &Path) -> Result<Metadata, VfsError> {
        if self.is_whiteout(norm) {
            return Err(VfsError::NotFound(orig.to_path_buf()));
        }
        if self.upper_has_entry(norm) {
            return self.upper.lstat(norm);
        }
        if self.lower_exists(norm) {
            return self.lstat_lower(norm);
        }
        Err(VfsError::NotFound(orig.to_path_buf()))
    }

    /// Ensure a single directory exists in the upper layer at `path`.
    fn ensure_single_dir_in_upper(&self, path: &Path) -> Result<(), VfsError> {
        if self.upper_has_entry(path) {
            return Ok(());
        }
        // Ensure parent first
        if let Some(parent) = path.parent()
            && parent != Path::new("/")
            && !self.upper_has_entry(parent)
        {
            self.ensure_single_dir_in_upper(parent)?;
        }
        self.upper.mkdir(path)
    }

    /// Copy-up a symlink from lower to upper.
    fn copy_up_symlink_if_needed(&self, path: &Path) -> Result<(), VfsError> {
        if self.upper_has_entry(path) {
            return Ok(());
        }
        if let Some(parent) = path.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        let target = self.readlink_lower(path)?;
        self.upper.symlink(&target, path)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Normalize an absolute path: resolve `.` and `..`.
fn normalize(path: &Path) -> Result<PathBuf, VfsError> {
    let s = path.to_str().unwrap_or("");
    if s.is_empty() {
        return Err(VfsError::InvalidPath("empty path".into()));
    }
    if !path.is_absolute() {
        return Err(VfsError::InvalidPath(format!(
            "path must be absolute: {}",
            path.display()
        )));
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::RootDir | Component::Prefix(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(seg) => {
                if let Some(s) = seg.to_str() {
                    parts.push(s.to_owned());
                } else {
                    return Err(VfsError::InvalidPath(format!(
                        "non-UTF-8 component in: {}",
                        path.display()
                    )));
                }
            }
        }
    }
    let mut result = PathBuf::from("/");
    for p in &parts {
        result.push(p);
    }
    Ok(result)
}

/// Split a normalized absolute path into component names.
fn path_components(path: &Path) -> Vec<&str> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect()
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
fn map_std_metadata(meta: &std::fs::Metadata) -> Metadata {
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
