#![cfg(feature = "native-fs")]
//! Integration tests for filesystem backends through the `RustBash` API.
//!
//! These tests exercise OverlayFs, ReadWriteFs, and MountableFs end-to-end,
//! verifying that each backend works correctly when wired into the shell
//! via `RustBashBuilder::fs()`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rust_bash::{
    InMemoryFs, MountableFs, OverlayFs, ReadWriteFs, RustBash, RustBashBuilder, VirtualFs,
};

// ── Helpers ────────────────────────────────────────────────────────

fn assert_stdout(shell: &mut RustBash, cmd: &str, expected: &str) {
    let r = shell.exec(cmd).unwrap();
    assert_eq!(
        r.stdout, expected,
        "command `{cmd}` produced unexpected stdout"
    );
}

// ── OverlayFs integration ──────────────────────────────────────────

#[test]
fn overlay_reads_from_real_directory() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), b"disk content\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "cat /hello.txt", "disk content\n");
}

#[test]
fn overlay_writes_stay_in_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("data.txt");
    std::fs::write(&file, b"original\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo modified > /data.txt").unwrap();
    assert_stdout(&mut shell, "cat /data.txt", "modified\n");

    // Disk file unchanged
    let disk = std::fs::read_to_string(&file).unwrap();
    assert_eq!(disk, "original\n");
}

#[test]
fn overlay_new_files_do_not_touch_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo hello > /newfile.txt").unwrap();
    assert_stdout(&mut shell, "cat /newfile.txt", "hello\n");

    assert!(!tmp.path().join("newfile.txt").exists());
}

#[test]
fn overlay_delete_hides_lower_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("gone.txt"), b"bye").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("rm /gone.txt").unwrap();
    let r = shell.exec("cat /gone.txt").unwrap();
    assert_ne!(r.exit_code, 0);

    // Disk file still exists
    assert!(tmp.path().join("gone.txt").exists());
}

#[test]
fn overlay_mkdir_and_write_in_new_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell
        .exec("mkdir -p /sub/dir && echo ok > /sub/dir/f.txt")
        .unwrap();
    assert_stdout(&mut shell, "cat /sub/dir/f.txt", "ok\n");
    assert!(!tmp.path().join("sub").exists());
}

#[test]
fn overlay_append_copies_up_from_lower() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("log.txt"), b"line1\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo line2 >> /log.txt").unwrap();
    assert_stdout(&mut shell, "cat /log.txt", "line1\nline2\n");

    // Disk file unchanged
    let disk = std::fs::read_to_string(tmp.path().join("log.txt")).unwrap();
    assert_eq!(disk, "line1\n");
}

#[test]
fn overlay_ls_merges_layers() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("lower.txt"), b"lo").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo hi > /upper.txt").unwrap();
    let r = shell.exec("ls / | sort").unwrap();
    assert!(r.stdout.contains("lower.txt"), "missing lower.txt in ls");
    assert!(r.stdout.contains("upper.txt"), "missing upper.txt in ls");
}

#[test]
fn overlay_pipeline_with_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("words.txt"), b"apple\nbanana\ncherry\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "cat /words.txt | wc -l | tr -d ' '", "3\n");
    assert_stdout(&mut shell, "grep an /words.txt", "banana\n");
}

// ── ReadWriteFs integration ────────────────────────────────────────

#[test]
fn readwrite_reads_and_writes_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("home")).unwrap();

    let rwfs = ReadWriteFs::with_root(&root).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(rwfs))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo real > /home/test.txt").unwrap();
    assert_stdout(&mut shell, "cat /home/test.txt", "real\n");

    // Verify it actually hit disk
    let disk = std::fs::read_to_string(root.join("home/test.txt")).unwrap();
    assert_eq!(disk, "real\n");
}

#[test]
fn readwrite_restricted_root_prevents_escape() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();

    let rwfs = ReadWriteFs::with_root(&root).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(rwfs))
        .cwd("/")
        .build()
        .unwrap();

    let r = shell.exec("cat /../../../etc/passwd").unwrap();
    assert_ne!(r.exit_code, 0, "should fail to escape root");
}

#[test]
fn readwrite_pipeline_with_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::write(root.join("nums.txt"), b"3\n1\n2\n").unwrap();

    let rwfs = ReadWriteFs::with_root(&root).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(rwfs))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "sort /nums.txt", "1\n2\n3\n");
}

#[test]
fn readwrite_subshell_writes_are_visible() {
    // ReadWriteFs has no subshell isolation — writes go to the real FS
    // and are visible in the parent shell.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let rwfs = ReadWriteFs::with_root(&root).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(rwfs))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo before > /rw.txt").unwrap();
    shell.exec("(echo after > /rw.txt)").unwrap();
    // Unlike OverlayFs/MountableFs, ReadWriteFs subshell writes ARE visible
    assert_stdout(&mut shell, "cat /rw.txt", "after\n");
}

