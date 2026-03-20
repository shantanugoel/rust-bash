//! Tests for OverlayFs.

use std::path::{Path, PathBuf};

use crate::platform::SystemTime;

use tempfile::TempDir;

use crate::vfs::{NodeType, OverlayFs, VirtualFs};

/// Helper: create a temp directory with some files for use as the lower layer.
fn setup_lower() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    // /src/main.rs
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("src/main.rs"), b"fn main() {}").unwrap();

    // /README.md
    std::fs::write(base.join("README.md"), b"# Hello").unwrap();

    // /data/config.toml
    std::fs::create_dir_all(base.join("data")).unwrap();
    std::fs::write(base.join("data/config.toml"), b"key = \"value\"").unwrap();

    tmp
}

/// Helper: build an OverlayFs with the lower rooted at virtual "/".
fn make_overlay(lower: &Path) -> OverlayFs {
    OverlayFs::new(lower).unwrap()
}

// -----------------------------------------------------------------------
// 3l.1 Read-through from lower
// -----------------------------------------------------------------------

#[test]
fn read_through_from_lower() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let content = ov.read_file(Path::new("/src/main.rs")).unwrap();
    assert_eq!(content, b"fn main() {}");

    let content = ov.read_file(Path::new("/README.md")).unwrap();
    assert_eq!(content, b"# Hello");
}

// -----------------------------------------------------------------------
// 3l.2 Write isolation
// -----------------------------------------------------------------------

#[test]
fn write_isolation() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Write a new file via overlay
    ov.write_file(Path::new("/new_file.txt"), b"overlay data")
        .unwrap();

    // Readable through overlay
    assert_eq!(
        ov.read_file(Path::new("/new_file.txt")).unwrap(),
        b"overlay data"
    );

    // NOT on disk
    assert!(!tmp.path().join("new_file.txt").exists());
}

#[test]
fn overwrite_lower_file_does_not_touch_disk() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/README.md"), b"overwritten")
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/README.md")).unwrap(),
        b"overwritten"
    );

    // Lower file is unchanged
    let on_disk = std::fs::read(tmp.path().join("README.md")).unwrap();
    assert_eq!(on_disk, b"# Hello");
}

// -----------------------------------------------------------------------
// 3l.3 Whiteout
// -----------------------------------------------------------------------

#[test]
fn whiteout_hides_lower_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    assert!(ov.exists(Path::new("/README.md")));
    ov.remove_file(Path::new("/README.md")).unwrap();
    assert!(!ov.exists(Path::new("/README.md")));

    // Still on disk
    assert!(tmp.path().join("README.md").exists());
}

// -----------------------------------------------------------------------
// 3l.4 Copy-up on modify
// -----------------------------------------------------------------------

#[test]
fn copy_up_on_append() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.append_file(Path::new("/README.md"), b"\nAppended")
        .unwrap();
    let content = ov.read_file(Path::new("/README.md")).unwrap();
    assert_eq!(content, b"# Hello\nAppended");

    // Lower unchanged
    let on_disk = std::fs::read(tmp.path().join("README.md")).unwrap();
    assert_eq!(on_disk, b"# Hello");
}

// -----------------------------------------------------------------------
// 3l.5 Merged readdir
// -----------------------------------------------------------------------

#[test]
fn merged_readdir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Write a new file in the same dir as a lower file
    ov.write_file(Path::new("/data/extra.txt"), b"extra")
        .unwrap();

    let mut entries = ov.readdir(Path::new("/data")).unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["config.toml", "extra.txt"]);
}

#[test]
fn readdir_excludes_whiteouts() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/data/config.toml")).unwrap();
    let entries = ov.readdir(Path::new("/data")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"config.toml"));
}

// -----------------------------------------------------------------------
// 3l.6 Rename across layers
// -----------------------------------------------------------------------

#[test]
fn rename_lower_only_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.rename(Path::new("/README.md"), Path::new("/RENAMED.md"))
        .unwrap();

    // New name exists
    assert_eq!(ov.read_file(Path::new("/RENAMED.md")).unwrap(), b"# Hello");

    // Old name gone
    assert!(!ov.exists(Path::new("/README.md")));

    // Lower unchanged
    assert!(tmp.path().join("README.md").exists());
}

// -----------------------------------------------------------------------
// 3l.7 Glob merging
// -----------------------------------------------------------------------

#[test]
fn glob_merging() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/src/lib.rs"), b"pub mod lib;")
        .unwrap();

    let mut matches = ov.glob("*.rs", Path::new("/src")).unwrap();
    matches.sort();
    assert_eq!(
        matches,
        vec![PathBuf::from("lib.rs"), PathBuf::from("main.rs")]
    );
}

