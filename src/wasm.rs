//! WASM bindings for rust-bash via `wasm-bindgen`.
//!
//! Provides the `WasmBash` class that wraps `RustBash` for use from JavaScript.
//! Feature-gated behind the `wasm` cargo feature.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::prelude::*;

use crate::api::{RustBash, RustBashBuilder};
use crate::commands::{CommandContext, CommandResult, VirtualCommand};
use crate::error::RustBashError;
use crate::interpreter::ExecutionLimits;
use crate::vfs::{NodeType, VirtualFs};

// ── WasmBash ─────────────────────────────────────────────────────────

/// A sandboxed bash interpreter for use from JavaScript.
#[wasm_bindgen]
pub struct WasmBash {
    inner: RustBash,
}

#[wasm_bindgen]
impl WasmBash {
    /// Create a new WasmBash instance.
    ///
    /// `config` is a JS object with optional fields:
    /// - `files`: `Record<string, string>` — seed virtual filesystem
    /// - `env`: `Record<string, string>` — environment variables
    /// - `cwd`: `string` — working directory (default: "/")
    /// - `executionLimits`: partial execution limits
    #[wasm_bindgen(constructor)]
    pub fn new(config: JsValue) -> Result<WasmBash, JsError> {
        let mut builder = RustBashBuilder::new();

        if !config.is_undefined() && !config.is_null() {
            // Parse files
            if let Ok(files_val) = Reflect::get(&config, &"files".into()) {
                if !files_val.is_undefined() && !files_val.is_null() {
                    let files = parse_string_record(&files_val)?;
                    let file_map: HashMap<String, Vec<u8>> = files
                        .into_iter()
                        .map(|(k, v)| (k, v.into_bytes()))
                        .collect();
                    builder = builder.files(file_map);
                }
            }

            // Parse env
            if let Ok(env_val) = Reflect::get(&config, &"env".into()) {
                if !env_val.is_undefined() && !env_val.is_null() {
                    let env = parse_string_record(&env_val)?;
                    builder = builder.env(env);
                }
            }

            // Parse cwd
            if let Ok(cwd_val) = Reflect::get(&config, &"cwd".into()) {
                if let Some(cwd) = cwd_val.as_string() {
                    builder = builder.cwd(cwd);
                }
            }

            // Parse executionLimits
            if let Ok(limits_val) = Reflect::get(&config, &"executionLimits".into()) {
                if !limits_val.is_undefined() && !limits_val.is_null() {
                    let limits = parse_execution_limits(&limits_val)?;
                    builder = builder.execution_limits(limits);
                }
            }
        }

        let inner = builder.build().map_err(|e| JsError::new(&e.to_string()))?;
        Ok(WasmBash { inner })
    }

    /// Execute a shell command string.
    ///
    /// Returns `{ stdout: string, stderr: string, exitCode: number }`.
    pub fn exec(&mut self, command: &str) -> Result<JsValue, JsError> {
        let result = self
            .inner
            .exec(command)
            .map_err(|e| JsError::new(&e.to_string()))?;
        exec_result_to_js(&result)
    }

