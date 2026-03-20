//! Tests for MountableFs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::vfs::{InMemoryFs, MountableFs, NodeType, VirtualFs};

/// Helper: create an InMemoryFs with some seed files.
fn make_memory_fs(files: &[(&str, &[u8])]) -> Arc<InMemoryFs> {
    let fs = InMemoryFs::new();
    for (path, content) in files {
        let p = Path::new(path);
        if let Some(parent) = p.parent() {
            if parent != Path::new("/") {
                fs.mkdir_p(parent).unwrap();
            }
        }
        fs.write_file(p, content).unwrap();
    }
    Arc::new(fs)
}

// -----------------------------------------------------------------------
// 4i.1 Basic delegation: read/write through mount points
// -----------------------------------------------------------------------

#[test]
fn basic_read_write_through_mount() {
    let root_fs = make_memory_fs(&[("/hello.txt", b"root hello")]);
    let project_fs = make_memory_fs(&[("/README.md", b"project readme")]);

    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", project_fs);

    assert_eq!(
        mfs.read_file(Path::new("/hello.txt")).unwrap(),
        b"root hello"
    );
    assert_eq!(
        mfs.read_file(Path::new("/project/README.md")).unwrap(),
        b"project readme"
    );

    // Write through mount
    mfs.write_file(Path::new("/project/new.txt"), b"new content")
        .unwrap();
    assert_eq!(
        mfs.read_file(Path::new("/project/new.txt")).unwrap(),
        b"new content"
    );
}

// -----------------------------------------------------------------------
// 4i.2 Longest-prefix: /project/src mount preferred over /project
// -----------------------------------------------------------------------

#[test]
fn longest_prefix_matching() {
    let project_fs = make_memory_fs(&[("/lib.rs", b"project lib")]);
    let src_fs = make_memory_fs(&[("/main.rs", b"src main")]);

    let mfs = MountableFs::new()
        .mount("/project", project_fs)
        .mount("/project/src", src_fs);

    // /project/src/main.rs should resolve to src_fs's /main.rs
    assert_eq!(
        mfs.read_file(Path::new("/project/src/main.rs")).unwrap(),
        b"src main"
    );

    // /project/lib.rs should resolve to project_fs's /lib.rs
    assert_eq!(
        mfs.read_file(Path::new("/project/lib.rs")).unwrap(),
        b"project lib"
    );
}

// -----------------------------------------------------------------------
// 4i.3 Cross-mount copy
// -----------------------------------------------------------------------

#[test]
fn cross_mount_copy() {
    let fs_a = make_memory_fs(&[("/file.txt", b"data from a")]);
    let fs_b: Arc<InMemoryFs> = make_memory_fs(&[]);

    let mfs = MountableFs::new()
        .mount("/a", fs_a.clone())
        .mount("/b", fs_b.clone());

    mfs.copy(Path::new("/a/file.txt"), Path::new("/b/file.txt"))
        .unwrap();

    // Destination should have the content
    assert_eq!(
        mfs.read_file(Path::new("/b/file.txt")).unwrap(),
        b"data from a"
    );
    // Source should still exist
    assert_eq!(
        mfs.read_file(Path::new("/a/file.txt")).unwrap(),
        b"data from a"
    );
}

// -----------------------------------------------------------------------
// 4i.4 Cross-mount rename (copy + delete semantics)
// -----------------------------------------------------------------------

#[test]
fn cross_mount_rename() {
    let fs_a = make_memory_fs(&[("/file.txt", b"move me")]);
    let fs_b: Arc<InMemoryFs> = make_memory_fs(&[]);

    let mfs = MountableFs::new()
        .mount("/a", fs_a.clone())
        .mount("/b", fs_b.clone());

    mfs.rename(Path::new("/a/file.txt"), Path::new("/b/file.txt"))
        .unwrap();

    // Destination should have the content
    assert_eq!(mfs.read_file(Path::new("/b/file.txt")).unwrap(), b"move me");
    // Source should be gone
    assert!(!mfs.exists(Path::new("/a/file.txt")));
}

// -----------------------------------------------------------------------
// 4i.5 Directory listing at boundaries: mount points appear as directories
// -----------------------------------------------------------------------