// -----------------------------------------------------------------------
// 3l.8 deep_clone isolation
// -----------------------------------------------------------------------

#[test]
fn deep_clone_isolation() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/cloneable.txt"), b"original")
        .unwrap();

    let clone = ov.deep_clone();

    // Mutate the clone
    clone
        .write_file(Path::new("/cloneable.txt"), b"mutated")
        .unwrap();
    clone
        .write_file(Path::new("/clone_only.txt"), b"only in clone")
        .unwrap();

    // Original unaffected
    assert_eq!(
        ov.read_file(Path::new("/cloneable.txt")).unwrap(),
        b"original"
    );
    assert!(!ov.exists(Path::new("/clone_only.txt")));

    // Clone sees its changes
    assert_eq!(
        clone.read_file(Path::new("/cloneable.txt")).unwrap(),
        b"mutated"
    );

    // Both can read from lower
    assert_eq!(
        clone.read_file(Path::new("/README.md")).unwrap(),
        b"# Hello"
    );
}

// -----------------------------------------------------------------------
// 3l.9 Non-existent lower → constructor error
// -----------------------------------------------------------------------

#[test]
fn constructor_error_for_nonexistent_lower() {
    let result = OverlayFs::new("/nonexistent/directory/that/does/not/exist");
    assert!(result.is_err());
}

// -----------------------------------------------------------------------
// 3l.10 Ancestor whiteout hides descendants
// -----------------------------------------------------------------------

#[test]
fn ancestor_whiteout_hides_descendants() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Remove the entire /src directory
    ov.remove_dir_all(Path::new("/src")).unwrap();

    // /src/main.rs should be gone
    assert!(!ov.exists(Path::new("/src/main.rs")));
    assert!(!ov.exists(Path::new("/src")));

    // Reading should fail
    assert!(ov.read_file(Path::new("/src/main.rs")).is_err());
}

// -----------------------------------------------------------------------
// 3l.11 mkdir_p through lower-only directories
// -----------------------------------------------------------------------

#[test]
fn mkdir_p_through_lower_dirs() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // /src exists only in lower. mkdir_p should recognize it and create only build/release
    ov.mkdir_p(Path::new("/src/build/release")).unwrap();
    assert!(ov.exists(Path::new("/src/build/release")));

    // /src is still from lower (not duplicated into upper unnecessarily)
    // The important thing is that it works correctly
    let entries = ov.readdir(Path::new("/src")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"main.rs"));
    assert!(names.contains(&"build"));
}

// -----------------------------------------------------------------------
// 3l.12 stat / lstat / chmod / utimes
// -----------------------------------------------------------------------

#[test]
fn stat_follows_through_layers() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let meta = ov.stat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
    assert_eq!(meta.size, 7); // "# Hello"
}

#[test]
fn lstat_on_lower_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let meta = ov.lstat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
}

#[test]
fn chmod_lower_file_copies_up() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.chmod(Path::new("/README.md"), 0o755).unwrap();
    let meta = ov.stat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.mode, 0o755);

    // Content preserved
    assert_eq!(ov.read_file(Path::new("/README.md")).unwrap(), b"# Hello");

    // Lower untouched
    let disk_meta = std::fs::metadata(tmp.path().join("README.md")).unwrap();
    assert_ne!(
        disk_meta.permissions().mode() & 0o777,
        0o755,
        "lower should not be modified"
    );
}

#[test]
fn utimes_lower_file_copies_up() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let new_time = SystemTime::UNIX_EPOCH;
    ov.utimes(Path::new("/README.md"), new_time).unwrap();
    let meta = ov.stat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.mtime, new_time);
}

// -----------------------------------------------------------------------
// Additional edge cases
// -----------------------------------------------------------------------

#[test]
fn exists_root() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    assert!(ov.exists(Path::new("/")));
}

#[test]
fn readdir_root_merges_both_layers() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/upper_only.txt"), b"hi").unwrap();

    let entries = ov.readdir(Path::new("/")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"README.md")); // lower
    assert!(names.contains(&"src")); // lower dir
    assert!(names.contains(&"upper_only.txt")); // upper
}

#[test]
fn copy_from_lower_to_upper() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.copy(Path::new("/README.md"), Path::new("/README_copy.md"))
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/README_copy.md")).unwrap(),
        b"# Hello"
    );
    // Lower untouched
    assert!(!tmp.path().join("README_copy.md").exists());
}