    /// Execute a shell command with per-exec options.
    ///
    /// `options` is a JS object with optional fields:
    /// - `env`: `Record<string, string>` — per-exec environment overrides
    /// - `cwd`: `string` — per-exec working directory
    /// - `stdin`: `string` — standard input content
    pub fn exec_with_options(
        &mut self,
        command: &str,
        options: JsValue,
    ) -> Result<JsValue, JsError> {
        let saved_cwd = self.inner.state.cwd.clone();
        let mut overwritten_env: Vec<(String, Option<crate::interpreter::Variable>)> = Vec::new();

        let result = (|| -> Result<JsValue, JsError> {
            if !options.is_undefined() && !options.is_null() {
                // Check if we should replace the entire environment
                let replace_env = Reflect::get(&options, &"replaceEnv".into())
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Apply per-exec env overrides
                if let Ok(env_val) = Reflect::get(&options, &"env".into()) {
                    if !env_val.is_undefined() && !env_val.is_null() {
                        let env = parse_string_record(&env_val)?;

                        if replace_env {
                            // Save all existing env vars and clear them
                            for (key, var) in self.inner.state.env.drain() {
                                overwritten_env.push((key, Some(var)));
                            }
                        }

                        for (key, value) in env {
                            if !replace_env {
                                let old = self.inner.state.env.get(&key).cloned();
                                overwritten_env.push((key.clone(), old));
                            }
                            self.inner.state.env.insert(
                                key,
                                crate::interpreter::Variable {
                                    value: crate::interpreter::VariableValue::Scalar(value),
                                    attrs: crate::interpreter::VariableAttrs::EXPORTED,
                                },
                            );
                        }
                    }
                }

                // Apply per-exec cwd
                if let Ok(cwd_val) = Reflect::get(&options, &"cwd".into()) {
                    if let Some(cwd) = cwd_val.as_string() {
                        self.inner.state.cwd = cwd;
                    }
                }

                // Handle stdin by wrapping command with heredoc
                if let Ok(stdin_val) = Reflect::get(&options, &"stdin".into()) {
                    if let Some(stdin) = stdin_val.as_string() {
                        let delimiter = if stdin.contains("__WASM_STDIN__") {
                            "__WASM_STDIN_BOUNDARY__"
                        } else {
                            "__WASM_STDIN__"
                        };
                        let full_command =
                            format!("{command} <<'{delimiter}'\n{stdin}\n{delimiter}");
                        let result = self
                            .inner
                            .exec(&full_command)
                            .map_err(|e| JsError::new(&e.to_string()))?;
                        return exec_result_to_js(&result);
                    }
                }
            }

            self.exec(command)
        })();

        // Restore state
        self.inner.state.cwd = saved_cwd;
        for (key, old_val) in overwritten_env {
            match old_val {
                Some(var) => {
                    self.inner.state.env.insert(key, var);
                }
                None => {
                    self.inner.state.env.remove(&key);
                }
            }
        }

        result
    }

