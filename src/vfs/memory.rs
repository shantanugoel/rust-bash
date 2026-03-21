use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::platform::SystemTime;

use parking_lot::RwLock;

use super::{DirEntry, FsNode, GlobOptions, Metadata, NodeType, VirtualFs};
use crate::error::VfsError;

const MAX_SYMLINK_DEPTH: u32 = 40;

/// A fully in-memory filesystem implementation.
///
/// Thread-safe via `Arc<RwLock<...>>` — all `VirtualFs` methods take `&self`.
/// Cloning is cheap (Arc increment) which is useful for subshell state cloning.
#[derive(Debug, Clone)]
pub struct InMemoryFs {
    root: Arc<RwLock<FsNode>>,
}

impl Default for InMemoryFs {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryFs {
    pub fn new() -> Self {
        Self {
            root: Arc::new(RwLock::new(FsNode::Directory {
                children: BTreeMap::new(),
                mode: 0o755,
                mtime: SystemTime::now(),
            })),
        }
    }

    /// Create a completely independent copy of this filesystem.
    ///
    /// Unlike `Clone` (which shares data via `Arc`), this recursively clones
    /// the entire `FsNode` tree so the copy and original are fully independent.
    /// Used for subshell isolation: `( cmds )`.
    pub fn deep_clone(&self) -> Self {
        let tree = self.root.read();
        Self {
            root: Arc::new(RwLock::new(tree.clone())),
        }
    }
}

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

/// Normalize an absolute path: resolve `.` and `..`, strip trailing slashes,
/// reject empty paths.
fn normalize(path: &Path) -> Result<PathBuf, VfsError> {
    let s = path.to_str().unwrap_or("");
    if s.is_empty() {
        return Err(VfsError::InvalidPath("empty path".into()));
    }
    if !super::vfs_path_is_absolute(path) {
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

/// Split a normalized absolute path into its component names (excluding root).
fn components(path: &Path) -> Vec<&str> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Internal node navigation helpers
// ---------------------------------------------------------------------------

impl InMemoryFs {
    /// Read-lock the tree, navigate to a node (resolving symlinks), apply `f`.
    fn with_node<F, T>(&self, path: &Path, f: F) -> Result<T, VfsError>
    where
        F: FnOnce(&FsNode) -> Result<T, VfsError>,
    {
        let norm = normalize(path)?;
        let tree = self.root.read();
        let node = navigate(&tree, &norm, true, MAX_SYMLINK_DEPTH, &tree)?;
        f(node)
    }

    /// Read-lock the tree, navigate to a node **without** resolving the final
    /// symlink component, apply `f`.
    fn with_node_no_follow<F, T>(&self, path: &Path, f: F) -> Result<T, VfsError>
    where
        F: FnOnce(&FsNode) -> Result<T, VfsError>,
    {
        let norm = normalize(path)?;
        let tree = self.root.read();
        let node = navigate(&tree, &norm, false, MAX_SYMLINK_DEPTH, &tree)?;
        f(node)
    }

    /// Write-lock, navigate to the **parent** of `path`, call `f(parent, child_name)`.
    fn with_parent_mut<F, T>(&self, path: &Path, f: F) -> Result<T, VfsError>
    where
        F: FnOnce(&mut FsNode, &str) -> Result<T, VfsError>,
    {
        let norm = normalize(path)?;
        let parts = components(&norm);
        if parts.is_empty() {
            return Err(VfsError::InvalidPath(
                "cannot operate on root itself".into(),
            ));
        }
        let child_name = parts.last().unwrap();
        let parent_path: PathBuf = if parts.len() == 1 {
            PathBuf::from("/")
        } else {
            let mut p = PathBuf::from("/");
            for seg in &parts[..parts.len() - 1] {
                p.push(seg);
            }
            p
        };

        let mut tree = self.root.write();
        let parent = navigate_mut(&mut tree, &parent_path, true, MAX_SYMLINK_DEPTH)?;
        f(parent, child_name)
    }

    /// Write-lock, navigate to a node (resolving symlinks), apply `f`.
    fn with_node_mut<F, T>(&self, path: &Path, f: F) -> Result<T, VfsError>
    where
        F: FnOnce(&mut FsNode) -> Result<T, VfsError>,
    {
        let norm = normalize(path)?;
        let mut tree = self.root.write();
        let node = navigate_mut(&mut tree, &norm, true, MAX_SYMLINK_DEPTH)?;
        f(node)
    }

    /// Resolve symlinks in a path, returning the canonical absolute path.
    fn resolve_path(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let norm = normalize(path)?;
        let tree = self.root.read();
        resolve_canonical(&tree, &norm, MAX_SYMLINK_DEPTH, &tree)
    }
}

// ---------------------------------------------------------------------------
// Tree navigation (works on borrowed FsNode trees)
// ---------------------------------------------------------------------------

/// Navigate the tree from `root` to `path`, optionally following symlinks on
/// the final component. Returns a reference to the target node.
fn navigate<'a>(
    root: &'a FsNode,
    path: &Path,
    follow_final: bool,
    depth: u32,
    tree_root: &'a FsNode,
) -> Result<&'a FsNode, VfsError> {
    if depth == 0 {
        return Err(VfsError::SymlinkLoop(path.to_path_buf()));
    }

    let parts = components(path);
    if parts.is_empty() {
        return Ok(root);
    }

    let mut current = root;
    for (i, name) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        // Resolve current if it's a symlink (intermediate components always resolved)
        current = resolve_if_symlink(current, path, depth, tree_root)?;

        match current {
            FsNode::Directory { children, .. } => {
                let child = children
                    .get(*name)
                    .ok_or_else(|| VfsError::NotFound(path.to_path_buf()))?;
                if is_last && !follow_final {
                    current = child;
                } else {
                    current = resolve_if_symlink(child, path, depth - 1, tree_root)?;
                }
            }
            _ => return Err(VfsError::NotADirectory(path.to_path_buf())),
        }
    }
    Ok(current)
}