#[test]
fn remove_file_then_recreate() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/README.md")).unwrap();
    assert!(!ov.exists(Path::new("/README.md")));

    ov.write_file(Path::new("/README.md"), b"new content")
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/README.md")).unwrap(),
        b"new content"
    );
}

#[test]
fn hardlink_from_lower() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.hardlink(Path::new("/README.md"), Path::new("/link.md"))
        .unwrap();
    assert_eq!(ov.read_file(Path::new("/link.md")).unwrap(), b"# Hello");

    // Modifying one doesn't affect the other (no real hardlink in overlay)
    ov.write_file(Path::new("/link.md"), b"changed").unwrap();
    assert_eq!(ov.read_file(Path::new("/README.md")).unwrap(), b"# Hello");
}

#[test]
fn symlink_in_upper() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link_to_readme"))
        .unwrap();
    let target = ov.readlink(Path::new("/link_to_readme")).unwrap();
    assert_eq!(target, PathBuf::from("/README.md"));

    // Reading through the symlink should work
    let content = ov.read_file(Path::new("/link_to_readme")).unwrap();
    assert_eq!(content, b"# Hello");
}

#[test]
fn glob_absolute_pattern() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let mut matches = ov.glob("/data/*", Path::new("/")).unwrap();
    matches.sort();
    assert_eq!(matches, vec![PathBuf::from("/data/config.toml")]);
}

#[test]
fn deep_clone_whiteout_isolation() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let clone = ov.deep_clone();
    clone.remove_file(Path::new("/README.md")).unwrap();

    // Original still has the file
    assert!(ov.exists(Path::new("/README.md")));
    // Clone does not
    assert!(!clone.exists(Path::new("/README.md")));
}

use std::os::unix::fs::PermissionsExt;

#[test]
fn chmod_lower_preserves_original_permissions() {
    let tmp = setup_lower();
    // Set specific permissions on the lower file
    let lower_path = tmp.path().join("README.md");
    std::fs::set_permissions(&lower_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let ov = make_overlay(tmp.path());
    ov.chmod(Path::new("/README.md"), 0o700).unwrap();

    // Overlay reports new mode
    assert_eq!(ov.stat(Path::new("/README.md")).unwrap().mode, 0o700);

    // Lower file still has old mode
    let lower_meta = std::fs::metadata(&lower_path).unwrap();
    assert_eq!(lower_meta.permissions().mode() & 0o777, 0o644);
}

#[test]
fn remove_dir_empty_upper_dir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.mkdir(Path::new("/empty_dir")).unwrap();
    assert!(ov.exists(Path::new("/empty_dir")));

    ov.remove_dir(Path::new("/empty_dir")).unwrap();
    assert!(!ov.exists(Path::new("/empty_dir")));
}

#[test]
fn mkdir_after_rmdir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Remove a lower-only directory (must be empty first)
    ov.remove_file(Path::new("/data/config.toml")).unwrap();
    ov.remove_dir(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));

    // Re-create it
    ov.mkdir(Path::new("/data")).unwrap();
    assert!(ov.exists(Path::new("/data")));
}

#[test]
fn canonicalize_lower_path() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let canon = ov.canonicalize(Path::new("/src/main.rs")).unwrap();
    assert_eq!(canon, PathBuf::from("/src/main.rs"));
}

#[test]
fn canonicalize_upper_path() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/upper.txt"), b"hi").unwrap();
    let canon = ov.canonicalize(Path::new("/upper.txt")).unwrap();
    assert_eq!(canon, PathBuf::from("/upper.txt"));
}

#[test]
fn canonicalize_nonexistent_fails() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    assert!(ov.canonicalize(Path::new("/no/such/path")).is_err());
}

#[test]
fn stat_directory_from_lower() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let meta = ov.stat(Path::new("/src")).unwrap();
    assert_eq!(meta.node_type, NodeType::Directory);
}

#[test]
fn write_to_nested_new_dir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.mkdir_p(Path::new("/a/b/c")).unwrap();
    ov.write_file(Path::new("/a/b/c/file.txt"), b"deep")
        .unwrap();
    assert_eq!(ov.read_file(Path::new("/a/b/c/file.txt")).unwrap(), b"deep");
}

// -----------------------------------------------------------------------
// Additional edge cases suggested by review
// -----------------------------------------------------------------------

