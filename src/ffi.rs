//! FFI surface for embedding rust-bash from C, Python, Go, or any language with C interop.
//!
//! This module provides JSON-based configuration deserialization that maps onto the
//! [`RustBashBuilder`] API. The JSON config schema allows callers to specify files,
//! environment variables, working directory, execution limits, and network policy.
//!
//! # Memory Ownership Rules
//!
//! - [`rust_bash_create`] returns a heap-allocated `RustBash*` that the caller owns.
//!   Free it with [`rust_bash_free`].
//! - [`rust_bash_exec`] returns a heap-allocated `CExecResult*` that the caller owns.
//!   Free it with [`rust_bash_result_free`].
//! - [`rust_bash_version`] returns a pointer to a static string. The caller must **not** free it.
//! - [`rust_bash_last_error`] returns a pointer into thread-local storage. The pointer is
//!   valid only until the next FFI call on the same thread. The caller must **not** free it.
//!
//! # Thread Safety
//!
//! A `RustBash*` handle must not be shared across threads without external synchronization.
//! Each handle is independently owned; different handles may be used concurrently from
//! different threads. The last-error storage is thread-local, so error messages are
//! per-thread.
//!
//! # JSON Config Schema
//!
//! ```json
//! {
//!   "files": {
//!     "/data.txt": "file content",
//!     "/config.json": "{}"
//!   },
//!   "env": {
//!     "USER": "agent",
//!     "HOME": "/home/agent"
//!   },
//!   "cwd": "/",
//!   "limits": {
//!     "max_command_count": 10000,
//!     "max_execution_time_secs": 30,
//!     "max_loop_iterations": 10000,
//!     "max_output_size": 10485760,
//!     "max_call_depth": 100,
//!     "max_string_length": 10485760,
//!     "max_glob_results": 100000,
//!     "max_substitution_depth": 50,
//!     "max_heredoc_size": 10485760,
//!     "max_brace_expansion": 10000
//!   },
//!   "network": {
//!     "enabled": true,
//!     "allowed_url_prefixes": ["https://api.example.com/"],
//!     "allowed_methods": ["GET", "POST"],
//!     "max_response_size": 10485760,
//!     "max_redirects": 5,
//!     "timeout_secs": 30
//!   }
//! }
//! ```
//!
//! All fields are optional. An empty `{}` produces a default-configured sandbox.

use serde::Deserialize;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char};
use std::time::Duration;

use crate::api::{RustBash, RustBashBuilder};
use crate::interpreter::ExecutionLimits;
use crate::network::NetworkPolicy;

// ---------------------------------------------------------------------------
// Thread-local error storage
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_error(msg: String) {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = CString::new(msg.replace('\0', "\\0")).ok();
    });
}

fn clear_last_error() {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

// ---------------------------------------------------------------------------
// CExecResult — C-compatible execution result
// ---------------------------------------------------------------------------

/// Result of executing a command via [`rust_bash_exec`].
///
/// The `stdout_ptr`/`stderr_ptr` fields point to heap-allocated byte buffers whose
/// lengths are given by `stdout_len`/`stderr_len`. The caller must free the entire
/// result (including these buffers) by passing it to [`rust_bash_result_free`].
#[repr(C)]
pub struct CExecResult {
    pub stdout_ptr: *const c_char,
    pub stdout_len: i32,
    pub stderr_ptr: *const c_char,
    pub stderr_len: i32,
    pub exit_code: i32,
}

// ---------------------------------------------------------------------------
// FFI entry points
// ---------------------------------------------------------------------------

/// Create a new sandboxed shell instance.
///
/// If `config_json` is `NULL`, a default configuration is used. Otherwise it must
/// point to a valid null-terminated UTF-8 JSON string conforming to the schema
/// documented in the [module-level docs](self).
///
/// Returns a heap-allocated `RustBash*` on success, or `NULL` on error. On error
/// the reason is retrievable via [`rust_bash_last_error`].
///
/// # Safety
///
/// - `config_json`, if non-null, must point to a valid null-terminated C string.
/// - The returned pointer must eventually be passed to [`rust_bash_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_bash_create(config_json: *const c_char) -> *mut RustBash {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        clear_last_error();

        let config: FfiConfig = if config_json.is_null() {
            FfiConfig::default()
        } else {
            let c_str = unsafe { CStr::from_ptr(config_json) };
            let json_str = match c_str.to_str() {
                Ok(s) => s,
                Err(e) => {
                    set_last_error(format!("Invalid UTF-8 in config_json: {e}"));
                    return std::ptr::null_mut();
                }
            };
            match serde_json::from_str(json_str) {
                Ok(c) => c,
                Err(e) => {
                    set_last_error(format!("JSON parse error: {e}"));
                    return std::ptr::null_mut();
                }
            }
        };

        match config.into_rust_bash() {
            Ok(shell) => Box::into_raw(Box::new(shell)),
            Err(e) => {
                set_last_error(format!("Failed to create sandbox: {e}"));
                std::ptr::null_mut()
            }
        }
    }));

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            set_last_error("rust_bash_create panicked".to_string());
            std::ptr::null_mut()
        }
    }
}