    /// Write a file to the virtual filesystem.
    pub fn write_file(&mut self, path: &str, content: &str) -> Result<(), JsError> {
        let p = Path::new(path);
        if let Some(parent) = p.parent() {
            if parent != Path::new("/") {
                self.inner
                    .state
                    .fs
                    .mkdir_p(parent)
                    .map_err(|e| JsError::new(&e.to_string()))?;
            }
        }
        self.inner
            .state
            .fs
            .write_file(p, content.as_bytes())
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Read a file from the virtual filesystem.
    pub fn read_file(&self, path: &str) -> Result<String, JsError> {
        let data = self
            .inner
            .state
            .fs
            .read_file(Path::new(path))
            .map_err(|e| JsError::new(&e.to_string()))?;
        String::from_utf8(data).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Create a directory in the virtual filesystem.
    pub fn mkdir(&mut self, path: &str, recursive: bool) -> Result<(), JsError> {
        let p = Path::new(path);
        if recursive {
            self.inner
                .state
                .fs
                .mkdir_p(p)
                .map_err(|e| JsError::new(&e.to_string()))
        } else {
            self.inner
                .state
                .fs
                .mkdir(p)
                .map_err(|e| JsError::new(&e.to_string()))
        }
    }

    /// Get the current working directory.
    pub fn cwd(&self) -> String {
        self.inner.cwd().to_string()
    }

    /// Get the exit code of the last executed command.
    pub fn last_exit_code(&self) -> i32 {
        self.inner.last_exit_code()
    }

    /// Get the names of all registered commands.
    pub fn command_names(&self) -> Vec<String> {
        self.inner
            .command_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Register a custom command backed by a JavaScript callback.
    ///
    /// The callback receives `(args: string[], ctx: object)` and must return
    /// `{ stdout: string, stderr: string, exitCode: number }` synchronously.
    ///
    /// The `ctx` object provides:
    /// - `cwd: string` — current working directory
    /// - `stdin: string` — piped input from the previous pipeline stage
    /// - `env: Record<string, string>` — environment variables
    /// - `fs` — virtual filesystem proxy (readFileSync, writeFileSync, …)
    /// - `exec(command: string) → { stdout, stderr, exitCode }` — execute a
    ///   sub-command through the shell interpreter.  **Must only be called
    ///   synchronously** within the callback; do **not** store or defer it.
    pub fn register_command(&mut self, name: &str, callback: Function) -> Result<(), JsError> {
        let fs_proxy = build_fs_proxy(&self.inner.state.fs);
        let cmd = JsBridgeCommand {
            name: name.to_string(),
            callback,
            fs_proxy,
        };
        self.inner
            .state
            .commands
            .insert(name.to_string(), Box::new(cmd));
        Ok(())
    }

    /// Check whether a path exists in the virtual filesystem.
    pub fn exists(&self, path: &str) -> bool {
        self.inner.exists(path)
    }

    /// List directory entries.
    ///
    /// Returns a JS array of `{ name: string, isDirectory: boolean }` objects.
    pub fn readdir(&self, path: &str) -> Result<JsValue, JsError> {
        let entries = self
            .inner
            .readdir(path)
            .map_err(|e| JsError::new(&e.to_string()))?;
        let arr = Array::new();
        for entry in entries {
            let obj = Object::new();
            let _ = Reflect::set(&obj, &"name".into(), &JsValue::from_str(&entry.name));
            let _ = Reflect::set(
                &obj,
                &"isDirectory".into(),
                &JsValue::from_bool(entry.node_type == NodeType::Directory),
            );
            arr.push(&obj.into());
        }
        Ok(arr.into())
    }

    /// Get metadata for a path.
    ///
    /// Returns `{ size: number, isDirectory: boolean, isFile: boolean, isSymlink: boolean }`.
    pub fn stat(&self, path: &str) -> Result<JsValue, JsError> {
        let meta = self
            .inner
            .stat(path)
            .map_err(|e| JsError::new(&e.to_string()))?;
        let obj = Object::new();
        let _ = Reflect::set(&obj, &"size".into(), &JsValue::from_f64(meta.size as f64));
        let _ = Reflect::set(
            &obj,
            &"isDirectory".into(),
            &JsValue::from_bool(meta.node_type == NodeType::Directory),
        );
        let _ = Reflect::set(
            &obj,
            &"isFile".into(),
            &JsValue::from_bool(meta.node_type == NodeType::File),
        );
        let _ = Reflect::set(
            &obj,
            &"isSymlink".into(),
            &JsValue::from_bool(meta.node_type == NodeType::Symlink),
        );
        Ok(obj.into())
    }

    /// Remove a file from the virtual filesystem.
    pub fn remove_file(&mut self, path: &str) -> Result<(), JsError> {
        self.inner
            .remove_file(path)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Recursively remove a directory and its contents.
    pub fn remove_dir_all(&mut self, path: &str) -> Result<(), JsError> {
        self.inner
            .remove_dir_all(path)
            .map_err(|e| JsError::new(&e.to_string()))
    }
}

// ── JsBridgeCommand ──────────────────────────────────────────────────

/// A command that delegates execution to a JavaScript callback function.
struct JsBridgeCommand {
    name: String,
    callback: Function,
    fs_proxy: JsValue,
}

// SAFETY: wasm32-unknown-unknown has no threads; Send + Sync are trivially safe.
// This does NOT hold for targets with thread support (e.g. wasm32-wasi-threads).
unsafe impl Send for JsBridgeCommand {}
unsafe impl Sync for JsBridgeCommand {}

impl VirtualCommand for JsBridgeCommand {
    fn name(&self) -> &str {
        &self.name
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        // Build args array
        let js_args = Array::new();
        for arg in args {
            js_args.push(&JsValue::from_str(arg));
        }

        // Build context object (reuses the pre-built fs proxy).
        // `_exec_closure` must stay alive until after `call2` returns so the
        // JS `exec` function pointer remains valid.
        let (js_ctx, _exec_closure) = build_js_command_context(ctx, &self.fs_proxy);

        // Call the JS callback: callback(args, ctx)
        let js_args_val: JsValue = js_args.into();
        let result = self.callback.call2(&JsValue::NULL, &js_args_val, &js_ctx);

        match result {
            Ok(val) => parse_command_result(&val),
            Err(e) => {
                let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
                CommandResult {
                    stderr: format!("{}: {}\n", self.name, msg),
                    exit_code: 1,
                    ..Default::default()
                }
            }
        }
    }
}

// ── Helper functions ─────────────────────────────────────────────────

fn parse_string_record(val: &JsValue) -> Result<HashMap<String, String>, JsError> {
    if !val.is_object() {
        return Err(JsError::new("expected a plain object"));
    }
    let mut map = HashMap::new();
    let keys = Object::keys(&val.clone().into());
    for i in 0..keys.length() {
        let key = keys.get(i);
        if let Some(key_str) = key.as_string() {
            if let Ok(value) = Reflect::get(val, &key) {
                if let Some(value_str) = value.as_string() {
                    map.insert(key_str, value_str);
                }
            }
        }
    }
    Ok(map)
}

fn parse_execution_limits(val: &JsValue) -> Result<ExecutionLimits, JsError> {
    let mut limits = ExecutionLimits::default();

    if let Ok(v) = Reflect::get(val, &"maxCommandCount".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_command_count = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxExecutionTimeSecs".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_execution_time = std::time::Duration::from_secs_f64(n);
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxLoopIterations".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_loop_iterations = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxOutputSize".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_output_size = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxCallDepth".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_call_depth = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxStringLength".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_string_length = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxGlobResults".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_glob_results = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxSubstitutionDepth".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_substitution_depth = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxHeredocSize".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_heredoc_size = n as usize;
        }
    }
    if let Ok(v) = Reflect::get(val, &"maxBraceExpansion".into()) {
        if let Some(n) = v.as_f64() {
            limits.max_brace_expansion = n as usize;
        }
    }

    Ok(limits)
}

fn exec_result_to_js(result: &crate::interpreter::ExecResult) -> Result<JsValue, JsError> {
    let obj = Object::new();
    Reflect::set(&obj, &"stdout".into(), &JsValue::from_str(&result.stdout))
        .map_err(|e| JsError::new(&format!("{e:?}")))?;
    Reflect::set(&obj, &"stderr".into(), &JsValue::from_str(&result.stderr))
        .map_err(|e| JsError::new(&format!("{e:?}")))?;
    Reflect::set(
        &obj,
        &"exitCode".into(),
        &JsValue::from_f64(f64::from(result.exit_code)),
    )
    .map_err(|e| JsError::new(&format!("{e:?}")))?;
    Ok(obj.into())
}

/// Convert a `CommandResult` into a JS object `{ stdout, stderr, exitCode }`.
fn command_result_to_js(result: &CommandResult) -> JsValue {
    let obj = Object::new();
    let _ = Reflect::set(&obj, &"stdout".into(), &JsValue::from_str(&result.stdout));
    let _ = Reflect::set(&obj, &"stderr".into(), &JsValue::from_str(&result.stderr));
    let _ = Reflect::set(
        &obj,
        &"exitCode".into(),
        &JsValue::from_f64(f64::from(result.exit_code)),
    );
    obj.into()
}

/// Read two pointer-sized values from a stack address.
/// Used to decompose a fat reference (&dyn Fn) into data + vtable pointers
/// while erasing the source lifetime.
///
/// # Safety
/// `src` must point to a valid fat reference (2 × usize bytes).
unsafe fn read_fat_ref_as_raw(src: *const u8) -> [usize; 2] {
    unsafe { std::ptr::read(src as *const [usize; 2]) }
}

fn build_js_command_context(
    ctx: &CommandContext,
    fs_proxy: &JsValue,
) -> (JsValue, Option<Closure<dyn FnMut(String) -> JsValue>>) {
    let obj = Object::new();

    // cwd
    let _ = Reflect::set(&obj, &"cwd".into(), &JsValue::from_str(ctx.cwd));

    // stdin
    let _ = Reflect::set(&obj, &"stdin".into(), &JsValue::from_str(ctx.stdin));

    // env as Record<string, string>
    let env_obj = Object::new();
    for (key, value) in ctx.env {
        let _ = Reflect::set(&env_obj, &JsValue::from_str(key), &JsValue::from_str(value));
    }
    let _ = Reflect::set(&obj, &"env".into(), &env_obj.into());

    // Pre-built fs proxy
    let _ = Reflect::set(&obj, &"fs".into(), fs_proxy);

    // exec(command: string) -> { stdout, stderr, exitCode }
    let exec_closure = ctx.exec.map(|exec_cb| {
        // SAFETY: wasm32-unknown-unknown is single-threaded and the JS callback
        // that receives this closure is invoked synchronously within
        // `JsBridgeCommand::execute`. The `exec_cb` reference points to the
        // `exec_callback` local created by `dispatch_command` in walker.rs,
        // which outlives the entire `execute` call. The returned `Closure` is
        // kept alive by the caller (`_exec_closure`) until after `call2`
        // returns, so the raw pointer is never dangling when dereferenced.
        //
        // INVARIANT: If JS stores `ctx.exec` and calls it after the
        // synchronous callback returns, this would be UB. The
        // `register_command` doc specifies that `ctx.exec` must only be
        // called synchronously within the callback.
        //
        // We decompose the fat reference into two usize values (data + vtable)
        // so the closure captures only 'static data. This is required because
        // wasm_bindgen::Closure demands 'static.
        type ExecFn = dyn Fn(&str) -> Result<CommandResult, RustBashError>;
        // Read the fat reference's raw bytes (data ptr + vtable ptr) through a
        // helper that takes *const u8, breaking the borrow chain so the
        // resulting [usize; 2] is 'static. Required because Closure demands 'static.
        // addr_of! creates a raw pointer without going through the borrow checker.
        let raw_parts: [usize; 2] =
            unsafe { read_fat_ref_as_raw(std::ptr::addr_of!(exec_cb) as *const u8) };

        let closure = Closure::wrap(Box::new(move |cmd: String| -> JsValue {
            let exec_fn: &ExecFn = unsafe { std::mem::transmute::<[usize; 2], &ExecFn>(raw_parts) };
            match exec_fn(&cmd) {
                Ok(result) => command_result_to_js(&result),
                Err(e) => command_result_to_js(&CommandResult {
                    stderr: e.to_string(),
                    exit_code: 1,
                    ..Default::default()
                }),
            }
        }) as Box<dyn FnMut(String) -> JsValue>);
        let _ = Reflect::set(&obj, &"exec".into(), closure.as_ref());
        closure
    });

    (obj.into(), exec_closure)
}

fn build_fs_proxy(fs: &Arc<dyn VirtualFs>) -> JsValue {
    let obj = Object::new();

    // We create closures that capture a clone of the Arc<dyn VirtualFs>.
    // Each closure is converted to a js_sys::Function via wasm_bindgen::closure::Closure.

    // readFileSync(path: string) -> string
    let fs_clone = Arc::clone(fs);
    let read_file = Closure::wrap(Box::new(move |path: String| -> Result<JsValue, JsValue> {
        let data = fs_clone
            .read_file(Path::new(&path))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let s = String::from_utf8(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(JsValue::from_str(&s))
    }) as Box<dyn FnMut(String) -> Result<JsValue, JsValue>>);
    let _ = Reflect::set(&obj, &"readFileSync".into(), read_file.as_ref());
    read_file.forget();

    // writeFileSync(path: string, content: string)
    let fs_clone = Arc::clone(fs);
    let write_file = Closure::wrap(Box::new(
        move |path: String, content: String| -> Result<JsValue, JsValue> {
            let p = Path::new(&path);
            if let Some(parent) = p.parent() {
                if parent != Path::new("/") {
                    let _ = fs_clone.mkdir_p(parent);
                }
            }
            fs_clone
                .write_file(p, content.as_bytes())
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            Ok(JsValue::UNDEFINED)
        },
    )
        as Box<dyn FnMut(String, String) -> Result<JsValue, JsValue>>);
    let _ = Reflect::set(&obj, &"writeFileSync".into(), write_file.as_ref());
    write_file.forget();

    // existsSync(path: string) -> boolean
    let fs_clone = Arc::clone(fs);
    let exists = Closure::wrap(Box::new(move |path: String| -> JsValue {
        JsValue::from_bool(fs_clone.exists(Path::new(&path)))
    }) as Box<dyn FnMut(String) -> JsValue>);
    let _ = Reflect::set(&obj, &"existsSync".into(), exists.as_ref());
    exists.forget();

    // mkdirSync(path: string, opts?: { recursive: boolean })
    let fs_clone = Arc::clone(fs);
    let mkdir_fn = Closure::wrap(Box::new(
        move |path: String, opts: JsValue| -> Result<JsValue, JsValue> {
            let p = Path::new(&path);
            let recursive = if !opts.is_undefined() && !opts.is_null() {
                Reflect::get(&opts, &"recursive".into())
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else {
                false
            };
            if recursive {
                fs_clone
                    .mkdir_p(p)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            } else {
                fs_clone
                    .mkdir(p)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            }
            Ok(JsValue::UNDEFINED)
        },
    )
        as Box<dyn FnMut(String, JsValue) -> Result<JsValue, JsValue>>);
    let _ = Reflect::set(&obj, &"mkdirSync".into(), mkdir_fn.as_ref());
    mkdir_fn.forget();

    // readdirSync(path: string) -> string[]
    let fs_clone = Arc::clone(fs);
    let readdir = Closure::wrap(Box::new(move |path: String| -> Result<JsValue, JsValue> {
        let entries = fs_clone
            .readdir(Path::new(&path))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let arr = Array::new();
        for entry in entries {
            arr.push(&JsValue::from_str(&entry.name));
        }
        Ok(arr.into())
    }) as Box<dyn FnMut(String) -> Result<JsValue, JsValue>>);
    let _ = Reflect::set(&obj, &"readdirSync".into(), readdir.as_ref());
    readdir.forget();

    // rmSync(path: string, opts?: { recursive: boolean })
    let fs_clone = Arc::clone(fs);
    let rm_fn = Closure::wrap(Box::new(
        move |path: String, opts: JsValue| -> Result<JsValue, JsValue> {
            let p = Path::new(&path);
            let recursive = if !opts.is_undefined() && !opts.is_null() {
                Reflect::get(&opts, &"recursive".into())
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else {
                false
            };
            if recursive {
                fs_clone
                    .remove_dir_all(p)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            } else if fs_clone
                .stat(p)
                .map(|m| m.node_type == crate::vfs::NodeType::Directory)
                .unwrap_or(false)
            {
                fs_clone
                    .remove_dir(p)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            } else {
                fs_clone
                    .remove_file(p)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            }
            Ok(JsValue::UNDEFINED)
        },
    )
        as Box<dyn FnMut(String, JsValue) -> Result<JsValue, JsValue>>);
    let _ = Reflect::set(&obj, &"rmSync".into(), rm_fn.as_ref());
    rm_fn.forget();

    // statSync(path: string) -> { size: number, isFile: boolean, isDirectory: boolean }
    let fs_clone = Arc::clone(fs);
    let stat_fn = Closure::wrap(Box::new(move |path: String| -> Result<JsValue, JsValue> {
        let meta = fs_clone
            .stat(Path::new(&path))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let obj = Object::new();
        let _ = Reflect::set(&obj, &"size".into(), &JsValue::from_f64(meta.size as f64));
        let _ = Reflect::set(
            &obj,
            &"isFile".into(),
            &JsValue::from_bool(meta.node_type == crate::vfs::NodeType::File),
        );
        let _ = Reflect::set(
            &obj,
            &"isDirectory".into(),
            &JsValue::from_bool(meta.node_type == crate::vfs::NodeType::Directory),
        );
        Ok(obj.into())
    }) as Box<dyn FnMut(String) -> Result<JsValue, JsValue>>);
    let _ = Reflect::set(&obj, &"statSync".into(), stat_fn.as_ref());
    stat_fn.forget();

    obj.into()
}

fn parse_command_result(val: &JsValue) -> CommandResult {
    let stdout = Reflect::get(val, &"stdout".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let stderr = Reflect::get(val, &"stderr".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let exit_code = Reflect::get(val, &"exitCode".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;

    CommandResult {
        stdout,
        stderr,
        exit_code,
        stdout_bytes: None,
    }
}