/// Resolve a node if it is a symlink, following chains up to `depth`.
fn resolve_if_symlink<'a>(
    node: &'a FsNode,
    original_path: &Path,
    depth: u32,
    tree_root: &'a FsNode,
) -> Result<&'a FsNode, VfsError> {
    if depth == 0 {
        return Err(VfsError::SymlinkLoop(original_path.to_path_buf()));
    }
    match node {
        FsNode::Symlink { target, .. } => {
            let target_norm = normalize(target)?;
            navigate(tree_root, &target_norm, true, depth - 1, tree_root)
        }
        other => Ok(other),
    }
}

/// Mutable navigation. Symlinks on intermediate components are resolved by
/// restarting from root (which requires dropping and re-borrowing).
/// For simplicity, we first compute the canonical path, then navigate directly.
fn navigate_mut<'a>(
    root: &'a mut FsNode,
    path: &Path,
    follow_final: bool,
    depth: u32,
) -> Result<&'a mut FsNode, VfsError> {
    if depth == 0 {
        return Err(VfsError::SymlinkLoop(path.to_path_buf()));
    }

    let parts = components(path);
    if parts.is_empty() {
        return Ok(root);
    }

    // We need to handle symlinks during mutable traversal.
    // Strategy: traverse step by step; if we hit a symlink, resolve it to get the
    // canonical path of that prefix, then restart navigation from root with the
    // resolved prefix + remaining components.
    let canonical = resolve_canonical_from_root(root, path, follow_final, depth)?;
    let canon_parts = components(&canonical);

    let mut current = root as &mut FsNode;
    for name in &canon_parts {
        match current {
            FsNode::Directory { children, .. } => {
                current = children
                    .get_mut(*name)
                    .ok_or_else(|| VfsError::NotFound(path.to_path_buf()))?;
            }
            _ => return Err(VfsError::NotADirectory(path.to_path_buf())),
        }
    }
    Ok(current)
}