#[test]
fn directory_listing_shows_mount_points() {
    let root_fs = make_memory_fs(&[("/root_file.txt", b"root")]);

    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", make_memory_fs(&[("/README.md", b"hi")]));

    let entries = mfs.readdir(Path::new("/")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

    assert!(names.contains(&"root_file.txt"), "entries: {names:?}");
    assert!(names.contains(&"project"), "entries: {names:?}");

    // The mount point entry should be a directory
    let project_entry = entries.iter().find(|e| e.name == "project").unwrap();
    assert_eq!(project_entry.node_type, NodeType::Directory);
}

#[test]
fn directory_listing_deduplicates_mount_with_real_dir() {
    // Root fs already has a "project" directory
    let root_fs = InMemoryFs::new();
    root_fs.mkdir_p(Path::new("/project")).unwrap();
    root_fs
        .write_file(Path::new("/other.txt"), b"other")
        .unwrap();

    let mfs = MountableFs::new()
        .mount("/", Arc::new(root_fs))
        .mount("/project", make_memory_fs(&[]));

    let entries = mfs.readdir(Path::new("/")).unwrap();
    let project_count = entries.iter().filter(|e| e.name == "project").count();
    assert_eq!(project_count, 1, "mount point should not be duplicated");
}

// -----------------------------------------------------------------------
// 4i.6 Mount at root: single mount at "/" works as full delegation
// -----------------------------------------------------------------------

#[test]
fn single_root_mount() {
    let root_fs = make_memory_fs(&[("/a.txt", b"aaa")]);
    let mfs = MountableFs::new().mount("/", root_fs);

    assert_eq!(mfs.read_file(Path::new("/a.txt")).unwrap(), b"aaa");
    mfs.write_file(Path::new("/b.txt"), b"bbb").unwrap();
    assert_eq!(mfs.read_file(Path::new("/b.txt")).unwrap(), b"bbb");
    assert!(mfs.exists(Path::new("/")));
}

// -----------------------------------------------------------------------
// 4i.7 Multiple mounts: complex setup with 3+ backends
// -----------------------------------------------------------------------

#[test]
fn multiple_mounts_complex_setup() {
    let root_fs = make_memory_fs(&[("/etc/hostname", b"myhost")]);
    let project_fs = make_memory_fs(&[("/Cargo.toml", b"[package]")]);
    let tmp_fs = make_memory_fs(&[]);

    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", project_fs)
        .mount("/tmp", tmp_fs);

    // Read from root
    assert_eq!(
        mfs.read_file(Path::new("/etc/hostname")).unwrap(),
        b"myhost"
    );
    // Read from project
    assert_eq!(
        mfs.read_file(Path::new("/project/Cargo.toml")).unwrap(),
        b"[package]"
    );
    // Write to tmp
    mfs.write_file(Path::new("/tmp/scratch.txt"), b"temp data")
        .unwrap();
    assert_eq!(
        mfs.read_file(Path::new("/tmp/scratch.txt")).unwrap(),
        b"temp data"
    );

    // Verify listings show mount points
    let root_entries = mfs.readdir(Path::new("/")).unwrap();
    let names: Vec<&str> = root_entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"etc"));
    assert!(names.contains(&"project"));
    assert!(names.contains(&"tmp"));
}

// -----------------------------------------------------------------------
// 4i.8 deep_clone isolation
// -----------------------------------------------------------------------

#[test]
fn deep_clone_isolation() {
    let root_fs = make_memory_fs(&[("/file.txt", b"original")]);
    let mfs = MountableFs::new().mount("/", root_fs);

    let cloned = mfs.deep_clone();

    // Mutate the clone
    cloned
        .write_file(Path::new("/file.txt"), b"modified")
        .unwrap();
    cloned
        .write_file(Path::new("/new.txt"), b"only in clone")
        .unwrap();

    // Original is unchanged
    assert_eq!(mfs.read_file(Path::new("/file.txt")).unwrap(), b"original");
    assert!(!mfs.exists(Path::new("/new.txt")));

    // Clone has changes
    assert_eq!(
        cloned.read_file(Path::new("/file.txt")).unwrap(),
        b"modified"
    );
    assert_eq!(
        cloned.read_file(Path::new("/new.txt")).unwrap(),
        b"only in clone"
    );
}

// -----------------------------------------------------------------------
// 4i.9 deep_clone with ReadWriteFs mount
// -----------------------------------------------------------------------

