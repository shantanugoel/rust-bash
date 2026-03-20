#![cfg(feature = "ffi")]

use std::ffi::{CStr, CString};

use rust_bash::ffi::{
    CExecResult, rust_bash_create, rust_bash_exec, rust_bash_free, rust_bash_last_error,
    rust_bash_result_free, rust_bash_version,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a sandbox from a JSON string (convenience wrapper).
fn create_from_json(json: &str) -> *mut rust_bash::RustBash {
    let c_json = CString::new(json).unwrap();
    unsafe { rust_bash_create(c_json.as_ptr()) }
}

/// Execute a command and return the raw result pointer (convenience wrapper).
fn exec_cmd(sb: *mut rust_bash::RustBash, cmd: &str) -> *mut CExecResult {
    let c_cmd = CString::new(cmd).unwrap();
    unsafe { rust_bash_exec(sb, c_cmd.as_ptr()) }
}

/// Read stdout from a CExecResult as a Rust String.
unsafe fn read_stdout(result: *const CExecResult) -> String {
    let r = unsafe { &*result };
    if r.stdout_len == 0 {
        return String::new();
    }
    let slice =
        unsafe { std::slice::from_raw_parts(r.stdout_ptr as *const u8, r.stdout_len as usize) };
    String::from_utf8_lossy(slice).into_owned()
}

/// Read stderr from a CExecResult as a Rust String.
unsafe fn read_stderr(result: *const CExecResult) -> String {
    let r = unsafe { &*result };
    if r.stderr_len == 0 {
        return String::new();
    }
    let slice =
        unsafe { std::slice::from_raw_parts(r.stderr_ptr as *const u8, r.stderr_len as usize) };
    String::from_utf8_lossy(slice).into_owned()
}

/// Read the current last_error as a Rust string, or None.
fn last_error() -> Option<String> {
    let ptr = rust_bash_last_error();
    if ptr.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

// ===========================================================================
// Happy path
// ===========================================================================

#[test]
fn create_default_exec_echo_and_free() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "echo hello");
    assert!(!result.is_null());
    unsafe {
        assert_eq!(read_stdout(result), "hello\n");
        assert_eq!((*result).exit_code, 0);
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn create_with_files_exec_cat() {
    let json = r#"{ "files": { "/data.txt": "file content here" } }"#;
    let sb = create_from_json(json);
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "cat /data.txt");
    assert!(!result.is_null());
    unsafe {
        assert_eq!(read_stdout(result), "file content here");
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn create_with_env_exec_echo_var() {
    let json = r#"{ "env": { "MY_VAR": "hello_world" } }"#;
    let sb = create_from_json(json);
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "echo $MY_VAR");
    assert!(!result.is_null());
    unsafe {
        assert_eq!(read_stdout(result).trim(), "hello_world");
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn create_with_cwd_exec_pwd() {
    let json = r#"{ "cwd": "/custom/dir" }"#;
    let sb = create_from_json(json);
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "pwd");
    assert!(!result.is_null());
    unsafe {
        assert_eq!(read_stdout(result).trim(), "/custom/dir");
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn state_persistence_across_exec_calls() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let r1 = exec_cmd(sb, "X=42");
    assert!(!r1.is_null());
    unsafe { rust_bash_result_free(r1) };

    let r2 = exec_cmd(sb, "echo $X");
    assert!(!r2.is_null());
    unsafe {
        assert_eq!(read_stdout(r2), "42\n");
    }
    unsafe { rust_bash_result_free(r2) };

    unsafe { rust_bash_free(sb) };
}

// ===========================================================================
// Error handling
// ===========================================================================

#[test]
fn exec_with_null_sandbox_returns_null_and_sets_error() {
    let result = unsafe {
        rust_bash_exec(
            std::ptr::null_mut(),
            CString::new("echo hi").unwrap().as_ptr(),
        )
    };
    assert!(result.is_null());
    let err = last_error().expect("last_error should be set");
    assert!(err.contains("Null sandbox pointer"), "got: {err}");
}

#[test]
fn exec_with_null_command_returns_null_and_sets_error() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = unsafe { rust_bash_exec(sb, std::ptr::null()) };
    assert!(result.is_null());
    let err = last_error().expect("last_error should be set");
    assert!(err.contains("Null command pointer"), "got: {err}");

    unsafe { rust_bash_free(sb) };
}

#[test]
fn create_with_invalid_json_returns_null() {
    let bad_json = CString::new("not json at all!!!").unwrap();
    let sb = unsafe { rust_bash_create(bad_json.as_ptr()) };
    assert!(sb.is_null());
    let err = last_error().expect("last_error should be set");
    assert!(
        err.contains("parse error") || err.contains("JSON"),
        "got: {err}"
    );
}

#[test]
fn last_error_is_null_after_successful_call() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());
    assert!(last_error().is_none(), "no error after successful create");

    let result = exec_cmd(sb, "echo ok");
    assert!(!result.is_null());
    assert!(last_error().is_none(), "no error after successful exec");

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

// ===========================================================================
// Edge cases
// ===========================================================================

#[test]
fn free_null_sandbox_is_noop() {
    unsafe { rust_bash_free(std::ptr::null_mut()) };
}

#[test]
fn result_free_null_is_noop() {
    unsafe { rust_bash_result_free(std::ptr::null_mut()) };
}

#[test]
fn create_with_empty_json_produces_valid_sandbox() {
    let sb = create_from_json("{}");
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "echo works");
    assert!(!result.is_null());
    unsafe {
        assert_eq!(read_stdout(result), "works\n");
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn exec_empty_command_succeeds() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "");
    assert!(!result.is_null());
    unsafe {
        assert_eq!((*result).exit_code, 0);
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn version_returns_non_null() {
    let ver = rust_bash_version();
    assert!(!ver.is_null());
    let s = unsafe { CStr::from_ptr(ver) }.to_str().unwrap();
    assert!(!s.is_empty());
    // Should look like a semver string
    assert!(s.contains('.'), "version should contain '.': {s}");
}

// ===========================================================================
// Limits
// ===========================================================================

#[test]
fn max_command_count_limit_triggers_error() {
    let json = r#"{ "limits": { "max_command_count": 1 } }"#;
    let sb = create_from_json(json);
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "echo a; echo b");
    assert!(result.is_null(), "should fail when limit exceeded");
    let err = last_error().expect("last_error should be set");
    assert!(
        err.to_lowercase().contains("limit"),
        "error should mention limit: {err}"
    );

    unsafe { rust_bash_free(sb) };
}

// ===========================================================================
// Empty / zero-length output
// ===========================================================================

#[test]
fn command_with_no_output() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "true");
    assert!(!result.is_null());
    unsafe {
        assert_eq!((*result).stdout_len, 0);
        assert_eq!((*result).exit_code, 0);
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn command_with_only_stderr() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "echo error >&2");
    assert!(!result.is_null());
    unsafe {
        assert_eq!((*result).stdout_len, 0);
        assert!((*result).stderr_len > 0);
        assert!(read_stderr(result).contains("error"));
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn unicode_output() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "echo '日本語'");
    assert!(!result.is_null());
    unsafe {
        let stdout = read_stdout(result);
        assert!(stdout.contains("日本語"), "got: {stdout}");
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

// ===========================================================================
// Expanded edge cases (Phase 4)
// ===========================================================================

#[test]
fn large_output_not_truncated() {
    let json = r#"{
        "limits": {
            "max_command_count": 100000,
            "max_loop_iterations": 20000,
            "max_output_size": 10485760
        }
    }"#;
    let sb = create_from_json(json);
    assert!(!sb.is_null());

    // Generate >1MB of output: 15000 lines × ~79 bytes each ≈ 1.17MB
    let result = exec_cmd(
        sb,
        r#"i=0; while [ $i -lt 15000 ]; do echo "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; i=$((i+1)); done"#,
    );
    assert!(!result.is_null(), "exec failed: {:?}", last_error());
    unsafe {
        let stdout = read_stdout(result);
        let len = (*result).stdout_len as usize;
        assert!(len > 1_000_000, "expected >1MB output, got {len} bytes");
        assert_eq!(len, stdout.len());
        assert!(stdout.ends_with('\n'));
        let line_count = stdout.lines().count();
        assert_eq!(line_count, 15000, "expected 15000 lines, got {line_count}");
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn non_zero_exit_code() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "exit 42");
    assert!(!result.is_null(), "exec failed: {:?}", last_error());
    unsafe {
        assert_eq!((*result).exit_code, 42);
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}

#[test]
fn sequential_create_free_cycles() {
    for i in 0..20 {
        let sb = unsafe { rust_bash_create(std::ptr::null()) };
        assert!(!sb.is_null(), "create failed on cycle {i}");

        let result = exec_cmd(sb, "echo cycle");
        assert!(!result.is_null(), "exec failed on cycle {i}");
        unsafe {
            assert_eq!(read_stdout(result), "cycle\n");
            rust_bash_result_free(result);
        }

        unsafe { rust_bash_free(sb) };
    }
}

#[test]
fn config_with_all_limits_fields_enforced() {
    let json = r#"{
        "limits": {
            "max_command_count": 500,
            "max_execution_time_secs": 10,
            "max_loop_iterations": 200,
            "max_output_size": 4096,
            "max_call_depth": 50,
            "max_string_length": 2048,
            "max_glob_results": 1000,
            "max_substitution_depth": 20,
            "max_heredoc_size": 8192,
            "max_brace_expansion": 100
        }
    }"#;
    let sb = create_from_json(json);
    assert!(!sb.is_null());

    // Basic command within limits succeeds
    let result = exec_cmd(sb, "echo hello");
    assert!(!result.is_null());
    unsafe {
        assert_eq!(read_stdout(result), "hello\n");
        assert_eq!((*result).exit_code, 0);
        rust_bash_result_free(result);
    }

    // Exceed max_output_size (4096 bytes): 100 lines × ~100 bytes = ~10KB
    let r2 = exec_cmd(
        sb,
        r#"i=0; while [ $i -lt 100 ]; do echo "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; i=$((i+1)); done"#,
    );
    // Should fail due to output size limit
    assert!(r2.is_null(), "expected output size limit to trigger");
    let err = last_error().expect("last_error should be set");
    assert!(
        err.to_lowercase().contains("limit") || err.to_lowercase().contains("output"),
        "error should mention limit/output: {err}"
    );

    // Verify sandbox is still usable after a limit failure
    let r3 = exec_cmd(sb, "echo alive");
    assert!(!r3.is_null(), "sandbox should survive a limit failure");
    unsafe {
        assert_eq!(read_stdout(r3).trim(), "alive");
        rust_bash_result_free(r3);
    }

    unsafe { rust_bash_free(sb) };
}

#[test]
fn stderr_and_stdout_captured_together() {
    let sb = unsafe { rust_bash_create(std::ptr::null()) };
    assert!(!sb.is_null());

    let result = exec_cmd(sb, "echo out_msg; echo err_msg >&2");
    assert!(!result.is_null());
    unsafe {
        let stdout = read_stdout(result);
        let stderr = read_stderr(result);
        assert!(stdout.contains("out_msg"), "stdout: {stdout}");
        assert!(stderr.contains("err_msg"), "stderr: {stderr}");
        assert_eq!((*result).stdout_len as usize, stdout.len());
        assert_eq!((*result).stderr_len as usize, stderr.len());
    }

    unsafe { rust_bash_result_free(result) };
    unsafe { rust_bash_free(sb) };
}