/// Resolve a path to its canonical form by walking the tree and resolving symlinks.
fn resolve_canonical(
    root: &FsNode,
    path: &Path,
    depth: u32,
    tree_root: &FsNode,
) -> Result<PathBuf, VfsError> {
    if depth == 0 {
        return Err(VfsError::SymlinkLoop(path.to_path_buf()));
    }

    let parts = components(path);
    let mut resolved = PathBuf::from("/");
    let mut current = root;

    for name in &parts {
        // current must be a directory (resolve symlinks)
        current = resolve_if_symlink(current, path, depth, tree_root)?;
        match current {
            FsNode::Directory { children, .. } => {
                let child = children
                    .get(*name)
                    .ok_or_else(|| VfsError::NotFound(path.to_path_buf()))?;
                match child {
                    FsNode::Symlink { target, .. } => {
                        let target_norm = normalize(target)?;
                        // Resolve the symlink target recursively
                        resolved =
                            resolve_canonical(tree_root, &target_norm, depth - 1, tree_root)?;
                        current = navigate(tree_root, &resolved, true, depth - 1, tree_root)?;
                    }
                    _ => {
                        resolved.push(name);
                        current = child;
                    }
                }
            }
            _ => return Err(VfsError::NotADirectory(path.to_path_buf())),
        }
    }
    Ok(resolved)
}

/// Same as `resolve_canonical` but works from a mutable root reference
/// (read-only traversal to compute the path).
fn resolve_canonical_from_root(
    root: &FsNode,
    path: &Path,
    follow_final: bool,
    depth: u32,
) -> Result<PathBuf, VfsError> {
    if depth == 0 {
        return Err(VfsError::SymlinkLoop(path.to_path_buf()));
    }

    let parts = components(path);
    let mut resolved = PathBuf::from("/");
    let mut current: &FsNode = root;

    for (i, name) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        // current must be a directory (resolve symlinks)
        current = resolve_if_symlink_from_root(current, path, depth, root)?;
        match current {
            FsNode::Directory { children, .. } => {
                let child = children
                    .get(*name)
                    .ok_or_else(|| VfsError::NotFound(path.to_path_buf()))?;
                if is_last && !follow_final {
                    resolved.push(name);
                    break;
                }
                match child {
                    FsNode::Symlink { target, .. } => {
                        let target_norm = normalize(target)?;
                        resolved =
                            resolve_canonical_from_root(root, &target_norm, true, depth - 1)?;
                        current = navigate_readonly(root, &resolved, true, depth - 1, root)?;
                    }
                    _ => {
                        resolved.push(name);
                        current = child;
                    }
                }
            }
            _ => return Err(VfsError::NotADirectory(path.to_path_buf())),
        }
    }
    Ok(resolved)
}

fn resolve_if_symlink_from_root<'a>(
    node: &'a FsNode,
    original_path: &Path,
    depth: u32,
    root: &'a FsNode,
) -> Result<&'a FsNode, VfsError> {
    if depth == 0 {
        return Err(VfsError::SymlinkLoop(original_path.to_path_buf()));
    }
    match node {
        FsNode::Symlink { target, .. } => {
            let target_norm = normalize(target)?;
            navigate_readonly(root, &target_norm, true, depth - 1, root)
        }
        other => Ok(other),
    }
}

fn navigate_readonly<'a>(
    root: &'a FsNode,
    path: &Path,
    follow_final: bool,
    depth: u32,
    tree_root: &'a FsNode,
) -> Result<&'a FsNode, VfsError> {
    navigate(root, path, follow_final, depth, tree_root)
}