#[test]
fn deep_clone_with_readwrite_fs_mount() {
    use crate::vfs::ReadWriteFs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("real.txt"), b"real data").unwrap();

    let rw_fs = Arc::new(ReadWriteFs::with_root(tmp.path()).unwrap());
    let mem_fs = make_memory_fs(&[("/mem.txt", b"memory data")]);

    let mfs = MountableFs::new().mount("/", mem_fs).mount("/real", rw_fs);

    let cloned = mfs.deep_clone();

    // ReadWriteFs deep_clone is a passthrough — both see the same real FS
    assert_eq!(
        cloned.read_file(Path::new("/real/real.txt")).unwrap(),
        b"real data"
    );

    // InMemoryFs clone is isolated
    cloned
        .write_file(Path::new("/mem.txt"), b"changed in clone")
        .unwrap();
    assert_eq!(
        mfs.read_file(Path::new("/mem.txt")).unwrap(),
        b"memory data"
    );
}

// -----------------------------------------------------------------------
// 4i.10 Glob across mounts
// -----------------------------------------------------------------------

#[test]
fn glob_across_mounts() {
    let root_fs = make_memory_fs(&[("/root.txt", b"r")]);
    let project_fs = make_memory_fs(&[("/src/main.rs", b"fn main() {}")]);

    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", project_fs);

    // Glob at root level should find entries from root and mount points
    let matches = mfs.glob("/*", Path::new("/")).unwrap();
    let strs: Vec<String> = matches
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert!(strs.contains(&"/root.txt".to_string()), "got: {strs:?}");
    assert!(
        strs.contains(&"/project".to_string()),
        "mount point should appear in glob: {strs:?}"
    );

    // Glob inside a mount
    let matches = mfs.glob("/project/src/*.rs", Path::new("/")).unwrap();
    assert_eq!(matches, vec![PathBuf::from("/project/src/main.rs")]);
}

#[test]
fn glob_relative_pattern() {
    let root_fs = make_memory_fs(&[("/home/user/a.txt", b"a"), ("/home/user/b.txt", b"b")]);
    let mfs = MountableFs::new().mount("/", root_fs);

    let matches = mfs.glob("*.txt", Path::new("/home/user")).unwrap();
    let names: Vec<String> = matches
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"a.txt".to_string()), "got: {names:?}");
    assert!(names.contains(&"b.txt".to_string()), "got: {names:?}");
}

// -----------------------------------------------------------------------
// 4i.11 No mount found → NotFound
// -----------------------------------------------------------------------

#[test]
fn no_mount_returns_not_found() {
    // MountableFs with no root mount — only /project is mounted
    let project_fs = make_memory_fs(&[("/file.txt", b"data")]);
    let mfs = MountableFs::new().mount("/project", project_fs);

    let result = mfs.read_file(Path::new("/etc/config"));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        crate::error::VfsError::NotFound(_)
    ));
}

// -----------------------------------------------------------------------
// 4i.12 exists() at mount point itself
// -----------------------------------------------------------------------

#[test]
fn exists_at_mount_point() {
    let root_fs = make_memory_fs(&[]);
    let project_fs = make_memory_fs(&[("/file.txt", b"data")]);

    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", project_fs);

    // Mount points should be treated as existing directories
    assert!(mfs.exists(Path::new("/project")));
    assert!(mfs.exists(Path::new("/")));
}

#[test]
fn stat_at_mount_point() {
    let root_fs = make_memory_fs(&[]);
    let project_fs = make_memory_fs(&[]);

    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", project_fs);

    let meta = mfs.stat(Path::new("/project")).unwrap();
    assert_eq!(meta.node_type, NodeType::Directory);
}

#[test]
fn exists_at_mount_ancestor() {
    // No root mount, but /a/b/c is mounted — /a and /a/b should exist as
    // synthetic ancestors.
    let fs = make_memory_fs(&[]);
    let mfs = MountableFs::new().mount("/a/b/c", fs);

    assert!(mfs.exists(Path::new("/a")));
    assert!(mfs.exists(Path::new("/a/b")));
    assert!(mfs.exists(Path::new("/a/b/c")));
    assert!(!mfs.exists(Path::new("/other")));
}