/// Execute a shell command string in an existing sandbox.
///
/// Returns a heap-allocated [`ExecResult`](CExecResult) on success, or `NULL` on error.
/// On error the reason is retrievable via [`rust_bash_last_error`].
///
/// # Safety
///
/// - `sb` must be a non-null pointer previously returned by [`rust_bash_create`]
///   and not yet freed.
/// - `command` must be a non-null pointer to a valid null-terminated C string.
/// - The returned `ExecResult*` must eventually be passed to [`rust_bash_result_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_bash_exec(
    sb: *mut RustBash,
    command: *const c_char,
) -> *mut CExecResult {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        clear_last_error();

        if sb.is_null() {
            set_last_error("Null sandbox pointer".to_string());
            return std::ptr::null_mut();
        }
        if command.is_null() {
            set_last_error("Null command pointer".to_string());
            return std::ptr::null_mut();
        }

        let cmd_str = match unsafe { CStr::from_ptr(command) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in command: {e}"));
                return std::ptr::null_mut();
            }
        };

        let shell = unsafe { &mut *sb };
        match shell.exec(cmd_str) {
            Ok(exec_result) => {
                let stdout_bytes: Vec<u8> = exec_result.stdout.into_bytes();
                let stdout_len: i32 = match stdout_bytes.len().try_into() {
                    Ok(n) => n,
                    Err(_) => {
                        set_last_error("stdout exceeds i32::MAX bytes".to_string());
                        return std::ptr::null_mut();
                    }
                };
                let stdout_boxed: Box<[u8]> = stdout_bytes.into_boxed_slice();
                let stdout_fat: *mut [u8] = Box::into_raw(stdout_boxed);
                let stdout_ptr: *const c_char = stdout_fat as *mut u8 as *const c_char;

                let stderr_bytes: Vec<u8> = exec_result.stderr.into_bytes();
                let stderr_len: i32 = match stderr_bytes.len().try_into() {
                    Ok(n) => n,
                    Err(_) => {
                        // Reclaim already-leaked stdout before returning
                        let fat = std::ptr::slice_from_raw_parts_mut(
                            stdout_ptr as *mut u8,
                            stdout_len as usize,
                        );
                        drop(unsafe { Box::from_raw(fat) });
                        set_last_error("stderr exceeds i32::MAX bytes".to_string());
                        return std::ptr::null_mut();
                    }
                };
                let stderr_boxed: Box<[u8]> = stderr_bytes.into_boxed_slice();
                let stderr_fat: *mut [u8] = Box::into_raw(stderr_boxed);
                let stderr_ptr: *const c_char = stderr_fat as *mut u8 as *const c_char;

                let c_result = CExecResult {
                    stdout_ptr,
                    stdout_len,
                    stderr_ptr,
                    stderr_len,
                    exit_code: exec_result.exit_code,
                };
                Box::into_raw(Box::new(c_result))
            }
            Err(e) => {
                set_last_error(e.to_string());
                std::ptr::null_mut()
            }
        }
    }));

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            set_last_error("rust_bash_exec panicked".to_string());
            std::ptr::null_mut()
        }
    }
}

/// Free an [`ExecResult`](CExecResult) previously returned by [`rust_bash_exec`].
///
/// If `result` is `NULL` this is a no-op.
///
/// # Safety
///
/// - `result` must be `NULL` or a pointer previously returned by [`rust_bash_exec`]
///   that has not yet been freed.
/// - After this call the pointer is invalid and must not be dereferenced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_bash_result_free(result: *mut CExecResult) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if result.is_null() {
            return;
        }
        let res = unsafe { Box::from_raw(result) };

        if !res.stdout_ptr.is_null() && res.stdout_len >= 0 {
            let fat = std::ptr::slice_from_raw_parts_mut(
                res.stdout_ptr as *mut u8,
                res.stdout_len as usize,
            );
            drop(unsafe { Box::from_raw(fat) });
        }

        if !res.stderr_ptr.is_null() && res.stderr_len >= 0 {
            let fat = std::ptr::slice_from_raw_parts_mut(
                res.stderr_ptr as *mut u8,
                res.stderr_len as usize,
            );
            drop(unsafe { Box::from_raw(fat) });
        }
    }));
}