// ── MountableFs integration ────────────────────────────────────────

#[test]
fn mountable_routes_to_correct_backend() {
    let mountable = MountableFs::new().mount("/", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo hello > /file.txt").unwrap();
    assert_stdout(&mut shell, "cat /file.txt", "hello\n");
}

#[test]
fn mountable_overlay_at_mount_point() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("real.txt"), b"from disk\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mountable = MountableFs::new()
        .mount("/", Arc::new(InMemoryFs::new()))
        .mount("/project", Arc::new(overlay));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "cat /project/real.txt", "from disk\n");

    // Write through mount stays in memory
    shell.exec("echo overlay > /project/real.txt").unwrap();
    let disk = std::fs::read_to_string(tmp.path().join("real.txt")).unwrap();
    assert_eq!(disk, "from disk\n");
}

#[test]
fn mountable_multiple_backends() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("src.rs"), b"fn main() {}\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mountable = MountableFs::new()
        .mount("/", Arc::new(InMemoryFs::new()))
        .mount("/project", Arc::new(overlay))
        .mount("/tmp", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    // InMemory root
    shell.exec("echo root > /root.txt").unwrap();
    assert_stdout(&mut shell, "cat /root.txt", "root\n");

    // Overlay /project
    assert_stdout(&mut shell, "cat /project/src.rs", "fn main() {}\n");

    // Separate InMemory /tmp
    shell.exec("echo temp > /tmp/work.txt").unwrap();
    assert_stdout(&mut shell, "cat /tmp/work.txt", "temp\n");
}

#[test]
fn mountable_cross_mount_copy() {
    let mountable = MountableFs::new()
        .mount("/", Arc::new(InMemoryFs::new()))
        .mount("/other", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo data > /file.txt").unwrap();
    shell.exec("cp /file.txt /other/copy.txt").unwrap();
    assert_stdout(&mut shell, "cat /other/copy.txt", "data\n");
}

#[test]
fn mountable_readwrite_at_mount_point() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/info.txt"), b"real info\n").unwrap();

    let rwfs = ReadWriteFs::with_root(&root).unwrap();
    let mountable = MountableFs::new()
        .mount("/", Arc::new(InMemoryFs::new()))
        .mount("/real", Arc::new(rwfs));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "cat /real/data/info.txt", "real info\n");
}

#[test]
fn mountable_with_builder_files() {
    let mountable = MountableFs::new().mount("/", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .files(HashMap::from([(
            "/seed.txt".to_string(),
            b"seeded\n".to_vec(),
        )]))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "cat /seed.txt", "seeded\n");
}

#[test]
fn mountable_env_and_cwd() {
    let mountable = MountableFs::new().mount("/", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .env(HashMap::from([(
            "MYVAR".to_string(),
            "value123".to_string(),
        )]))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("mkdir -p /work && cd /work").unwrap();
    assert_stdout(&mut shell, "echo $MYVAR", "value123\n");
    assert_stdout(&mut shell, "pwd", "/work\n");
}

// ── Subshell isolation tests ───────────────────────────────────────

#[test]
fn subshell_writes_dont_leak_inmemoryfs() {
    let mut shell = RustBashBuilder::new().cwd("/").build().unwrap();

    shell.exec("echo before > /file.txt").unwrap();
    shell.exec("(echo after > /file.txt)").unwrap();
    let r = shell.exec("cat /file.txt").unwrap();
    assert_eq!(r.stdout, "before\n");
}

#[test]
fn subshell_writes_dont_leak_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("base.txt"), b"base\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo before > /file.txt").unwrap();
    shell.exec("(echo after > /file.txt)").unwrap();
    assert_stdout(&mut shell, "cat /file.txt", "before\n");
}

#[test]
fn subshell_writes_dont_leak_mountable() {
    let mountable = MountableFs::new().mount("/", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo before > /file.txt").unwrap();
    shell.exec("(echo after > /file.txt)").unwrap();
    assert_stdout(&mut shell, "cat /file.txt", "before\n");
}

#[test]
fn subshell_new_files_dont_leak_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("(echo secret > /leak.txt)").unwrap();
    let r = shell.exec("cat /leak.txt").unwrap();
    assert_ne!(r.exit_code, 0, "subshell file should not leak");
}

#[test]
fn subshell_new_files_dont_leak_mountable() {
    let mountable = MountableFs::new().mount("/", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("(echo secret > /leak.txt)").unwrap();
    let r = shell.exec("cat /leak.txt").unwrap();
    assert_ne!(r.exit_code, 0, "subshell file should not leak");
}

#[test]
fn subshell_deletes_dont_leak_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("keep.txt"), b"keep\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("(rm /keep.txt)").unwrap();
    assert_stdout(&mut shell, "cat /keep.txt", "keep\n");
}

