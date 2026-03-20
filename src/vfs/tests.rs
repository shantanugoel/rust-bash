use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{InMemoryFs, NodeType, VirtualFs};
use crate::error::VfsError;

fn fs() -> InMemoryFs {
    InMemoryFs::new()
}

// ==========================================================================
// File CRUD
// ==========================================================================

#[test]
fn write_and_read_file() {
    let fs = fs();
    fs.write_file(Path::new("/hello.txt"), b"Hello, world!")
        .unwrap();
    let content = fs.read_file(Path::new("/hello.txt")).unwrap();
    assert_eq!(content, b"Hello, world!");
}

#[test]
fn overwrite_file() {
    let fs = fs();
    fs.write_file(Path::new("/f.txt"), b"first").unwrap();
    fs.write_file(Path::new("/f.txt"), b"second").unwrap();
    assert_eq!(fs.read_file(Path::new("/f.txt")).unwrap(), b"second");
}

#[test]
fn append_file() {
    let fs = fs();
    fs.write_file(Path::new("/f.txt"), b"hello").unwrap();
    fs.append_file(Path::new("/f.txt"), b" world").unwrap();
    assert_eq!(fs.read_file(Path::new("/f.txt")).unwrap(), b"hello world");
}

#[test]
fn append_nonexistent_file_errors() {
    let fs = fs();
    let err = fs.append_file(Path::new("/nope.txt"), b"data").unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn remove_file() {
    let fs = fs();
    fs.write_file(Path::new("/f.txt"), b"data").unwrap();
    fs.remove_file(Path::new("/f.txt")).unwrap();
    assert!(!fs.exists(Path::new("/f.txt")));
}

#[test]
fn remove_nonexistent_file_errors() {
    let fs = fs();
    let err = fs.remove_file(Path::new("/nope.txt")).unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn read_nonexistent_file_errors() {
    let fs = fs();
    let err = fs.read_file(Path::new("/nope.txt")).unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn read_directory_as_file_errors() {
    let fs = fs();
    fs.mkdir(Path::new("/dir")).unwrap();
    let err = fs.read_file(Path::new("/dir")).unwrap_err();
    assert!(matches!(err, VfsError::IsADirectory(_)));
}

#[test]
fn write_file_in_nested_dir() {
    let fs = fs();
    fs.mkdir_p(Path::new("/a/b/c")).unwrap();
    fs.write_file(Path::new("/a/b/c/file.txt"), b"nested")
        .unwrap();
    assert_eq!(
        fs.read_file(Path::new("/a/b/c/file.txt")).unwrap(),
        b"nested"
    );
}

#[test]
fn write_file_parent_not_found_errors() {
    let fs = fs();
    let err = fs
        .write_file(Path::new("/no/such/dir/file.txt"), b"data")
        .unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

// ==========================================================================
// Directory operations
// ==========================================================================

#[test]
fn mkdir_and_readdir() {
    let fs = fs();
    fs.mkdir(Path::new("/mydir")).unwrap();
    let entries = fs.readdir(Path::new("/")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "mydir");
    assert_eq!(entries[0].node_type, NodeType::Directory);
}

#[test]
fn mkdir_already_exists_errors() {
    let fs = fs();
    fs.mkdir(Path::new("/mydir")).unwrap();
    let err = fs.mkdir(Path::new("/mydir")).unwrap_err();
    assert!(matches!(err, VfsError::AlreadyExists(_)));
}

#[test]
fn mkdir_p_creates_parents() {
    let fs = fs();
    fs.mkdir_p(Path::new("/a/b/c")).unwrap();
    assert!(fs.exists(Path::new("/a")));
    assert!(fs.exists(Path::new("/a/b")));
    assert!(fs.exists(Path::new("/a/b/c")));
}

#[test]
fn mkdir_p_existing_is_ok() {
    let fs = fs();
    fs.mkdir_p(Path::new("/a/b")).unwrap();
    fs.mkdir_p(Path::new("/a/b")).unwrap(); // should not error
    assert!(fs.exists(Path::new("/a/b")));
}

#[test]
fn mkdir_p_over_file_errors() {
    let fs = fs();
    fs.write_file(Path::new("/file"), b"data").unwrap();
    let err = fs.mkdir_p(Path::new("/file/sub")).unwrap_err();
    assert!(matches!(err, VfsError::NotADirectory(_)));
}

#[test]
fn readdir_lists_sorted() {
    let fs = fs();
    fs.write_file(Path::new("/c.txt"), b"").unwrap();
    fs.write_file(Path::new("/a.txt"), b"").unwrap();
    fs.write_file(Path::new("/b.txt"), b"").unwrap();
    let entries = fs.readdir(Path::new("/")).unwrap();
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn remove_dir_empty() {
    let fs = fs();
    fs.mkdir(Path::new("/dir")).unwrap();
    fs.remove_dir(Path::new("/dir")).unwrap();
    assert!(!fs.exists(Path::new("/dir")));
}

#[test]
fn remove_dir_not_empty_errors() {
    let fs = fs();
    fs.mkdir(Path::new("/dir")).unwrap();
    fs.write_file(Path::new("/dir/file"), b"").unwrap();
    let err = fs.remove_dir(Path::new("/dir")).unwrap_err();
    assert!(matches!(err, VfsError::DirectoryNotEmpty(_)));
}

#[test]
fn remove_dir_on_file_errors() {
    let fs = fs();
    fs.write_file(Path::new("/file"), b"").unwrap();
    let err = fs.remove_dir(Path::new("/file")).unwrap_err();
    assert!(matches!(err, VfsError::NotADirectory(_)));
}

#[test]
fn remove_dir_all_recursive() {
    let fs = fs();
    fs.mkdir_p(Path::new("/a/b/c")).unwrap();
    fs.write_file(Path::new("/a/b/file.txt"), b"data").unwrap();
    fs.write_file(Path::new("/a/b/c/deep.txt"), b"deep")
        .unwrap();
    fs.remove_dir_all(Path::new("/a")).unwrap();
    assert!(!fs.exists(Path::new("/a")));
}

#[test]
fn remove_dir_all_nonexistent_errors() {
    let fs = fs();
    let err = fs.remove_dir_all(Path::new("/nope")).unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn readdir_nonexistent_errors() {
    let fs = fs();
    let err = fs.readdir(Path::new("/nope")).unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn readdir_on_file_errors() {
    let fs = fs();
    fs.write_file(Path::new("/file"), b"").unwrap();
    let err = fs.readdir(Path::new("/file")).unwrap_err();
    assert!(matches!(err, VfsError::NotADirectory(_)));
}

// ==========================================================================
// Path normalization
// ==========================================================================

#[test]
fn normalize_dot_and_dotdot() {
    let fs = fs();
    fs.mkdir_p(Path::new("/a/b")).unwrap();
    fs.write_file(Path::new("/a/b/file.txt"), b"data").unwrap();
    // Access through . and ..
    let content = fs.read_file(Path::new("/a/./b/../b/./file.txt")).unwrap();
    assert_eq!(content, b"data");
}

#[test]
fn normalize_trailing_slash() {
    let fs = fs();
    fs.mkdir(Path::new("/dir")).unwrap();
    assert!(fs.exists(Path::new("/dir/")));
}

#[test]
fn reject_empty_path() {
    let fs = fs();
    let err = fs.read_file(Path::new("")).unwrap_err();
    assert!(matches!(err, VfsError::InvalidPath(_)));
}

#[test]
fn reject_relative_path() {
    let fs = fs();
    let err = fs.read_file(Path::new("relative/path")).unwrap_err();
    assert!(matches!(err, VfsError::InvalidPath(_)));
}

#[test]
fn dotdot_at_root_stays_at_root() {
    let fs = fs();
    fs.write_file(Path::new("/file.txt"), b"root").unwrap();
    let content = fs.read_file(Path::new("/../../../file.txt")).unwrap();
    assert_eq!(content, b"root");
}

// ==========================================================================
// Symlinks
// ==========================================================================

#[test]
fn symlink_read_through() {
    let fs = fs();
    fs.write_file(Path::new("/target.txt"), b"real content")
        .unwrap();
    fs.symlink(Path::new("/target.txt"), Path::new("/link.txt"))
        .unwrap();
    let content = fs.read_file(Path::new("/link.txt")).unwrap();
    assert_eq!(content, b"real content");
}

#[test]
fn symlink_readlink() {
    let fs = fs();
    fs.write_file(Path::new("/target.txt"), b"").unwrap();
    fs.symlink(Path::new("/target.txt"), Path::new("/link.txt"))
        .unwrap();
    let target = fs.readlink(Path::new("/link.txt")).unwrap();
    assert_eq!(target, Path::new("/target.txt"));
}

#[test]
fn symlink_lstat_returns_symlink_type() {
    let fs = fs();
    fs.write_file(Path::new("/target.txt"), b"").unwrap();
    fs.symlink(Path::new("/target.txt"), Path::new("/link.txt"))
        .unwrap();
    let meta = fs.lstat(Path::new("/link.txt")).unwrap();
    assert_eq!(meta.node_type, NodeType::Symlink);
}

#[test]
fn symlink_stat_returns_target_type() {
    let fs = fs();
    fs.write_file(Path::new("/target.txt"), b"data").unwrap();
    fs.symlink(Path::new("/target.txt"), Path::new("/link.txt"))
        .unwrap();
    let meta = fs.stat(Path::new("/link.txt")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
}

#[test]
fn symlink_chain() {
    let fs = fs();
    fs.write_file(Path::new("/real.txt"), b"chain").unwrap();
    fs.symlink(Path::new("/real.txt"), Path::new("/link1"))
        .unwrap();
    fs.symlink(Path::new("/link1"), Path::new("/link2"))
        .unwrap();
    let content = fs.read_file(Path::new("/link2")).unwrap();
    assert_eq!(content, b"chain");
}

#[test]
fn symlink_loop_detected() {
    let fs = fs();
    fs.symlink(Path::new("/b"), Path::new("/a")).unwrap();
    fs.symlink(Path::new("/a"), Path::new("/b")).unwrap();
    let err = fs.read_file(Path::new("/a")).unwrap_err();
    assert!(matches!(err, VfsError::SymlinkLoop(_)));
}

#[test]
fn symlink_to_nonexistent_errors_on_read() {
    let fs = fs();
    fs.symlink(Path::new("/nonexistent"), Path::new("/link"))
        .unwrap();
    let err = fs.read_file(Path::new("/link")).unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn remove_file_removes_symlink_not_target() {
    let fs = fs();
    fs.write_file(Path::new("/target"), b"keep me").unwrap();
    fs.symlink(Path::new("/target"), Path::new("/link"))
        .unwrap();
    fs.remove_file(Path::new("/link")).unwrap();
    assert!(!fs.exists(Path::new("/link")));
    assert_eq!(fs.read_file(Path::new("/target")).unwrap(), b"keep me");
}

#[test]
fn symlink_already_exists_errors() {
    let fs = fs();
    fs.write_file(Path::new("/file"), b"").unwrap();
    let err = fs
        .symlink(Path::new("/target"), Path::new("/file"))
        .unwrap_err();
    assert!(matches!(err, VfsError::AlreadyExists(_)));
}

// ==========================================================================
// Metadata: mode, mtime
// ==========================================================================

#[test]
fn file_default_mode() {
    let fs = fs();
    fs.write_file(Path::new("/f.txt"), b"").unwrap();
    let meta = fs.stat(Path::new("/f.txt")).unwrap();
    assert_eq!(meta.mode, 0o644);
}

#[test]
fn dir_default_mode() {
    let fs = fs();
    fs.mkdir(Path::new("/dir")).unwrap();
    let meta = fs.stat(Path::new("/dir")).unwrap();
    assert_eq!(meta.mode, 0o755);
}

#[test]
fn chmod_changes_mode() {
    let fs = fs();
    fs.write_file(Path::new("/f.txt"), b"").unwrap();
    fs.chmod(Path::new("/f.txt"), 0o755).unwrap();
    let meta = fs.stat(Path::new("/f.txt")).unwrap();
    assert_eq!(meta.mode, 0o755);
}

#[test]
fn utimes_changes_mtime() {
    let fs = fs();
    fs.write_file(Path::new("/f.txt"), b"").unwrap();
    let new_time = SystemTime::UNIX_EPOCH;
    fs.utimes(Path::new("/f.txt"), new_time).unwrap();
    let meta = fs.stat(Path::new("/f.txt")).unwrap();
    assert_eq!(meta.mtime, SystemTime::UNIX_EPOCH);
}

#[test]
fn file_size_in_metadata() {
    let fs = fs();
    fs.write_file(Path::new("/f.txt"), b"12345").unwrap();
    let meta = fs.stat(Path::new("/f.txt")).unwrap();
    assert_eq!(meta.size, 5);
}

// ==========================================================================
// Copy, rename, hardlink
// ==========================================================================

#[test]
fn copy_file() {
    let fs = fs();
    fs.write_file(Path::new("/src.txt"), b"copy me").unwrap();
    fs.chmod(Path::new("/src.txt"), 0o700).unwrap();
    fs.copy(Path::new("/src.txt"), Path::new("/dst.txt"))
        .unwrap();
    assert_eq!(fs.read_file(Path::new("/dst.txt")).unwrap(), b"copy me");
    let meta = fs.stat(Path::new("/dst.txt")).unwrap();
    assert_eq!(meta.mode, 0o700);
}

#[test]
fn rename_file() {
    let fs = fs();
    fs.write_file(Path::new("/old.txt"), b"data").unwrap();
    fs.rename(Path::new("/old.txt"), Path::new("/new.txt"))
        .unwrap();
    assert!(!fs.exists(Path::new("/old.txt")));
    assert_eq!(fs.read_file(Path::new("/new.txt")).unwrap(), b"data");
}

#[test]
fn rename_directory() {
    let fs = fs();
    fs.mkdir(Path::new("/olddir")).unwrap();
    fs.write_file(Path::new("/olddir/file.txt"), b"inside")
        .unwrap();
    fs.rename(Path::new("/olddir"), Path::new("/newdir"))
        .unwrap();
    assert!(!fs.exists(Path::new("/olddir")));
    assert_eq!(
        fs.read_file(Path::new("/newdir/file.txt")).unwrap(),
        b"inside"
    );
}

#[test]
fn hardlink_creates_copy() {
    let fs = fs();
    fs.write_file(Path::new("/src.txt"), b"linked").unwrap();
    fs.hardlink(Path::new("/src.txt"), Path::new("/dst.txt"))
        .unwrap();
    assert_eq!(fs.read_file(Path::new("/dst.txt")).unwrap(), b"linked");
}

// ==========================================================================
// Canonicalize
// ==========================================================================

#[test]
fn canonicalize_resolves_symlinks() {
    let fs = fs();
    fs.mkdir(Path::new("/real")).unwrap();
    fs.write_file(Path::new("/real/file.txt"), b"").unwrap();
    fs.symlink(Path::new("/real"), Path::new("/link")).unwrap();
    let canon = fs.canonicalize(Path::new("/link/file.txt")).unwrap();
    assert_eq!(canon, Path::new("/real/file.txt"));
}

#[test]
fn canonicalize_root() {
    let fs = fs();
    let canon = fs.canonicalize(Path::new("/")).unwrap();
    assert_eq!(canon, Path::new("/"));
}

#[test]
fn canonicalize_dotdot() {
    let fs = fs();
    fs.mkdir_p(Path::new("/a/b")).unwrap();
    let canon = fs.canonicalize(Path::new("/a/b/..")).unwrap();
    assert_eq!(canon, Path::new("/a"));
}

// ==========================================================================
// Glob stub
// ==========================================================================

#[test]
fn glob_basic_matching() {
    let fs = fs();
    fs.write_file(Path::new("/a.txt"), b"hello").unwrap();
    fs.write_file(Path::new("/b.txt"), b"world").unwrap();
    fs.write_file(Path::new("/c.md"), b"readme").unwrap();
    let result = fs.glob("*.txt", Path::new("/")).unwrap();
    assert_eq!(result, vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]);
}

#[test]
fn glob_no_match_returns_empty() {
    let fs = fs();
    let result = fs.glob("*.xyz", Path::new("/")).unwrap();
    assert!(result.is_empty());
}

#[test]
fn glob_absolute_pattern() {
    let fs = fs();
    fs.mkdir(Path::new("/dir")).unwrap();
    fs.write_file(Path::new("/dir/f1.log"), b"").unwrap();
    fs.write_file(Path::new("/dir/f2.log"), b"").unwrap();
    let result = fs.glob("/dir/*.log", Path::new("/")).unwrap();
    assert_eq!(
        result,
        vec![PathBuf::from("/dir/f1.log"), PathBuf::from("/dir/f2.log")]
    );
}

#[test]
fn glob_question_mark_pattern() {
    let fs = fs();
    fs.write_file(Path::new("/a.txt"), b"").unwrap();
    fs.write_file(Path::new("/bb.txt"), b"").unwrap();
    let result = fs.glob("?.txt", Path::new("/")).unwrap();
    assert_eq!(result, vec![PathBuf::from("a.txt")]);
}

#[test]
fn glob_bracket_pattern() {
    let fs = fs();
    fs.write_file(Path::new("/a.txt"), b"").unwrap();
    fs.write_file(Path::new("/b.txt"), b"").unwrap();
    fs.write_file(Path::new("/c.txt"), b"").unwrap();
    let result = fs.glob("[ab].txt", Path::new("/")).unwrap();
    assert_eq!(result, vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]);
}

#[test]
fn glob_recursive_doublestar() {
    let fs = fs();
    fs.mkdir_p(Path::new("/proj/src/sub")).unwrap();
    fs.write_file(Path::new("/proj/README.md"), b"").unwrap();
    fs.write_file(Path::new("/proj/src/lib.md"), b"").unwrap();
    fs.write_file(Path::new("/proj/src/sub/deep.md"), b"")
        .unwrap();
    let result = fs.glob("/proj/**/*.md", Path::new("/")).unwrap();
    assert_eq!(
        result,
        vec![
            PathBuf::from("/proj/README.md"),
            PathBuf::from("/proj/src/lib.md"),
            PathBuf::from("/proj/src/sub/deep.md"),
        ]
    );
}

#[test]
fn glob_hidden_files_skipped() {
    let fs = fs();
    fs.write_file(Path::new("/.hidden"), b"").unwrap();
    fs.write_file(Path::new("/visible"), b"").unwrap();
    let result = fs.glob("*", Path::new("/")).unwrap();
    assert_eq!(result, vec![PathBuf::from("visible")]);
}

#[test]
fn glob_hidden_files_explicit_dot() {
    let fs = fs();
    fs.write_file(Path::new("/.a"), b"").unwrap();
    fs.write_file(Path::new("/.b"), b"").unwrap();
    fs.write_file(Path::new("/c"), b"").unwrap();
    let result = fs.glob(".*", Path::new("/")).unwrap();
    assert_eq!(result, vec![PathBuf::from(".a"), PathBuf::from(".b")]);
}

#[test]
fn glob_through_symlink_dir() {
    let fs = fs();
    fs.mkdir_p(Path::new("/real/sub")).unwrap();
    fs.write_file(Path::new("/real/sub/file.txt"), b"").unwrap();
    fs.symlink(Path::new("/real"), Path::new("/link")).unwrap();
    let result = fs.glob("/link/*/*.txt", Path::new("/")).unwrap();
    assert_eq!(result, vec![PathBuf::from("/link/sub/file.txt")]);
}

#[test]
fn glob_doublestar_skips_hidden() {
    let fs = fs();
    fs.mkdir_p(Path::new("/d/.hidden")).unwrap();
    fs.write_file(Path::new("/d/.hidden/secret.md"), b"")
        .unwrap();
    fs.write_file(Path::new("/d/visible.md"), b"").unwrap();
    let result = fs.glob("/d/**/*.md", Path::new("/")).unwrap();
    assert_eq!(result, vec![PathBuf::from("/d/visible.md")]);
}

// ==========================================================================
// Exists
// ==========================================================================

#[test]
fn exists_root() {
    let fs = fs();
    assert!(fs.exists(Path::new("/")));
}

#[test]
fn exists_nonexistent() {
    let fs = fs();
    assert!(!fs.exists(Path::new("/nope")));
}

#[test]
fn exists_through_symlink() {
    let fs = fs();
    fs.write_file(Path::new("/real"), b"").unwrap();
    fs.symlink(Path::new("/real"), Path::new("/link")).unwrap();
    assert!(fs.exists(Path::new("/link")));
}

#[test]
fn exists_dangling_symlink_false() {
    let fs = fs();
    fs.symlink(Path::new("/nonexistent"), Path::new("/link"))
        .unwrap();
    assert!(!fs.exists(Path::new("/link")));
}

// ==========================================================================
// Edge cases
// ==========================================================================

#[test]
fn write_empty_file() {
    let fs = fs();
    fs.write_file(Path::new("/empty"), b"").unwrap();
    assert_eq!(fs.read_file(Path::new("/empty")).unwrap(), b"");
    assert_eq!(fs.stat(Path::new("/empty")).unwrap().size, 0);
}

#[test]
fn binary_file_content() {
    let fs = fs();
    let data: Vec<u8> = (0..=255).collect();
    fs.write_file(Path::new("/binary"), &data).unwrap();
    assert_eq!(fs.read_file(Path::new("/binary")).unwrap(), data);
}

#[test]
fn remove_file_on_directory_errors() {
    let fs = fs();
    fs.mkdir(Path::new("/dir")).unwrap();
    let err = fs.remove_file(Path::new("/dir")).unwrap_err();
    assert!(matches!(err, VfsError::IsADirectory(_)));
}

#[test]
fn rename_nonexistent_errors() {
    let fs = fs();
    let err = fs
        .rename(Path::new("/nope"), Path::new("/dst"))
        .unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn hardlink_already_exists_errors() {
    let fs = fs();
    fs.write_file(Path::new("/src"), b"").unwrap();
    fs.write_file(Path::new("/dst"), b"").unwrap();
    let err = fs
        .hardlink(Path::new("/src"), Path::new("/dst"))
        .unwrap_err();
    assert!(matches!(err, VfsError::AlreadyExists(_)));
}

#[test]
fn readlink_on_non_symlink_errors() {
    let fs = fs();
    fs.write_file(Path::new("/file"), b"").unwrap();
    let err = fs.readlink(Path::new("/file")).unwrap_err();
    assert!(matches!(err, VfsError::InvalidPath(_)));
}

#[test]
fn chmod_nonexistent_errors() {
    let fs = fs();
    let err = fs.chmod(Path::new("/nope"), 0o644).unwrap_err();
    assert!(matches!(err, VfsError::NotFound(_)));
}

#[test]
fn remove_dir_all_on_file_errors() {
    let fs = fs();
    fs.write_file(Path::new("/file"), b"").unwrap();
    let err = fs.remove_dir_all(Path::new("/file")).unwrap_err();
    assert!(matches!(err, VfsError::NotADirectory(_)));
}

#[test]
fn symlink_to_directory() {
    let fs = fs();
    fs.mkdir(Path::new("/realdir")).unwrap();
    fs.write_file(Path::new("/realdir/file.txt"), b"hello")
        .unwrap();
    fs.symlink(Path::new("/realdir"), Path::new("/linkdir"))
        .unwrap();
    let content = fs.read_file(Path::new("/linkdir/file.txt")).unwrap();
    assert_eq!(content, b"hello");
}

#[test]
fn write_through_symlink() {
    let fs = fs();
    fs.write_file(Path::new("/real.txt"), b"original").unwrap();
    fs.symlink(Path::new("/real.txt"), Path::new("/link.txt"))
        .unwrap();
    fs.write_file(Path::new("/link.txt"), b"updated").unwrap();
    assert_eq!(fs.read_file(Path::new("/real.txt")).unwrap(), b"updated");
}

#[test]
fn trait_deep_clone_produces_independent_copy() {
    let fs = fs();
    fs.write_file(Path::new("/a.txt"), b"original").unwrap();

    let cloned: std::sync::Arc<dyn VirtualFs> = VirtualFs::deep_clone(&fs);
    cloned.write_file(Path::new("/a.txt"), b"modified").unwrap();
    cloned.write_file(Path::new("/new.txt"), b"new").unwrap();

    // Original is untouched
    assert_eq!(fs.read_file(Path::new("/a.txt")).unwrap(), b"original");
    assert!(!fs.exists(Path::new("/new.txt")));
}