/// Free a `RustBash*` handle previously returned by [`rust_bash_create`].
///
/// If `sb` is `NULL` this is a no-op.
///
/// # Safety
///
/// - `sb` must be `NULL` or a pointer previously returned by [`rust_bash_create`]
///   that has not yet been freed.
/// - After this call the pointer is invalid and must not be dereferenced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_bash_free(sb: *mut RustBash) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !sb.is_null() {
            drop(unsafe { Box::from_raw(sb) });
        }
    }));
}

/// Return the library version as a static null-terminated string.
///
/// The returned pointer is valid for the lifetime of the loaded library and must
/// **not** be freed by the caller.
///
/// # Safety
///
/// The returned pointer points to static read-only memory.
#[unsafe(no_mangle)]
pub extern "C" fn rust_bash_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Retrieve the last error message for the current thread.
///
/// Returns `NULL` if no error is stored (i.e. the last FFI call succeeded).
/// The returned pointer is valid only until the next FFI call on the same thread.
/// The caller must **not** free the returned pointer.
///
/// # Safety
///
/// The returned pointer (if non-null) points to thread-local storage and is
/// invalidated by the next FFI call on the same thread.
#[unsafe(no_mangle)]
pub extern "C" fn rust_bash_last_error() -> *const c_char {
    let result = std::panic::catch_unwind(|| {
        LAST_ERROR.with(|cell| {
            cell.borrow()
                .as_ref()
                .map_or(std::ptr::null(), |cs| cs.as_ptr())
        })
    });
    result.unwrap_or(std::ptr::null())
}

/// JSON-deserializable configuration for creating a [`RustBash`] sandbox.
#[derive(Deserialize, Default)]
pub struct FfiConfig {
    #[serde(default)]
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    pub limits: Option<FfiLimits>,
    pub network: Option<FfiNetwork>,
}

/// Execution-limit overrides. Unset fields inherit from [`ExecutionLimits::default()`].
#[derive(Deserialize, Default)]
pub struct FfiLimits {
    pub max_command_count: Option<usize>,
    pub max_execution_time_secs: Option<u64>,
    pub max_loop_iterations: Option<usize>,
    pub max_output_size: Option<usize>,
    pub max_call_depth: Option<usize>,
    pub max_string_length: Option<usize>,
    pub max_glob_results: Option<usize>,
    pub max_substitution_depth: Option<usize>,
    pub max_heredoc_size: Option<usize>,
    pub max_brace_expansion: Option<usize>,
}

/// Network-policy overrides. Unset fields inherit from [`NetworkPolicy::default()`].
#[derive(Deserialize, Default)]
pub struct FfiNetwork {
    pub enabled: Option<bool>,
    pub allowed_url_prefixes: Option<Vec<String>>,
    pub allowed_methods: Option<Vec<String>>,
    pub max_response_size: Option<usize>,
    pub max_redirects: Option<usize>,
    pub timeout_secs: Option<u64>,
}

impl FfiLimits {
    /// Convert into [`ExecutionLimits`], filling unset fields with defaults.
    pub fn into_execution_limits(self) -> ExecutionLimits {
        let defaults = ExecutionLimits::default();
        ExecutionLimits {
            max_command_count: self.max_command_count.unwrap_or(defaults.max_command_count),
            max_execution_time: self
                .max_execution_time_secs
                .map_or(defaults.max_execution_time, Duration::from_secs),
            max_loop_iterations: self
                .max_loop_iterations
                .unwrap_or(defaults.max_loop_iterations),
            max_output_size: self.max_output_size.unwrap_or(defaults.max_output_size),
            max_call_depth: self.max_call_depth.unwrap_or(defaults.max_call_depth),
            max_string_length: self.max_string_length.unwrap_or(defaults.max_string_length),
            max_glob_results: self.max_glob_results.unwrap_or(defaults.max_glob_results),
            max_substitution_depth: self
                .max_substitution_depth
                .unwrap_or(defaults.max_substitution_depth),
            max_heredoc_size: self.max_heredoc_size.unwrap_or(defaults.max_heredoc_size),
            max_brace_expansion: self
                .max_brace_expansion
                .unwrap_or(defaults.max_brace_expansion),
        }
    }
}