#[test]
fn subshell_deletes_dont_leak_mountable() {
    let mountable = MountableFs::new().mount("/", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo keep > /keep.txt").unwrap();
    shell.exec("(rm /keep.txt)").unwrap();
    assert_stdout(&mut shell, "cat /keep.txt", "keep\n");
}

#[test]
fn command_substitution_isolation_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo original > /cs.txt").unwrap();
    shell
        .exec("X=$(echo modified > /cs.txt && cat /cs.txt)")
        .unwrap();
    assert_stdout(&mut shell, "cat /cs.txt", "original\n");
}

#[test]
fn command_substitution_isolation_mountable() {
    let mountable = MountableFs::new().mount("/", Arc::new(InMemoryFs::new()));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    shell.exec("echo original > /cs.txt").unwrap();
    shell
        .exec("X=$(echo modified > /cs.txt && cat /cs.txt)")
        .unwrap();
    assert_stdout(&mut shell, "cat /cs.txt", "original\n");
}

#[test]
fn subshell_isolation_with_mountable_overlay_mount() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("project.txt"), b"disk\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mountable = MountableFs::new()
        .mount("/", Arc::new(InMemoryFs::new()))
        .mount("/project", Arc::new(overlay));
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(mountable))
        .cwd("/")
        .build()
        .unwrap();

    // Write through mountable to overlay mount
    shell.exec("echo parent > /project/file.txt").unwrap();
    shell.exec("(echo child > /project/file.txt)").unwrap();
    assert_stdout(&mut shell, "cat /project/file.txt", "parent\n");

    // Disk untouched
    let disk = std::fs::read_to_string(tmp.path().join("project.txt")).unwrap();
    assert_eq!(disk, "disk\n");
}

// ── Builder .files() with custom backends ──────────────────────────

#[test]
fn builder_files_work_with_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .files(HashMap::from([(
            "/seed/data.txt".to_string(),
            b"seeded content\n".to_vec(),
        )]))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "cat /seed/data.txt", "seeded content\n");
    // Seed file only in memory, not on disk
    assert!(!tmp.path().join("seed/data.txt").exists());
}

#[test]
fn builder_files_work_with_readwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let rwfs = ReadWriteFs::with_root(&root).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(rwfs))
        .files(HashMap::from([(
            "/init.txt".to_string(),
            b"initialized\n".to_vec(),
        )]))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(&mut shell, "cat /init.txt", "initialized\n");
    // ReadWriteFs writes to disk
    let disk = std::fs::read_to_string(root.join("init.txt")).unwrap();
    assert_eq!(disk, "initialized\n");
}

// ── Control flow with backends ─────────────────────────────────────

#[test]
fn for_loop_with_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"A\n").unwrap();
    std::fs::write(tmp.path().join("b.txt"), b"B\n").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    let r = shell
        .exec("for f in /a.txt /b.txt; do cat $f; done")
        .unwrap();
    assert_eq!(r.stdout, "A\nB\n");
}

#[test]
fn if_test_with_overlay_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("exists.txt"), b"yes").unwrap();

    let overlay = OverlayFs::new(tmp.path()).unwrap();
    let mut shell = RustBashBuilder::new()
        .fs(Arc::new(overlay))
        .cwd("/")
        .build()
        .unwrap();

    assert_stdout(
        &mut shell,
        "if [ -f /exists.txt ]; then echo found; fi",
        "found\n",
    );
    assert_stdout(
        &mut shell,
        "if [ -f /nope.txt ]; then echo found; else echo missing; fi",
        "missing\n",
    );
}

// ── deep_clone via the public VirtualFs trait ──────────────────────

#[test]
fn overlay_deep_clone_is_independent() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("base.txt"), b"base\n").unwrap();

    let overlay = Arc::new(OverlayFs::new(tmp.path()).unwrap());
    overlay
        .write_file(Path::new("/upper.txt"), b"original")
        .unwrap();

    let cloned = overlay.deep_clone();
    cloned
        .write_file(Path::new("/upper.txt"), b"modified")
        .unwrap();

    // Original unchanged
    let content = overlay.read_file(Path::new("/upper.txt")).unwrap();
    assert_eq!(content, b"original");
}

#[test]
fn mountable_deep_clone_is_independent() {
    let mountable = Arc::new(MountableFs::new().mount("/", Arc::new(InMemoryFs::new())));
    mountable
        .write_file(Path::new("/file.txt"), b"original")
        .unwrap();

    let cloned = mountable.deep_clone();
    cloned
        .write_file(Path::new("/file.txt"), b"modified")
        .unwrap();

    let content = mountable.read_file(Path::new("/file.txt")).unwrap();
    assert_eq!(content, b"original");
}
