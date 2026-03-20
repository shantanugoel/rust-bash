#[cfg(test)]
mod readwrite_tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use tempfile::TempDir;

    use crate::error::VfsError;
    use crate::vfs::{NodeType, ReadWriteFs, VirtualFs};

    /// Helper: create a ReadWriteFs rooted at a temp directory.
    fn rooted_fs() -> (TempDir, ReadWriteFs) {
        let tmp = TempDir::new().unwrap();
        let fs = ReadWriteFs::with_root(tmp.path()).unwrap();
        (tmp, fs)
    }

    // ======================================================================
    // Basic file CRUD
    // ======================================================================

    #[test]
    fn write_and_read_file() {
        let (tmp, fs) = rooted_fs();
        let _ = tmp; // keep alive
        fs.write_file(Path::new("/hello.txt"), b"Hello, world!")
            .unwrap();
        let content = fs.read_file(Path::new("/hello.txt")).unwrap();
        assert_eq!(content, b"Hello, world!");
    }

    #[test]
    fn overwrite_file() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/f.txt"), b"first").unwrap();
        fs.write_file(Path::new("/f.txt"), b"second").unwrap();
        assert_eq!(fs.read_file(Path::new("/f.txt")).unwrap(), b"second");
    }

    #[test]
    fn append_file() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/f.txt"), b"hello").unwrap();
        fs.append_file(Path::new("/f.txt"), b" world").unwrap();
        assert_eq!(fs.read_file(Path::new("/f.txt")).unwrap(), b"hello world");
    }

    #[test]
    fn append_nonexistent_file_errors() {
        let (_tmp, fs) = rooted_fs();
        let err = fs.append_file(Path::new("/nope.txt"), b"data").unwrap_err();
        assert!(matches!(err, VfsError::NotFound(_)));
    }

    #[test]
    fn remove_file() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/f.txt"), b"data").unwrap();
        fs.remove_file(Path::new("/f.txt")).unwrap();
        assert!(!fs.exists(Path::new("/f.txt")));
    }

    #[test]
    fn read_nonexistent_file_errors() {
        let (_tmp, fs) = rooted_fs();
        let err = fs.read_file(Path::new("/nope.txt")).unwrap_err();
        assert!(matches!(err, VfsError::NotFound(_)));
    }

    // ======================================================================
    // Directory operations
    // ======================================================================

    #[test]
    fn mkdir_and_readdir() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/mydir")).unwrap();
        fs.write_file(Path::new("/mydir/a.txt"), b"a").unwrap();
        fs.write_file(Path::new("/mydir/b.txt"), b"b").unwrap();

        let mut entries: Vec<String> = fs
            .readdir(Path::new("/mydir"))
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        entries.sort();
        assert_eq!(entries, vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn mkdir_p_creates_intermediate_dirs() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir_p(Path::new("/a/b/c")).unwrap();
        assert!(fs.exists(Path::new("/a/b/c")));
        let stat = fs.stat(Path::new("/a/b/c")).unwrap();
        assert_eq!(stat.node_type, NodeType::Directory);
    }

    #[test]
    fn remove_dir_empty() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/empty")).unwrap();
        fs.remove_dir(Path::new("/empty")).unwrap();
        assert!(!fs.exists(Path::new("/empty")));
    }

    #[test]
    fn remove_dir_nonempty_fails() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/nonempty")).unwrap();
        fs.write_file(Path::new("/nonempty/f.txt"), b"x").unwrap();
        let err = fs.remove_dir(Path::new("/nonempty")).unwrap_err();
        // On Linux, removing a non-empty directory gives ENOTEMPTY or EEXIST
        assert!(
            matches!(err, VfsError::DirectoryNotEmpty(_) | VfsError::IoError(_)),
            "expected DirectoryNotEmpty or IoError, got {err:?}"
        );
    }

    #[test]
    fn remove_dir_all() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir_p(Path::new("/tree/sub")).unwrap();
        fs.write_file(Path::new("/tree/sub/f.txt"), b"data")
            .unwrap();
        fs.remove_dir_all(Path::new("/tree")).unwrap();
        assert!(!fs.exists(Path::new("/tree")));
    }

    // ======================================================================
    // Symlink and hardlink operations
    // ======================================================================

    #[test]
    fn symlink_and_readlink() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/target.txt"), b"content").unwrap();
        fs.symlink(Path::new("/target.txt"), Path::new("/link.txt"))
            .unwrap();

        let target = fs.readlink(Path::new("/link.txt")).unwrap();
        assert_eq!(target, PathBuf::from("/target.txt"));

        // Reading through the symlink should work.
        let content = fs.read_file(Path::new("/link.txt")).unwrap();
        assert_eq!(content, b"content");
    }

    #[test]
    fn hardlink_shares_content() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/original.txt"), b"shared")
            .unwrap();
        fs.hardlink(Path::new("/original.txt"), Path::new("/linked.txt"))
            .unwrap();

        let content = fs.read_file(Path::new("/linked.txt")).unwrap();
        assert_eq!(content, b"shared");

        // Modify through one name, visible through the other.
        fs.write_file(Path::new("/linked.txt"), b"modified")
            .unwrap();
        assert_eq!(
            fs.read_file(Path::new("/original.txt")).unwrap(),
            b"modified"
        );
    }

    #[test]
    fn lstat_on_symlink_returns_symlink_type() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/real.txt"), b"data").unwrap();
        fs.symlink(Path::new("/real.txt"), Path::new("/sym.txt"))
            .unwrap();

        let stat = fs.stat(Path::new("/sym.txt")).unwrap();
        assert_eq!(stat.node_type, NodeType::File); // follows symlink

        let lstat = fs.lstat(Path::new("/sym.txt")).unwrap();
        assert_eq!(lstat.node_type, NodeType::Symlink); // does not follow
    }

    // ======================================================================
    // Path restriction enforcement
    // ======================================================================

    #[test]
    fn operations_within_root_succeed() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/inside.txt"), b"ok").unwrap();
        assert_eq!(fs.read_file(Path::new("/inside.txt")).unwrap(), b"ok");
    }

    #[test]
    fn path_traversal_attack_rejected() {
        let (_tmp, fs) = rooted_fs();
        // Attempt to escape via ../
        let err = fs.read_file(Path::new("/../../etc/passwd")).unwrap_err();
        assert!(
            matches!(err, VfsError::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[test]
    fn dotdot_in_middle_of_path_rejected() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/sub")).unwrap();
        let err = fs
            .read_file(Path::new("/sub/../../etc/passwd"))
            .unwrap_err();
        assert!(
            matches!(err, VfsError::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[test]
    fn symlink_escape_rejected() {
        let (tmp, fs) = rooted_fs();
        // Create a symlink pointing outside the root on the real FS.
        let escape_link = tmp.path().join("escape");
        std::os::unix::fs::symlink("/etc", &escape_link).unwrap();

        // Canonicalize should detect the escape.
        let err = fs.canonicalize(Path::new("/escape")).unwrap_err();
        assert!(
            matches!(err, VfsError::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[test]
    fn exists_outside_root_returns_false() {
        let (_tmp, fs) = rooted_fs();
        assert!(!fs.exists(Path::new("/../../etc/passwd")));
    }

    // ======================================================================
    // Write to non-existent file with root restriction
    // ======================================================================

    #[test]
    fn write_new_file_in_root() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/subdir")).unwrap();
        fs.write_file(Path::new("/subdir/new.txt"), b"fresh")
            .unwrap();
        assert_eq!(
            fs.read_file(Path::new("/subdir/new.txt")).unwrap(),
            b"fresh"
        );
    }

    // ======================================================================
    // Glob on real directory tree
    // ======================================================================

    #[test]
    fn glob_star_pattern() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/a.txt"), b"").unwrap();
        fs.write_file(Path::new("/b.txt"), b"").unwrap();
        fs.write_file(Path::new("/c.rs"), b"").unwrap();

        let mut matches = fs.glob("/*.txt", Path::new("/")).unwrap();
        matches.sort();
        assert_eq!(
            matches,
            vec![PathBuf::from("/a.txt"), PathBuf::from("/b.txt")]
        );
    }

    #[test]
    fn glob_relative_pattern() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/src")).unwrap();
        fs.write_file(Path::new("/src/main.rs"), b"").unwrap();
        fs.write_file(Path::new("/src/lib.rs"), b"").unwrap();

        let mut matches = fs.glob("*.rs", Path::new("/src")).unwrap();
        matches.sort();
        assert_eq!(
            matches,
            vec![PathBuf::from("lib.rs"), PathBuf::from("main.rs")]
        );
    }

    #[test]
    fn glob_recursive_pattern() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir_p(Path::new("/a/b")).unwrap();
        fs.write_file(Path::new("/a/x.txt"), b"").unwrap();
        fs.write_file(Path::new("/a/b/y.txt"), b"").unwrap();

        let mut matches = fs.glob("/**/*.txt", Path::new("/")).unwrap();
        matches.sort();
        assert_eq!(
            matches,
            vec![PathBuf::from("/a/b/y.txt"), PathBuf::from("/a/x.txt")]
        );
    }

    #[test]
    fn glob_does_not_escape_root_via_symlink() {
        let (tmp, fs) = rooted_fs();
        // Create a symlink inside root pointing outside
        let escape_link = tmp.path().join("escape");
        std::os::unix::fs::symlink("/etc", &escape_link).unwrap();

        // Glob should not follow the symlink outside root
        let matches = fs.glob("/escape/*", Path::new("/")).unwrap();
        assert!(
            matches.is_empty(),
            "glob should not return results from outside root, got {matches:?}"
        );
    }

    // ======================================================================
    // deep_clone
    // ======================================================================

    #[test]
    fn deep_clone_returns_independent_instance() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/before.txt"), b"data").unwrap();

        let cloned = fs.deep_clone();

        // Both see the same file (passthrough).
        assert_eq!(cloned.read_file(Path::new("/before.txt")).unwrap(), b"data");

        // Writes in the clone are visible to the original (real FS passthrough).
        cloned.write_file(Path::new("/after.txt"), b"new").unwrap();
        assert_eq!(fs.read_file(Path::new("/after.txt")).unwrap(), b"new");
    }

    // ======================================================================
    // stat / lstat / chmod / utimes
    // ======================================================================

    #[test]
    fn stat_on_file() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/f.txt"), b"hello").unwrap();
        let meta = fs.stat(Path::new("/f.txt")).unwrap();
        assert_eq!(meta.node_type, NodeType::File);
        assert_eq!(meta.size, 5);
    }

    #[test]
    fn stat_on_directory() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/d")).unwrap();
        let meta = fs.stat(Path::new("/d")).unwrap();
        assert_eq!(meta.node_type, NodeType::Directory);
    }

    #[test]
    fn chmod_changes_mode() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/f.txt"), b"data").unwrap();
        fs.chmod(Path::new("/f.txt"), 0o755).unwrap();
        let meta = fs.stat(Path::new("/f.txt")).unwrap();
        assert_eq!(meta.mode & 0o777, 0o755);
    }

    #[test]
    fn utimes_changes_mtime() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/f.txt"), b"data").unwrap();

        let new_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        fs.utimes(Path::new("/f.txt"), new_mtime).unwrap();

        let meta = fs.stat(Path::new("/f.txt")).unwrap();
        assert_eq!(meta.mtime, new_mtime);
    }

    // ======================================================================
    // Canonicalize
    // ======================================================================

    #[test]
    fn canonicalize_within_root() {
        let (_tmp, fs) = rooted_fs();
        fs.mkdir_p(Path::new("/a/b")).unwrap();
        let canon = fs.canonicalize(Path::new("/a/b")).unwrap();
        assert_eq!(canon, PathBuf::from("/a/b"));
    }

    // ======================================================================
    // Unrestricted mode
    // ======================================================================

    #[test]
    fn unrestricted_reads_real_file() {
        let tmp = TempDir::new().unwrap();
        let real_path = tmp.path().join("test.txt");
        std::fs::write(&real_path, b"hello").unwrap();

        let fs = ReadWriteFs::new();
        let content = fs.read_file(&real_path).unwrap();
        assert_eq!(content, b"hello");
    }

    #[test]
    fn unrestricted_writes_real_file() {
        let tmp = TempDir::new().unwrap();
        let real_path = tmp.path().join("out.txt");

        let fs = ReadWriteFs::new();
        fs.write_file(&real_path, b"written").unwrap();
        assert_eq!(std::fs::read(&real_path).unwrap(), b"written");
    }

    // ======================================================================
    // Copy and rename
    // ======================================================================

    #[test]
    fn copy_file() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/src.txt"), b"data").unwrap();
        fs.copy(Path::new("/src.txt"), Path::new("/dst.txt"))
            .unwrap();
        assert_eq!(fs.read_file(Path::new("/dst.txt")).unwrap(), b"data");
        // Original still exists
        assert!(fs.exists(Path::new("/src.txt")));
    }

    #[test]
    fn rename_file() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/old.txt"), b"data").unwrap();
        fs.rename(Path::new("/old.txt"), Path::new("/new.txt"))
            .unwrap();
        assert!(!fs.exists(Path::new("/old.txt")));
        assert_eq!(fs.read_file(Path::new("/new.txt")).unwrap(), b"data");
    }

    // ======================================================================
    // Readdir reports correct node types
    // ======================================================================

    #[test]
    fn readdir_reports_node_types() {
        let (_tmp, fs) = rooted_fs();
        fs.write_file(Path::new("/file.txt"), b"").unwrap();
        fs.mkdir(Path::new("/dir")).unwrap();
        fs.symlink(Path::new("/file.txt"), Path::new("/link"))
            .unwrap();

        let entries = fs.readdir(Path::new("/")).unwrap();
        let find = |name: &str| entries.iter().find(|e| e.name == name).unwrap().node_type;
        assert_eq!(find("file.txt"), NodeType::File);
        assert_eq!(find("dir"), NodeType::Directory);
        assert_eq!(find("link"), NodeType::Symlink);
    }

    // ======================================================================
    // Symlink escape via read_file / stat (not just canonicalize)
    // ======================================================================

    #[test]
    fn read_file_through_symlink_escape_rejected() {
        let (tmp, fs) = rooted_fs();
        let escape_link = tmp.path().join("escape");
        std::os::unix::fs::symlink("/etc/hostname", &escape_link).unwrap();

        let err = fs.read_file(Path::new("/escape")).unwrap_err();
        assert!(
            matches!(err, VfsError::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[test]
    fn stat_through_symlink_escape_rejected() {
        let (tmp, fs) = rooted_fs();
        let escape_link = tmp.path().join("escape");
        std::os::unix::fs::symlink("/etc", &escape_link).unwrap();

        let err = fs.stat(Path::new("/escape")).unwrap_err();
        assert!(
            matches!(err, VfsError::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[test]
    fn relative_symlink_escape_rejected() {
        let (tmp, fs) = rooted_fs();
        fs.mkdir(Path::new("/sub")).unwrap();
        // Create a relative symlink that escapes: enough ../ to reach /etc
        let escape_link = tmp.path().join("sub/link");
        std::os::unix::fs::symlink("../../../../../../../../etc", &escape_link).unwrap();

        let err = fs.canonicalize(Path::new("/sub/link")).unwrap_err();
        assert!(
            matches!(err, VfsError::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }
}