#[test]
fn mkdir_p_after_remove_dir_all_does_not_resurrect_siblings() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Lower has /data/config.toml
    assert!(ov.exists(Path::new("/data/config.toml")));

    // Remove everything under /data
    ov.remove_dir_all(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));
    assert!(!ov.exists(Path::new("/data/config.toml")));

    // Re-create /data/sub — config.toml must NOT reappear
    ov.mkdir_p(Path::new("/data/sub")).unwrap();
    assert!(ov.exists(Path::new("/data/sub")));
    assert!(!ov.exists(Path::new("/data/config.toml")));

    let entries = ov.readdir(Path::new("/data")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["sub"]);
}

#[test]
fn remove_dir_nonempty_merged_directory() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // /data has config.toml from lower — remove_dir should fail
    let result = ov.remove_dir(Path::new("/data"));
    assert!(result.is_err());
}

#[test]
fn append_file_nonexistent_returns_not_found() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let result = ov.append_file(Path::new("/no_such_file.txt"), b"data");
    assert!(result.is_err());
}

#[test]
fn glob_excludes_whiteouts() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/data/config.toml")).unwrap();

    // Glob should not return the whiteout-ed file
    let matches = ov.glob("*", Path::new("/data")).unwrap();
    assert!(matches.is_empty());
}

#[test]
fn rename_directory_with_mixed_layer_children() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Add an upper-only file alongside the lower-only file
    ov.write_file(Path::new("/src/upper_file.rs"), b"upper")
        .unwrap();

    // Rename the directory
    ov.rename(Path::new("/src"), Path::new("/source")).unwrap();

    // Both children should be under the new name
    assert_eq!(
        ov.read_file(Path::new("/source/main.rs")).unwrap(),
        b"fn main() {}"
    );
    assert_eq!(
        ov.read_file(Path::new("/source/upper_file.rs")).unwrap(),
        b"upper"
    );

    // Old name should be gone
    assert!(!ov.exists(Path::new("/src")));
    assert!(!ov.exists(Path::new("/src/main.rs")));
}

// -----------------------------------------------------------------------
// FIX 1: ensure_upper_dir_path clears ancestor whiteouts
// -----------------------------------------------------------------------

#[test]
fn write_file_under_removed_dir_all_is_visible() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // /data exists in lower with config.toml
    assert!(ov.exists(Path::new("/data/config.toml")));

    // Remove the entire /data tree
    ov.remove_dir_all(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));

    // Write a new file under /data — ensure_upper_dir_path must clear the
    // whiteout on /data for this to become visible.
    ov.write_file(Path::new("/data/new.txt"), b"hello").unwrap();
    assert!(ov.exists(Path::new("/data")));
    assert!(ov.exists(Path::new("/data/new.txt")));
    assert_eq!(ov.read_file(Path::new("/data/new.txt")).unwrap(), b"hello");

    // The old file must NOT reappear
    assert!(!ov.exists(Path::new("/data/config.toml")));
}

#[test]
fn mkdir_under_whiteout_ancestor() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Remove /data entirely
    ov.remove_dir_all(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));

    // mkdir (not mkdir_p) a new dir under a re-created parent
    ov.mkdir_p(Path::new("/data")).unwrap();
    ov.mkdir(Path::new("/data/sub")).unwrap();
    assert!(ov.exists(Path::new("/data/sub")));

    // Old children still gone
    assert!(!ov.exists(Path::new("/data/config.toml")));
}

// -----------------------------------------------------------------------
// FIX 5: append_file / chmod / utimes follow symlinks through overlay
// -----------------------------------------------------------------------

#[test]
fn append_through_symlink_to_lower_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Create a symlink in upper pointing to a lower-layer file
    ov.symlink(Path::new("/README.md"), Path::new("/link_to_readme"))
        .unwrap();

    // Append through the symlink
    ov.append_file(Path::new("/link_to_readme"), b" world")
        .unwrap();

    // The target file should have the appended content
    assert_eq!(
        ov.read_file(Path::new("/README.md")).unwrap(),
        b"# Hello world"
    );
    // Reading through the symlink should also work
    assert_eq!(
        ov.read_file(Path::new("/link_to_readme")).unwrap(),
        b"# Hello world"
    );
}

#[test]
fn chmod_through_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link_readme"))
        .unwrap();
    ov.chmod(Path::new("/link_readme"), 0o700).unwrap();

    // The target file should have the new mode
    assert_eq!(ov.stat(Path::new("/README.md")).unwrap().mode, 0o700);
}

#[test]
fn utimes_through_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link_readme"))
        .unwrap();
    let new_time = SystemTime::UNIX_EPOCH;
    ov.utimes(Path::new("/link_readme"), new_time).unwrap();

    assert_eq!(ov.stat(Path::new("/README.md")).unwrap().mtime, new_time);
}