// -----------------------------------------------------------------------
// 4i.13 Full integration: create shell with MountableFs via builder
// -----------------------------------------------------------------------

#[test]
fn integration_shell_with_mountable_fs() {
    use crate::api::RustBashBuilder;

    let project_fs = make_memory_fs(&[("/hello.txt", b"Hello from project!")]);
    let root_fs = Arc::new(InMemoryFs::new());

    let mountable = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", project_fs);

    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    let result = shell.exec("cat /project/hello.txt").unwrap();
    assert_eq!(result.stdout.trim(), "Hello from project!");

    let result = shell
        .exec("echo test > /tmp_file.txt && cat /tmp_file.txt")
        .unwrap();
    assert_eq!(result.stdout, "test\n");
}

// -----------------------------------------------------------------------
// Additional edge-case tests
// -----------------------------------------------------------------------

#[test]
fn hardlink_across_mounts_returns_error() {
    let fs_a = make_memory_fs(&[("/file.txt", b"data")]);
    let fs_b = make_memory_fs(&[]);

    let mfs = MountableFs::new().mount("/a", fs_a).mount("/b", fs_b);

    let result = mfs.hardlink(Path::new("/a/file.txt"), Path::new("/b/link.txt"));
    assert!(result.is_err());
}

#[test]
fn hardlink_within_same_mount_works() {
    let fs = make_memory_fs(&[("/file.txt", b"data")]);
    let mfs = MountableFs::new().mount("/", fs);

    mfs.hardlink(Path::new("/file.txt"), Path::new("/link.txt"))
        .unwrap();
    assert_eq!(mfs.read_file(Path::new("/link.txt")).unwrap(), b"data");
}

#[test]
fn same_mount_copy_delegates_directly() {
    let fs = make_memory_fs(&[("/a.txt", b"hello")]);
    let mfs = MountableFs::new().mount("/", fs);

    mfs.copy(Path::new("/a.txt"), Path::new("/b.txt")).unwrap();
    assert_eq!(mfs.read_file(Path::new("/b.txt")).unwrap(), b"hello");
    assert_eq!(mfs.read_file(Path::new("/a.txt")).unwrap(), b"hello");
}

#[test]
fn same_mount_rename_delegates_directly() {
    let fs = make_memory_fs(&[("/a.txt", b"hello")]);
    let mfs = MountableFs::new().mount("/", fs);

    mfs.rename(Path::new("/a.txt"), Path::new("/b.txt"))
        .unwrap();
    assert_eq!(mfs.read_file(Path::new("/b.txt")).unwrap(), b"hello");
    assert!(!mfs.exists(Path::new("/a.txt")));
}

#[test]
fn mkdir_and_write_through_mount() {
    let fs = make_memory_fs(&[]);
    let mfs = MountableFs::new().mount("/", fs);

    mfs.mkdir_p(Path::new("/a/b/c")).unwrap();
    mfs.write_file(Path::new("/a/b/c/file.txt"), b"nested")
        .unwrap();
    assert_eq!(
        mfs.read_file(Path::new("/a/b/c/file.txt")).unwrap(),
        b"nested"
    );
}

#[test]
fn append_file_through_mount() {
    let fs = make_memory_fs(&[("/file.txt", b"hello")]);
    let mfs = MountableFs::new().mount("/", fs);

    mfs.append_file(Path::new("/file.txt"), b" world").unwrap();
    assert_eq!(
        mfs.read_file(Path::new("/file.txt")).unwrap(),
        b"hello world"
    );
}

#[test]
fn remove_file_and_dir_through_mount() {
    let fs = make_memory_fs(&[("/dir/file.txt", b"data")]);
    let mfs = MountableFs::new().mount("/", fs);

    mfs.remove_file(Path::new("/dir/file.txt")).unwrap();
    assert!(!mfs.exists(Path::new("/dir/file.txt")));

    mfs.remove_dir(Path::new("/dir")).unwrap();
    assert!(!mfs.exists(Path::new("/dir")));
}

#[test]
fn canonicalize_through_mount() {
    let fs = make_memory_fs(&[("/dir/file.txt", b"data")]);
    let mfs = MountableFs::new().mount("/data", fs);

    let canonical = mfs.canonicalize(Path::new("/data/dir/file.txt")).unwrap();
    assert_eq!(canonical, PathBuf::from("/data/dir/file.txt"));
}