impl FfiNetwork {
    /// Convert into [`NetworkPolicy`], filling unset fields with defaults.
    pub fn into_network_policy(self) -> NetworkPolicy {
        let defaults = NetworkPolicy::default();
        NetworkPolicy {
            enabled: self.enabled.unwrap_or(defaults.enabled),
            allowed_url_prefixes: self
                .allowed_url_prefixes
                .unwrap_or(defaults.allowed_url_prefixes),
            allowed_methods: self
                .allowed_methods
                .map(|v| v.into_iter().collect::<HashSet<String>>())
                .unwrap_or(defaults.allowed_methods),
            max_response_size: self.max_response_size.unwrap_or(defaults.max_response_size),
            max_redirects: self.max_redirects.unwrap_or(defaults.max_redirects),
            timeout: self
                .timeout_secs
                .map_or(defaults.timeout, Duration::from_secs),
        }
    }
}

impl FfiConfig {
    /// Build a [`RustBash`] sandbox from this configuration.
    pub fn into_rust_bash(self) -> Result<RustBash, crate::error::RustBashError> {
        let files: HashMap<String, Vec<u8>> = self
            .files
            .into_iter()
            .map(|(path, content)| (path, content.into_bytes()))
            .collect();

        let mut builder = RustBashBuilder::new().files(files).env(self.env);

        if let Some(cwd) = self.cwd {
            builder = builder.cwd(cwd);
        }

        if let Some(limits) = self.limits {
            builder = builder.execution_limits(limits.into_execution_limits());
        }

        if let Some(network) = self.network {
            builder = builder.network_policy(network.into_network_policy());
        }

        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_json_produces_default_config() {
        let config: FfiConfig = serde_json::from_str("{}").unwrap();
        assert!(config.files.is_empty());
        assert!(config.env.is_empty());
        assert!(config.cwd.is_none());
        assert!(config.limits.is_none());
        assert!(config.network.is_none());
    }

    #[test]
    fn full_config_deserializes_all_fields() {
        let json = r#"{
            "files": { "/data.txt": "hello" },
            "env": { "USER": "agent" },
            "cwd": "/home",
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
            },
            "network": {
                "enabled": true,
                "allowed_url_prefixes": ["https://api.example.com/"],
                "allowed_methods": ["GET"],
                "max_response_size": 1024,
                "max_redirects": 3,
                "timeout_secs": 15
            }
        }"#;