/// Navigate a mutable tree by component names (no symlink resolution).
/// Returns `None` if any component is missing or not a directory.
fn navigate_to_mut<'a>(node: &'a mut FsNode, parts: &[&str]) -> Option<&'a mut FsNode> {
    let mut current = node;
    for name in parts {
        match current {
            FsNode::Directory { children, .. } => {
                current = children.get_mut(*name)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

// ---------------------------------------------------------------------------
// VirtualFs implementation
// ---------------------------------------------------------------------------

impl VirtualFs for InMemoryFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.with_node(path, |node| match node {
            FsNode::File { content, .. } => Ok(content.clone()),
            FsNode::Directory { .. } => Err(VfsError::IsADirectory(path.to_path_buf())),
            FsNode::Symlink { .. } => Err(VfsError::IoError(
                "unexpected symlink after resolution".into(),
            )),
        })
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let norm = normalize(path)?;

        // Try to overwrite an existing file first
        {
            let mut tree = self.root.write();
            let canon_result = resolve_canonical_from_root(&tree, &norm, true, MAX_SYMLINK_DEPTH);
            if let Ok(canon) = canon_result {
                let canon_parts = components(&canon);
                let node = navigate_to_mut(&mut tree, &canon_parts);
                if let Some(node) = node {
                    match node {
                        FsNode::File {
                            content: c,
                            mtime: m,
                            ..
                        } => {
                            *c = content.to_vec();
                            *m = SystemTime::now();
                            return Ok(());
                        }
                        FsNode::Directory { .. } => {
                            return Err(VfsError::IsADirectory(path.to_path_buf()));
                        }
                        FsNode::Symlink { .. } => {}
                    }
                }
            }
        }

        // File doesn't exist — create it in the parent directory
        self.with_parent_mut(path, |parent, child_name| match parent {
            FsNode::Directory { children, .. } => {
                children.insert(
                    child_name.to_string(),
                    FsNode::File {
                        content: content.to_vec(),
                        mode: 0o644,
                        mtime: SystemTime::now(),
                    },
                );
                Ok(())
            }
            _ => Err(VfsError::NotADirectory(path.to_path_buf())),
        })
    }

    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        self.with_node_mut(path, |node| match node {
            FsNode::File {
                content: c,
                mtime: m,
                ..
            } => {
                c.extend_from_slice(content);
                *m = SystemTime::now();
                Ok(())
            }
            FsNode::Directory { .. } => Err(VfsError::IsADirectory(path.to_path_buf())),
            FsNode::Symlink { .. } => Err(VfsError::IoError(
                "unexpected symlink after resolution".into(),
            )),
        })
    }

    fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        // Resolve the path to find the actual location of the file
        let norm = normalize(path)?;

        // Check if the final component is a symlink — remove_file should remove the link, not the target
        self.with_parent_mut(path, |parent, child_name| match parent {
            FsNode::Directory { children, .. } => match children.get(child_name) {
                Some(FsNode::File { .. } | FsNode::Symlink { .. }) => {
                    children.remove(child_name);
                    Ok(())
                }
                Some(FsNode::Directory { .. }) => Err(VfsError::IsADirectory(norm.clone())),
                None => Err(VfsError::NotFound(norm.clone())),
            },
            _ => Err(VfsError::NotADirectory(norm.clone())),
        })
    }

    fn mkdir(&self, path: &Path) -> Result<(), VfsError> {
        self.with_parent_mut(path, |parent, child_name| match parent {
            FsNode::Directory { children, .. } => {
                if children.contains_key(child_name) {
                    return Err(VfsError::AlreadyExists(path.to_path_buf()));
                }
                children.insert(
                    child_name.to_string(),
                    FsNode::Directory {
                        children: BTreeMap::new(),
                        mode: 0o755,
                        mtime: SystemTime::now(),
                    },
                );
                Ok(())
            }
            _ => Err(VfsError::NotADirectory(path.to_path_buf())),
        })
    }

    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let parts = components(&norm);
        if parts.is_empty() {
            return Ok(()); // root already exists
        }

        let mut tree = self.root.write();
        let mut current: &mut FsNode = &mut tree;
        for name in &parts {
            match current {
                FsNode::Directory { children, .. } => {
                    current =
                        children
                            .entry((*name).to_string())
                            .or_insert_with(|| FsNode::Directory {
                                children: BTreeMap::new(),
                                mode: 0o755,
                                mtime: SystemTime::now(),
                            });
                    // If it already exists as a dir, that's fine. If it's a file, error.
                    match current {
                        FsNode::Directory { .. } => {}
                        FsNode::File { .. } => {
                            return Err(VfsError::NotADirectory(path.to_path_buf()));
                        }
                        FsNode::Symlink { .. } => {
                            // For simplicity, don't follow symlinks in mkdir_p path creation
                            return Err(VfsError::NotADirectory(path.to_path_buf()));
                        }
                    }
                }
                _ => return Err(VfsError::NotADirectory(path.to_path_buf())),
            }
        }
        Ok(())
    }

    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        self.with_node(path, |node| match node {
            FsNode::Directory { children, .. } => {
                let entries = children
                    .iter()
                    .map(|(name, child)| DirEntry {
                        name: name.clone(),
                        node_type: match child {
                            FsNode::File { .. } => NodeType::File,
                            FsNode::Directory { .. } => NodeType::Directory,
                            FsNode::Symlink { .. } => NodeType::Symlink,
                        },
                    })
                    .collect();
                Ok(entries)
            }
            _ => Err(VfsError::NotADirectory(path.to_path_buf())),
        })
    }

    fn remove_dir(&self, path: &Path) -> Result<(), VfsError> {
        self.with_parent_mut(path, |parent, child_name| match parent {
            FsNode::Directory { children, .. } => match children.get(child_name) {
                Some(FsNode::Directory { children: ch, .. }) => {
                    if ch.is_empty() {
                        children.remove(child_name);
                        Ok(())
                    } else {
                        Err(VfsError::DirectoryNotEmpty(path.to_path_buf()))
                    }
                }
                Some(FsNode::File { .. }) => Err(VfsError::NotADirectory(path.to_path_buf())),
                Some(FsNode::Symlink { .. }) => Err(VfsError::NotADirectory(path.to_path_buf())),
                None => Err(VfsError::NotFound(path.to_path_buf())),
            },
            _ => Err(VfsError::NotADirectory(path.to_path_buf())),
        })
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        self.with_parent_mut(path, |parent, child_name| match parent {
            FsNode::Directory { children, .. } => match children.get(child_name) {
                Some(FsNode::Directory { .. }) => {
                    children.remove(child_name);
                    Ok(())
                }
                Some(FsNode::File { .. }) => Err(VfsError::NotADirectory(path.to_path_buf())),
                Some(FsNode::Symlink { .. }) => Err(VfsError::NotADirectory(path.to_path_buf())),
                None => Err(VfsError::NotFound(path.to_path_buf())),
            },
            _ => Err(VfsError::NotADirectory(path.to_path_buf())),
        })
    }

    fn exists(&self, path: &Path) -> bool {
        let norm = match normalize(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        let tree = self.root.read();
        navigate(&tree, &norm, true, MAX_SYMLINK_DEPTH, &tree).is_ok()
    }

    fn stat(&self, path: &Path) -> Result<Metadata, VfsError> {
        self.with_node(path, |node| Ok(node_metadata(node)))
    }

    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError> {
        self.with_node_no_follow(path, |node| Ok(node_metadata(node)))
    }

    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError> {
        self.with_node_mut(path, |node| {
            match node {
                FsNode::File { mode: m, .. } | FsNode::Directory { mode: m, .. } => {
                    *m = mode;
                }
                FsNode::Symlink { .. } => {
                    // chmod on a symlink (after resolution) shouldn't hit this
                    return Err(VfsError::IoError("cannot chmod a symlink directly".into()));
                }
            }
            Ok(())
        })
    }

    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError> {
        self.with_node_mut(path, |node| {
            match node {
                FsNode::File { mtime: m, .. }
                | FsNode::Directory { mtime: m, .. }
                | FsNode::Symlink { mtime: m, .. } => {
                    *m = mtime;
                }
            }
            Ok(())
        })
    }

    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError> {
        self.with_parent_mut(link, |parent, child_name| match parent {
            FsNode::Directory { children, .. } => {
                if children.contains_key(child_name) {
                    return Err(VfsError::AlreadyExists(link.to_path_buf()));
                }
                children.insert(
                    child_name.to_string(),
                    FsNode::Symlink {
                        target: target.to_path_buf(),
                        mtime: SystemTime::now(),
                    },
                );
                Ok(())
            }
            _ => Err(VfsError::NotADirectory(link.to_path_buf())),
        })
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        // Hard links in an in-memory FS: clone the node data.
        let content = self.read_file(src)?;
        let meta = self.stat(src)?;
        self.with_parent_mut(dst, |parent, child_name| match parent {
            FsNode::Directory { children, .. } => {
                if children.contains_key(child_name) {
                    return Err(VfsError::AlreadyExists(dst.to_path_buf()));
                }
                children.insert(
                    child_name.to_string(),
                    FsNode::File {
                        content: content.clone(),
                        mode: meta.mode,
                        mtime: meta.mtime,
                    },
                );
                Ok(())
            }
            _ => Err(VfsError::NotADirectory(dst.to_path_buf())),
        })
    }

    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError> {
        self.with_node_no_follow(path, |node| match node {
            FsNode::Symlink { target, .. } => Ok(target.clone()),
            _ => Err(VfsError::InvalidPath(format!(
                "not a symlink: {}",
                path.display()
            ))),
        })
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        self.resolve_path(path)
    }

    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let content = self.read_file(src)?;
        let meta = self.stat(src)?;
        self.write_file(dst, &content)?;
        self.chmod(dst, meta.mode)?;
        Ok(())
    }

    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let src_norm = normalize(src)?;
        let dst_norm = normalize(dst)?;

        let src_parts = components(&src_norm);
        let dst_parts = components(&dst_norm);
        if src_parts.is_empty() || dst_parts.is_empty() {
            return Err(VfsError::InvalidPath("cannot rename root".into()));
        }

        let mut tree = self.root.write();

        // Extract the source node from the tree
        let node = {
            let src_parent_parts = &src_parts[..src_parts.len() - 1];
            let src_child = src_parts.last().unwrap();

            let mut parent: &mut FsNode = &mut tree;
            for name in src_parent_parts {
                match parent {
                    FsNode::Directory { children, .. } => {
                        parent = children
                            .get_mut(*name)
                            .ok_or_else(|| VfsError::NotFound(src.to_path_buf()))?;
                    }
                    _ => return Err(VfsError::NotADirectory(src.to_path_buf())),
                }
            }
            match parent {
                FsNode::Directory { children, .. } => children
                    .remove(*src_child)
                    .ok_or_else(|| VfsError::NotFound(src.to_path_buf()))?,
                _ => return Err(VfsError::NotADirectory(src.to_path_buf())),
            }
        };

        // Insert at destination
        {
            let dst_parent_parts = &dst_parts[..dst_parts.len() - 1];
            let dst_child = dst_parts.last().unwrap();

            let mut parent: &mut FsNode = &mut tree;
            for name in dst_parent_parts {
                match parent {
                    FsNode::Directory { children, .. } => {
                        parent = children
                            .get_mut(*name)
                            .ok_or_else(|| VfsError::NotFound(dst.to_path_buf()))?;
                    }
                    _ => return Err(VfsError::NotADirectory(dst.to_path_buf())),
                }
            }
            match parent {
                FsNode::Directory { children, .. } => {
                    children.insert((*dst_child).to_string(), node);
                }
                _ => return Err(VfsError::NotADirectory(dst.to_path_buf())),
            }
        }

        Ok(())
    }

    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError> {
        // VFS-level glob always supports ** recursion; the shell expansion layer
        // controls whether ** is sent as a pattern based on `shopt -s globstar`.
        self.glob_with_opts(
            pattern,
            cwd,
            &GlobOptions {
                globstar: true,
                ..GlobOptions::default()
            },
        )
    }

    fn glob_with_opts(
        &self,
        pattern: &str,
        cwd: &Path,
        opts: &GlobOptions,
    ) -> Result<Vec<PathBuf>, VfsError> {
        let is_absolute = pattern.starts_with('/');
        let abs_pattern = if is_absolute {
            pattern.to_string()
        } else {
            let cwd_str = cwd.to_str().unwrap_or("/").trim_end_matches('/');
            format!("{cwd_str}/{pattern}")
        };

        let components: Vec<&str> = abs_pattern.split('/').filter(|s| !s.is_empty()).collect();
        let tree = self.root.read();
        let mut results = Vec::new();
        let max = 100_000;
        glob_collect(
            &tree,
            &components,
            PathBuf::from("/"),
            &mut results,
            &tree,
            max,
            opts,
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
        Arc::new(InMemoryFs::deep_clone(self))
    }
}