#[test]
fn canonicalize_at_root_mount() {
    let fs = make_memory_fs(&[("/file.txt", b"data")]);
    let mfs = MountableFs::new().mount("/", fs);

    let canonical = mfs.canonicalize(Path::new("/file.txt")).unwrap();
    assert_eq!(canonical, PathBuf::from("/file.txt"));
}

#[test]
fn readdir_on_unmounted_path_with_child_mounts() {
    // No root mount, but /project is mounted. Listing "/" should show "project".
    let project_fs = make_memory_fs(&[("/file.txt", b"data")]);
    let mfs = MountableFs::new().mount("/project", project_fs);

    let entries = mfs.readdir(Path::new("/")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"project"), "entries: {names:?}");
}

#[test]
fn nested_mount_points_in_listing() {
    let root_fs = make_memory_fs(&[]);
    let project_fs = make_memory_fs(&[]);
    let src_fs = make_memory_fs(&[]);

    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/project", project_fs)
        .mount("/project/src", src_fs);

    // Listing /project should show "src" as a child mount
    let entries = mfs.readdir(Path::new("/project")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"src"), "entries: {names:?}");
}

#[test]
fn readdir_at_synthetic_ancestor() {
    let deep_fs = make_memory_fs(&[("/file.txt", b"deep")]);
    let root_fs = make_memory_fs(&[]);
    let mfs = MountableFs::new()
        .mount("/", root_fs)
        .mount("/a/b/c", deep_fs);

    // /a should be listable and show "b"
    assert!(mfs.exists(Path::new("/a")));
    let entries = mfs.readdir(Path::new("/a")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"b"), "entries: {names:?}");

    // /a/b should show "c"
    let entries = mfs.readdir(Path::new("/a/b")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"c"), "entries: {names:?}");
}

// -----------------------------------------------------------------------
// FIX 2: readdir on empty directory returns Ok(vec![]), not NotFound
// -----------------------------------------------------------------------

#[test]
fn readdir_empty_directory_returns_ok() {
    let root_fs = Arc::new(InMemoryFs::new());
    root_fs.mkdir(Path::new("/empty")).unwrap();
    let mfs = MountableFs::new().mount("/", root_fs);

    let entries = mfs.readdir(Path::new("/empty")).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn readdir_nonexistent_path_returns_not_found() {
    let root_fs = Arc::new(InMemoryFs::new());
    let mfs = MountableFs::new().mount("/", root_fs);

    let result = mfs.readdir(Path::new("/nonexistent"));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        crate::error::VfsError::NotFound(_)
    ));
}

// -----------------------------------------------------------------------
// FIX 3: Cross-mount rename of directory returns clear error
// -----------------------------------------------------------------------

#[test]
fn cross_mount_rename_directory_returns_error() {
    let fs_a = make_memory_fs(&[("/dir/file.txt", b"data")]);
    let fs_b = make_memory_fs(&[]);

    let mfs = MountableFs::new().mount("/a", fs_a).mount("/b", fs_b);

    let result = mfs.rename(Path::new("/a/dir"), Path::new("/b/dir"));
    assert!(result.is_err());
    let err = result.unwrap_err();
    match &err {
        crate::error::VfsError::IoError(msg) => {
            assert!(
                msg.contains("directories across mount boundaries"),
                "unexpected error message: {msg}"
            );
        }
        other => panic!("expected IoError, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// FIX 4: Symlink with absolute target at non-root mount
// -----------------------------------------------------------------------

#[test]
fn symlink_absolute_target_at_non_root_mount() {
    let project_fs = make_memory_fs(&[("/real.txt", b"real content")]);

    let mfs = MountableFs::new()
        .mount("/", Arc::new(InMemoryFs::new()))
        .mount("/project", project_fs);

    // Create a symlink with an absolute target (in global namespace)
    mfs.symlink(Path::new("/project/real.txt"), Path::new("/project/link"))
        .unwrap();

    // Reading through the symlink should work
    let content = mfs.read_file(Path::new("/project/link")).unwrap();
    assert_eq!(content, b"real content");

    // readlink should return the global path
    let target = mfs.readlink(Path::new("/project/link")).unwrap();
    assert_eq!(target, PathBuf::from("/project/real.txt"));
}