        let config: FfiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.files.get("/data.txt").unwrap(), "hello");
        assert_eq!(config.env.get("USER").unwrap(), "agent");
        assert_eq!(config.cwd.as_deref(), Some("/home"));

        let limits = config.limits.unwrap().into_execution_limits();
        assert_eq!(limits.max_command_count, 500);
        assert_eq!(limits.max_execution_time, Duration::from_secs(10));
        assert_eq!(limits.max_loop_iterations, 200);
        assert_eq!(limits.max_output_size, 4096);
        assert_eq!(limits.max_call_depth, 50);
        assert_eq!(limits.max_string_length, 2048);
        assert_eq!(limits.max_glob_results, 1000);
        assert_eq!(limits.max_substitution_depth, 20);
        assert_eq!(limits.max_heredoc_size, 8192);
        assert_eq!(limits.max_brace_expansion, 100);

        let network = config.network.unwrap().into_network_policy();
        assert!(network.enabled);
        assert_eq!(
            network.allowed_url_prefixes,
            vec!["https://api.example.com/"]
        );
        assert_eq!(network.allowed_methods, HashSet::from(["GET".to_string()]));
        assert_eq!(network.max_response_size, 1024);
        assert_eq!(network.max_redirects, 3);
        assert_eq!(network.timeout, Duration::from_secs(15));
    }

    #[test]
    fn partial_config_defaults_missing_fields() {
        let json = r#"{ "files": { "/a.txt": "content" } }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.files.len(), 1);
        assert!(config.env.is_empty());
        assert!(config.cwd.is_none());
        assert!(config.limits.is_none());
        assert!(config.network.is_none());
    }

    #[test]
    fn limits_with_partial_fields_defaults_the_rest() {
        let json = r#"{ "limits": { "max_command_count": 42 } }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        let limits = config.limits.unwrap().into_execution_limits();
        assert_eq!(limits.max_command_count, 42);

        let defaults = ExecutionLimits::default();
        assert_eq!(limits.max_execution_time, defaults.max_execution_time);
        assert_eq!(limits.max_loop_iterations, defaults.max_loop_iterations);
        assert_eq!(limits.max_output_size, defaults.max_output_size);
        assert_eq!(limits.max_call_depth, defaults.max_call_depth);
        assert_eq!(limits.max_string_length, defaults.max_string_length);
        assert_eq!(limits.max_glob_results, defaults.max_glob_results);
        assert_eq!(
            limits.max_substitution_depth,
            defaults.max_substitution_depth
        );
        assert_eq!(limits.max_heredoc_size, defaults.max_heredoc_size);
        assert_eq!(limits.max_brace_expansion, defaults.max_brace_expansion);
    }

    #[test]
    fn network_config_maps_to_network_policy() {
        let json = r#"{
            "network": {
                "enabled": true,
                "allowed_url_prefixes": ["https://a.com/", "https://b.com/"],
                "allowed_methods": ["GET", "POST", "PUT"],
                "max_response_size": 2048,
                "max_redirects": 10,
                "timeout_secs": 60
            }
        }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        let policy = config.network.unwrap().into_network_policy();

        assert!(policy.enabled);
        assert_eq!(policy.allowed_url_prefixes.len(), 2);
        assert!(
            policy
                .allowed_url_prefixes
                .contains(&"https://a.com/".to_string())
        );
        assert!(
            policy
                .allowed_url_prefixes
                .contains(&"https://b.com/".to_string())
        );
        assert_eq!(policy.allowed_methods.len(), 3);
        assert!(policy.allowed_methods.contains("GET"));
        assert!(policy.allowed_methods.contains("POST"));
        assert!(policy.allowed_methods.contains("PUT"));
        assert_eq!(policy.max_response_size, 2048);
        assert_eq!(policy.max_redirects, 10);
        assert_eq!(policy.timeout, Duration::from_secs(60));
    }

    #[test]
    fn default_network_policy_when_no_fields_set() {
        let json = r#"{ "network": {} }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        let policy = config.network.unwrap().into_network_policy();
        let defaults = NetworkPolicy::default();

        assert_eq!(policy.enabled, defaults.enabled);
        assert_eq!(policy.allowed_url_prefixes, defaults.allowed_url_prefixes);
        assert_eq!(policy.allowed_methods, defaults.allowed_methods);
        assert_eq!(policy.max_response_size, defaults.max_response_size);
        assert_eq!(policy.max_redirects, defaults.max_redirects);
        assert_eq!(policy.timeout, defaults.timeout);
    }

    #[test]
    fn unknown_extra_fields_are_ignored() {
        let json = r#"{ "files": {}, "extra_field": 42, "another": "value" }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        assert!(config.files.is_empty());
    }

    #[test]
    fn into_rust_bash_builds_with_empty_config() {
        let config: FfiConfig = serde_json::from_str("{}").unwrap();
        let shell = config.into_rust_bash();
        assert!(shell.is_ok());
    }

    #[test]
    fn into_rust_bash_builds_with_full_config() {
        let json = r#"{
            "files": { "/hello.txt": "world" },
            "env": { "FOO": "bar" },
            "cwd": "/tmp",
            "limits": { "max_command_count": 100 },
            "network": { "enabled": false }
        }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        let mut shell = config.into_rust_bash().unwrap();
        let result = shell.exec("cat /hello.txt").unwrap();
        assert_eq!(result.stdout, "world");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn into_rust_bash_sets_cwd() {
        let json = r#"{ "cwd": "/mydir" }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        let mut shell = config.into_rust_bash().unwrap();
        let result = shell.exec("pwd").unwrap();
        assert_eq!(result.stdout.trim(), "/mydir");
    }

    #[test]
    fn into_rust_bash_sets_env() {
        let json = r#"{ "env": { "GREETING": "hello" } }"#;
        let config: FfiConfig = serde_json::from_str(json).unwrap();
        let mut shell = config.into_rust_bash().unwrap();
        let result = shell.exec("echo $GREETING").unwrap();
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[test]
    fn invalid_type_returns_deserialization_error() {
        let json = r#"{ "limits": { "max_command_count": "not_a_number" } }"#;
        let result = serde_json::from_str::<FfiConfig>(json);
        assert!(result.is_err());
    }
}