// ---------------------------------------------------------------------------
// Glob tree walk
// ---------------------------------------------------------------------------

use crate::interpreter::pattern::{glob_match, glob_match_nocase};

/// Recursively collect filesystem paths matching a glob pattern.
///
/// `node` is the current tree position, `components` the remaining pattern
/// segments, and `current_path` the path assembled so far. `tree_root` is
/// used for resolving symlinks, and `max` caps the result count.
fn glob_collect(
    node: &FsNode,
    components: &[&str],
    current_path: PathBuf,
    results: &mut Vec<PathBuf>,
    tree_root: &FsNode,
    max: usize,
    opts: &GlobOptions,
) {
    if results.len() >= max {
        return;
    }

    if components.is_empty() {
        results.push(current_path);
        return;
    }

    // Resolve symlinks so we can see through them to the target directory.
    let resolved =
        resolve_if_symlink(node, &current_path, MAX_SYMLINK_DEPTH, tree_root).unwrap_or(node);

    let pattern = components[0];
    let rest = &components[1..];

    if pattern == "**" && opts.globstar {
        // Zero directories — advance past **
        glob_collect(
            resolved,
            rest,
            current_path.clone(),
            results,
            tree_root,
            max,
            opts,
        );

        // One or more directories — recurse into children
        if let FsNode::Directory { children, .. } = resolved {
            for (name, child) in children {
                if results.len() >= max {
                    return;
                }
                if name.starts_with('.') && !opts.dotglob {
                    continue;
                }
                let child_path = current_path.join(name);
                glob_collect(child, components, child_path, results, tree_root, max, opts);
            }
        }
    } else {
        // When globstar is off, treat ** as *
        let effective_pattern = if pattern == "**" { "*" } else { pattern };

        if let FsNode::Directory { children, .. } = resolved {
            for (name, child) in children {
                if results.len() >= max {
                    return;
                }
                // Skip hidden files unless dotglob is on or pattern explicitly starts with '.'
                if name.starts_with('.') && !effective_pattern.starts_with('.') && !opts.dotglob {
                    continue;
                }
                let matched = if opts.nocaseglob {
                    glob_match_nocase(effective_pattern, name)
                } else {
                    glob_match(effective_pattern, name)
                };
                if matched {
                    let child_path = current_path.join(name);
                    if rest.is_empty() {
                        results.push(child_path);
                    } else {
                        glob_collect(child, rest, child_path, results, tree_root, max, opts);
                    }
                }
            }
        }
    }
}

/// Extract metadata from a node.
fn node_metadata(node: &FsNode) -> Metadata {
    match node {
        FsNode::File {
            content,
            mode,
            mtime,
            ..
        } => Metadata {
            node_type: NodeType::File,
            size: content.len() as u64,
            mode: *mode,
            mtime: *mtime,
        },
        FsNode::Directory { mode, mtime, .. } => Metadata {
            node_type: NodeType::Directory,
            size: 0,
            mode: *mode,
            mtime: *mtime,
        },
        FsNode::Symlink { target, mtime, .. } => Metadata {
            node_type: NodeType::Symlink,
            size: target.to_string_lossy().len() as u64,
            mode: 0o777,
            mtime: *mtime,
        },
    }
}
