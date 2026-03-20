use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

fn rust_bash() -> Command {
    Command::cargo_bin("rust-bash").unwrap()
}

// ── Execution modes ─────────────────────────────────────────────────

#[test]
fn c_flag_echo_hello() {
    rust_bash()
        .args(["-c", "echo hello"])
        .assert()
        .success()
        .stdout("hello\n");
}

#[test]
fn c_flag_exit_code() {
    rust_bash().args(["-c", "exit 42"]).assert().code(42);
}

#[test]
fn c_flag_stderr_redirect() {
    rust_bash()
        .args(["-c", "echo err >&2"])
        .assert()
        .success()
        .stderr("err\n");
}

#[test]
fn script_file_execution() {
    let mut tmp = NamedTempFile::new().unwrap();
    writeln!(tmp, "echo hello").unwrap();
    tmp.flush().unwrap();

    rust_bash()
        .arg(tmp.path())
        .assert()
        .success()
        .stdout("hello\n");
}

#[test]
fn stdin_pipe_execution() {
    rust_bash()
        .write_stdin("echo hello\n")
        .assert()
        .success()
        .stdout("hello\n");
}

// ── JSON output ─────────────────────────────────────────────────────

#[test]
fn json_echo_hello() {
    let output = rust_bash()
        .args(["--json", "-c", "echo hello"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(json["stdout"], "hello\n");
    assert_eq!(json["stderr"], "");
    assert_eq!(json["exit_code"], 0);
}

#[test]
fn json_exit_nonzero() {
    let output = rust_bash()
        .args(["--json", "-c", "exit 1"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(json["exit_code"], 1);
}

#[test]
fn json_stderr_field() {
    let output = rust_bash()
        .args(["--json", "-c", "echo err >&2"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(json["stderr"], "err\n");
}

// ── File seeding ────────────────────────────────────────────────────

#[test]
fn files_single_file_mapping() {
    let mut tmp = NamedTempFile::new().unwrap();
    write!(tmp, "file contents").unwrap();
    tmp.flush().unwrap();

    let mapping = format!("{}:/seed.txt", tmp.path().display());

    rust_bash()
        .args(["--files", &mapping, "-c", "cat /seed.txt"])
        .assert()
        .success()
        .stdout("file contents");
}

#[test]
fn files_directory_seeded_at_root() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "a").unwrap();
    std::fs::write(dir.path().join("beta.txt"), "b").unwrap();

    let mapping = dir.path().to_str().unwrap();

    let output = rust_bash()
        .args(["--files", mapping, "-c", "ls /"])
        .output()
        .unwrap();

    assert!(output.status.success(), "process should succeed");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("alpha.txt"), "should list alpha.txt");
    assert!(stdout.contains("beta.txt"), "should list beta.txt");
}

#[test]
fn files_directory_with_vfs_prefix() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("data.txt"), "hello").unwrap();

    let mapping = format!("{}:/mydir", dir.path().display());

    rust_bash()
        .args(["--files", &mapping, "-c", "cat /mydir/data.txt"])
        .assert()
        .success()
        .stdout("hello");
}

#[test]
fn files_multiple_flags_combine() {
    let mut tmp1 = NamedTempFile::new().unwrap();
    write!(tmp1, "one").unwrap();
    tmp1.flush().unwrap();

    let mut tmp2 = NamedTempFile::new().unwrap();
    write!(tmp2, "two").unwrap();
    tmp2.flush().unwrap();

    let m1 = format!("{}:/a.txt", tmp1.path().display());
    let m2 = format!("{}:/b.txt", tmp2.path().display());

    rust_bash()
        .args([
            "--files",
            &m1,
            "--files",
            &m2,
            "-c",
            "cat /a.txt; cat /b.txt",
        ])
        .assert()
        .success()
        .stdout("onetwo");
}

// ── Environment & working directory ─────────────────────────────────

#[test]
fn env_single_variable() {
    rust_bash()
        .args(["--env", "FOO=bar", "-c", "echo $FOO"])
        .assert()
        .success()
        .stdout("bar\n");
}

#[test]
fn env_multiple_variables() {
    rust_bash()
        .args(["--env", "A=1", "--env", "B=2", "-c", "echo $A $B"])
        .assert()
        .success()
        .stdout("1 2\n");
}

#[test]
fn cwd_sets_working_directory() {
    rust_bash()
        .args(["--cwd", "/app", "-c", "pwd"])
        .assert()
        .success()
        .stdout("/app\n");
}

#[test]
fn cwd_env_and_files_combined() {
    let mut tmp = NamedTempFile::new().unwrap();
    writeln!(tmp, "data").unwrap();
    tmp.flush().unwrap();

    let mapping = format!("{}:/work/input.txt", tmp.path().display());

    rust_bash()
        .args([
            "--cwd",
            "/work",
            "--env",
            "GREETING=hi",
            "--files",
            &mapping,
            "-c",
            "echo $GREETING; cat /work/input.txt; pwd",
        ])
        .assert()
        .success()
        .stdout("hi\ndata\n/work\n");
}

// ── Error cases ─────────────────────────────────────────────────────

#[test]
fn nonexistent_script_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("no_such_script.sh");

    rust_bash()
        .arg(&path)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no_such_script"));
}

