//! napi-rs bindings for rust-bash.
//!
//! Provides a `NativeBash` class for use from Node.js via the native addon.

use std::collections::HashMap;
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use rust_bash::api::{RustBash, RustBashBuilder};
use rust_bash::interpreter::ExecutionLimits;

// ── Config types for JSON deserialization ─────────────────────────────

#[derive(serde::Deserialize, Default)]
struct NativeConfig {
    #[serde(default)]
    files: HashMap<String, String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    limits: Option<LimitsConfig>,
    #[serde(default)]
    network: Option<NetworkConfig>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitsConfig {
    max_command_count: Option<usize>,
    max_execution_time_secs: Option<f64>,
    max_loop_iterations: Option<usize>,
    max_output_size: Option<usize>,
    max_call_depth: Option<usize>,
    max_string_length: Option<usize>,
    max_glob_results: Option<usize>,
    max_substitution_depth: Option<usize>,
    max_heredoc_size: Option<usize>,
    max_brace_expansion: Option<usize>,
}

#[derive(serde::Deserialize, Default)]
struct ExecOptionsConfig {
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    stdin: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    allowed_url_prefixes: Vec<String>,
    #[serde(default)]
    allowed_methods: Vec<String>,
    #[serde(default)]
    max_response_size: Option<usize>,
    #[serde(default)]
    max_redirects: Option<usize>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecResultJson {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FileStatJson {
    is_file: bool,
    is_directory: bool,
    size: u64,
}

// ── NativeBash class ─────────────────────────────────────────────────

#[napi]
pub struct NativeBash {
    inner: RustBash,
}

#[napi]
impl NativeBash {
    #[napi(constructor)]
    pub fn new(config_json: String) -> Result<Self> {
        let config: NativeConfig = if config_json.is_empty() || config_json == "{}" {
            NativeConfig::default()
        } else {
            serde_json::from_str(&config_json)
                .map_err(|e| Error::from_reason(format!("Invalid config JSON: {e}")))?
        };

        let mut builder = RustBashBuilder::new();

        if !config.files.is_empty() {
            let file_map: HashMap<String, Vec<u8>> = config
                .files
                .into_iter()
                .map(|(k, v)| (k, v.into_bytes()))
                .collect();
            builder = builder.files(file_map);
        }

        if !config.env.is_empty() {
            builder = builder.env(config.env);
        }

        if let Some(cwd) = config.cwd {
            builder = builder.cwd(cwd);
        }

        if let Some(limits_cfg) = config.limits {
            let mut limits = ExecutionLimits::default();
            if let Some(v) = limits_cfg.max_command_count {
                limits.max_command_count = v;
            }
            if let Some(v) = limits_cfg.max_execution_time_secs {
                limits.max_execution_time = std::time::Duration::from_secs_f64(v);
            }
            if let Some(v) = limits_cfg.max_loop_iterations {
                limits.max_loop_iterations = v;
            }
            if let Some(v) = limits_cfg.max_output_size {
                limits.max_output_size = v;
            }
            if let Some(v) = limits_cfg.max_call_depth {
                limits.max_call_depth = v;
            }
            if let Some(v) = limits_cfg.max_string_length {
                limits.max_string_length = v;
            }
            if let Some(v) = limits_cfg.max_glob_results {
                limits.max_glob_results = v;
            }
            if let Some(v) = limits_cfg.max_substitution_depth {
                limits.max_substitution_depth = v;
            }
            if let Some(v) = limits_cfg.max_heredoc_size {
                limits.max_heredoc_size = v;
            }
            if let Some(v) = limits_cfg.max_brace_expansion {
                limits.max_brace_expansion = v;
            }
            builder = builder.execution_limits(limits);
        }

        if let Some(net_cfg) = config.network {
            let defaults = rust_bash::NetworkPolicy::default();
            let policy = rust_bash::NetworkPolicy {
                enabled: net_cfg.enabled,
                allowed_url_prefixes: if net_cfg.allowed_url_prefixes.is_empty() {
                    defaults.allowed_url_prefixes
                } else {
                    net_cfg.allowed_url_prefixes
                },
                allowed_methods: if net_cfg.allowed_methods.is_empty() {
                    defaults.allowed_methods
                } else {
                    net_cfg.allowed_methods.into_iter().collect()
                },
                max_response_size: net_cfg
                    .max_response_size
                    .unwrap_or(defaults.max_response_size),
                max_redirects: net_cfg.max_redirects.unwrap_or(defaults.max_redirects),
                timeout: net_cfg
                    .timeout_secs
                    .map(std::time::Duration::from_secs)
                    .unwrap_or(defaults.timeout),
            };
            builder = builder.network_policy(policy);
        }

        let inner = builder
            .build()
            .map_err(|e| Error::from_reason(e.to_string()))?;

        Ok(NativeBash { inner })
    }

    /// Execute a shell command and return JSON result.
    #[napi]
    pub fn exec(&mut self, command: String) -> Result<String> {
        let result = self
            .inner
            .exec(&command)
            .map_err(|e| Error::from_reason(e.to_string()))?;

        let json = ExecResultJson {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
        };

        serde_json::to_string(&json).map_err(|e| Error::from_reason(e.to_string()))
    }

    /// Execute a shell command with per-exec options, return JSON result.
    #[napi]
    pub fn exec_with_options(&mut self, command: String, options_json: String) -> Result<String> {
        let options: ExecOptionsConfig = serde_json::from_str(&options_json)
            .map_err(|e| Error::from_reason(format!("Invalid options JSON: {e}")))?;

        let result = self
            .inner
            .exec_with_overrides(
                &command,
                options.env.as_ref(),
                options.cwd.as_deref(),
                options.stdin.as_deref(),
            )
            .map_err(|e| Error::from_reason(e.to_string()))?;

        let json = ExecResultJson {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
        };

        serde_json::to_string(&json).map_err(|e| Error::from_reason(e.to_string()))
    }

    /// Write a file to the virtual filesystem.
    #[napi]
    pub fn write_file(&self, path: String, content: String) -> Result<()> {
        self.inner
            .write_file(&path, content.as_bytes())
            .map_err(|e| Error::from_reason(e.to_string()))
    }

    /// Read a file from the virtual filesystem.
    #[napi]
    pub fn read_file(&self, path: String) -> Result<String> {
        let data = self
            .inner
            .read_file(&path)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        String::from_utf8(data).map_err(|e| Error::from_reason(e.to_string()))
    }

    /// Create a directory in the virtual filesystem.
    #[napi]
    pub fn mkdir(&self, path: String, recursive: bool) -> Result<()> {
        self.inner
            .mkdir(&path, recursive)
            .map_err(|e| Error::from_reason(e.to_string()))
    }

    /// Check if a path exists in the virtual filesystem.
    #[napi]
    pub fn exists(&self, path: String) -> bool {
        self.inner.exists(&path)
    }

    /// List directory contents.
    #[napi]
    pub fn readdir(&self, path: String) -> Result<Vec<String>> {
        let entries = self
            .inner
            .readdir(&path)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(entries.into_iter().map(|e| e.name).collect())
    }

    /// Get file stat as JSON.
    #[napi]
    pub fn stat(&self, path: String) -> Result<String> {
        let metadata = self
            .inner
            .stat(&path)
            .map_err(|e| Error::from_reason(e.to_string()))?;

        let stat = FileStatJson {
            is_file: metadata.node_type == rust_bash::NodeType::File,
            is_directory: metadata.node_type == rust_bash::NodeType::Directory,
            size: metadata.size,
        };

        serde_json::to_string(&stat).map_err(|e| Error::from_reason(e.to_string()))
    }

    /// Remove a file or directory.
    #[napi]
    pub fn rm(&self, path: String, recursive: bool) -> Result<()> {
        if recursive {
            self.inner
                .remove_dir_all(&path)
                .map_err(|e| Error::from_reason(e.to_string()))
        } else {
            self.inner
                .remove_file(&path)
                .map_err(|e| Error::from_reason(e.to_string()))
        }
    }

    /// Get the current working directory.
    #[napi]
    pub fn get_cwd(&self) -> String {
        self.inner.cwd().to_string()
    }

    /// Get the exit code of the last executed command.
    #[napi]
    pub fn get_last_exit_code(&self) -> i32 {
        self.inner.last_exit_code()
    }

    /// Get the names of all registered commands.
    #[napi]
    pub fn get_command_names(&self) -> Vec<String> {
        self.inner
            .command_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Register a custom command backed by a JavaScript callback.
    ///
    /// The callback receives a JSON string `{ args: string[], ctx: { cwd, env, stdin } }`
    /// and must return a JSON string `{ stdout, stderr, exitCode }`.
    #[napi]
    pub fn register_command(
        &mut self,
        name: String,
        callback: napi::threadsafe_function::ThreadsafeFunction<String>,
    ) -> Result<()> {
        let cmd = NativeBridgeCommand {
            name: name.clone(),
            callback,
        };
        self.inner.register_command(Arc::new(cmd));
        Ok(())
    }
}

// ── NativeBridgeCommand ──────────────────────────────────────────────

/// A command that delegates execution to a Node.js callback via ThreadsafeFunction.
///
/// NOTE: The current implementation fires the callback but cannot capture its return value.
/// `ThreadsafeFunction::call()` returns a status code, not the JS callback result.
/// A future iteration should use `call_with_return_value` or a channel-based pattern
/// to retrieve the actual `{ stdout, stderr, exitCode }` from the JS callback.
struct NativeBridgeCommand {
    name: String,
    callback: napi::threadsafe_function::ThreadsafeFunction<String>,
}

// ThreadsafeFunction is Send + Sync by design in napi-rs

impl rust_bash::VirtualCommand for NativeBridgeCommand {
    fn name(&self) -> &str {
        &self.name
    }

    fn execute(
        &self,
        args: &[String],
        ctx: &rust_bash::CommandContext,
    ) -> rust_bash::CommandResult {
        let env: HashMap<String, String> = ctx
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let call_data = serde_json::json!({
            "args": args,
            "ctx": {
                "cwd": ctx.cwd,
                "env": env,
                "stdin": ctx.stdin,
            }
        });

        let json_str = call_data.to_string();

        match self.callback.call(
            Ok(json_str),
            napi::threadsafe_function::ThreadsafeFunctionCallMode::Blocking,
        ) {
            napi::Status::Ok => rust_bash::CommandResult::default(),
            status => rust_bash::CommandResult {
                stderr: format!(
                    "{}: ThreadsafeFunction call failed: {:?}\n",
                    self.name, status
                ),
                exit_code: 1,
                ..Default::default()
            },
        }
    }
}