#[test]
fn invalid_env_format_no_equals() {
    rust_bash()
        .args(["--env", "BADFORMAT", "-c", "echo hi"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("KEY=VALUE"));
}

#[test]
fn nonexistent_files_host_path() {
    let dir = TempDir::new().unwrap();
    let bad_path = dir.path().join("no_such_file");
    let mapping = format!("{}:/dest", bad_path.display());

    rust_bash()
        .args(["--files", &mapping, "-c", "echo hi"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn c_flag_empty_string_is_noop() {
    rust_bash().args(["-c", ""]).assert().success().stdout("");
}

// ── Flag interactions ───────────────────────────────────────────────

#[test]
fn c_flag_takes_priority_over_script_arg() {
    let mut tmp = NamedTempFile::new().unwrap();
    writeln!(tmp, "echo from-file").unwrap();
    tmp.flush().unwrap();

    rust_bash()
        .args(["-c", "echo from-c"])
        .arg(tmp.path())
        .assert()
        .success()
        .stdout("from-c\n");
}

#[test]
fn c_flag_takes_priority_over_stdin() {
    rust_bash()
        .args(["-c", "echo from-c"])
        .write_stdin("echo from-stdin\n")
        .assert()
        .success()
        .stdout("from-c\n");
}

// ── Script positional arguments ─────────────────────────────────────

#[test]
fn script_positional_args() {
    let mut tmp = NamedTempFile::with_suffix(".sh").unwrap();
    writeln!(tmp, "echo $1 $2").unwrap();
    tmp.flush().unwrap();

    rust_bash()
        .arg(tmp.path())
        .args(["arg1", "arg2"])
        .assert()
        .success()
        .stdout("arg1 arg2\n");
}

#[test]
fn script_dollar_zero() {
    let mut tmp = NamedTempFile::with_suffix(".sh").unwrap();
    writeln!(tmp, "echo $0").unwrap();
    tmp.flush().unwrap();

    let path_str = tmp.path().to_str().unwrap().to_string();

    rust_bash()
        .arg(tmp.path())
        .assert()
        .success()
        .stdout(format!("{path_str}\n"));
}

// ── Additional edge cases ───────────────────────────────────────────

#[test]
fn cwd_nonexistent_auto_creates() {
    rust_bash()
        .args(["--cwd", "/nonexistent/deep/path", "-c", "pwd"])
        .assert()
        .success()
        .stdout("/nonexistent/deep/path\n");
}

#[test]
fn multiline_command_string() {
    rust_bash()
        .args(["-c", "echo a\necho b"])
        .assert()
        .success()
        .stdout("a\nb\n");
}

#[test]
fn empty_stdin_is_noop() {
    rust_bash().write_stdin("").assert().success().stdout("");
}

#[test]
fn newline_only_stdin_is_noop() {
    rust_bash().write_stdin("\n").assert().success().stdout("");
}

#[test]
fn env_override_home() {
    rust_bash()
        .args(["--env", "HOME=/custom", "-c", "echo $HOME"])
        .assert()
        .success()
        .stdout("/custom\n");
}

#[test]
fn files_colon_in_host_path() {
    // First colon is the separator: host="/path/with" vfs="colon"
    // Since "/path/with" won't exist, we expect exit 2 with error.
    rust_bash()
        .args(["--files", "/path/with:colon", "-c", "echo hi"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn double_dash_stops_flag_parsing() {
    let mut tmp = NamedTempFile::with_suffix(".sh").unwrap();
    writeln!(tmp, "echo from-script").unwrap();
    tmp.flush().unwrap();

    rust_bash()
        .arg("--")
        .arg(tmp.path())
        .assert()
        .success()
        .stdout("from-script\n");
}

#[test]
fn files_binary_content_passthrough() {
    // The shell's stdout is string-based, so full 0-255 binary data gets
    // UTF-8 mangled for bytes > 127. Test with ASCII-range bytes (0-127)
    // which round-trip correctly.
    let mut tmp = NamedTempFile::new().unwrap();
    let binary_data: Vec<u8> = (0..=127).collect();
    tmp.write_all(&binary_data).unwrap();
    tmp.flush().unwrap();

    let mapping = format!("{}:/bin.dat", tmp.path().display());
    let output = rust_bash()
        .args(["--files", &mapping, "-c", "cat /bin.dat"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, binary_data);
}

// ── Additional --json coverage ──────────────────────────────────────

#[test]
fn json_script_file() {
    let mut tmp = NamedTempFile::new().unwrap();
    writeln!(tmp, "echo from-file").unwrap();
    tmp.flush().unwrap();

    let output = rust_bash().arg("--json").arg(tmp.path()).output().unwrap();

    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(json["stdout"], "from-file\n");
}

#[test]
fn json_stdin_pipe() {
    let output = rust_bash()
        .arg("--json")
        .write_stdin("echo from-stdin\n")
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(json["stdout"], "from-stdin\n");
}

// ── Additional edge case coverage ──────────────────────────────────

#[test]
fn env_value_with_equals() {
    rust_bash()
        .args(["--env", "DSN=host=localhost;port=5432", "-c", "echo $DSN"])
        .assert()
        .success()
        .stdout("host=localhost;port=5432\n");
}

#[test]
fn files_nested_subdirectories() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
    std::fs::write(dir.path().join("sub/deep/file.txt"), "nested").unwrap();

    let mapping = format!("{}:/app", dir.path().display());

    rust_bash()
        .args(["--files", &mapping, "-c", "cat /app/sub/deep/file.txt"])
        .assert()
        .success()
        .stdout("nested");
}

// Note: `--json` without `-c` on a real TTY → exit 2 is not tested here
// because assert_cmd runs without a TTY, so stdin is never detected as
// a terminal. This path is covered by manual testing.
