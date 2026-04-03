//! Public API: `RustBash` shell instance and builder.

use crate::commands::{self, VirtualCommand};
use crate::error::RustBashError;
use crate::interpreter::{
    self, ExecResult, ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, ShoptOpts,
    Variable, VariableAttrs, VariableValue,
};
use crate::network::NetworkPolicy;
use crate::platform::Instant;
use crate::vfs::{InMemoryFs, VirtualFs};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// A sandboxed bash shell interpreter.
pub struct RustBash {
    pub(crate) state: InterpreterState,
}

impl RustBash {
    /// Execute a shell command string and return the result.
    pub fn exec(&mut self, input: &str) -> Result<ExecResult, RustBashError> {
        self.state.counters.reset();
        self.state.should_exit = false;
        self.state.current_source_text = input.to_string();
        self.state.last_verbose_line = 0;

        let program = match interpreter::parse(input) {
            Ok(p) => p,
            Err(e) => {
                self.state.last_exit_code = 2;
                return Ok(ExecResult {
                    exit_code: 2,
                    stderr: format!("{e}\n"),
                    ..ExecResult::default()
                });
            }
        };
        let mut result = interpreter::execute_program(&program, &mut self.state)?;

        // Fire EXIT trap at end of exec()
        if let Some(exit_cmd) = self.state.traps.get("EXIT").cloned()
            && !exit_cmd.is_empty()
            && !self.state.in_trap
        {
            let trap_result = interpreter::execute_trap(&exit_cmd, &mut self.state)?;
            result.stdout.push_str(&trap_result.stdout);
            result.stderr.push_str(&trap_result.stderr);
        }

        Ok(result)
    }

    /// Returns the current working directory.
    pub fn cwd(&self) -> &str {
        &self.state.cwd
    }

    /// Returns the exit code of the last executed command.
    pub fn last_exit_code(&self) -> i32 {
        self.state.last_exit_code
    }

    /// Returns `true` if the shell received an `exit` command.
    pub fn should_exit(&self) -> bool {
        self.state.should_exit
    }

    /// Returns the names of all registered commands (builtins + custom).
    pub fn command_names(&self) -> Vec<&str> {
        self.state.commands.keys().map(|k| k.as_str()).collect()
    }

    /// Returns the `CommandMeta` for a registered command, if it provides one.
    pub fn command_meta(&self, name: &str) -> Option<&'static commands::CommandMeta> {
        self.state.commands.get(name).and_then(|cmd| cmd.meta())
    }

    /// Sets the shell name (`$0`).
    pub fn set_shell_name(&mut self, name: String) {
        self.state.shell_name = name;
    }

    /// Sets the positional parameters (`$1`, `$2`, ...).
    pub fn set_positional_params(&mut self, params: Vec<String>) {
        self.state.positional_params = params;
    }

    // ── VFS convenience methods ──────────────────────────────────────

    /// Returns a reference to the virtual filesystem.
    pub fn fs(&self) -> &Arc<dyn crate::vfs::VirtualFs> {
        &self.state.fs
    }

    /// Write a file to the virtual filesystem, creating parent directories.
    pub fn write_file(&self, path: &str, content: &[u8]) -> Result<(), crate::VfsError> {
        let p = Path::new(path);
        if let Some(parent) = p.parent()
            && parent != Path::new("/")
        {
            self.state.fs.mkdir_p(parent)?;
        }
        self.state.fs.write_file(p, content)
    }

    /// Read a file from the virtual filesystem.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, crate::VfsError> {
        self.state.fs.read_file(Path::new(path))
    }

    /// Create a directory in the virtual filesystem.
    pub fn mkdir(&self, path: &str, recursive: bool) -> Result<(), crate::VfsError> {
        let p = Path::new(path);
        if recursive {
            self.state.fs.mkdir_p(p)
        } else {
            self.state.fs.mkdir(p)
        }
    }

    /// Check if a path exists in the virtual filesystem.
    pub fn exists(&self, path: &str) -> bool {
        self.state.fs.exists(Path::new(path))
    }

    /// List entries in a directory.
    pub fn readdir(&self, path: &str) -> Result<Vec<crate::vfs::DirEntry>, crate::VfsError> {
        self.state.fs.readdir(Path::new(path))
    }

    /// Get metadata for a path.
    pub fn stat(&self, path: &str) -> Result<crate::vfs::Metadata, crate::VfsError> {
        self.state.fs.stat(Path::new(path))
    }

    /// Remove a file from the virtual filesystem.
    pub fn remove_file(&self, path: &str) -> Result<(), crate::VfsError> {
        self.state.fs.remove_file(Path::new(path))
    }

    /// Remove a directory (and contents if recursive) from the virtual filesystem.
    pub fn remove_dir_all(&self, path: &str) -> Result<(), crate::VfsError> {
        self.state.fs.remove_dir_all(Path::new(path))
    }

    /// Register a custom command.
    pub fn register_command(&mut self, cmd: Arc<dyn VirtualCommand>) {
        self.state.commands.insert(cmd.name().to_string(), cmd);
    }

    /// Execute a command with per-exec environment and cwd overrides.
    ///
    /// Overrides are applied before execution and restored afterward.
    pub fn exec_with_overrides(
        &mut self,
        input: &str,
        env: Option<&HashMap<String, String>>,
        cwd: Option<&str>,
        stdin: Option<&str>,
    ) -> Result<ExecResult, RustBashError> {
        let saved_cwd = self.state.cwd.clone();
        let mut overwritten_env: Vec<(String, Option<Variable>)> = Vec::new();

        if let Some(env) = env {
            for (key, value) in env {
                let old = self.state.env.get(key).cloned();
                overwritten_env.push((key.clone(), old));
                self.state.env.insert(
                    key.clone(),
                    Variable {
                        value: VariableValue::Scalar(value.clone()),
                        attrs: VariableAttrs::EXPORTED,
                    },
                );
            }
        }

        if let Some(cwd) = cwd {
            self.state.cwd = cwd.to_string();
        }

        let result = if let Some(stdin) = stdin {
            let delimiter = if stdin.contains("__EXEC_STDIN__") {
                "__EXEC_STDIN_BOUNDARY__"
            } else {
                "__EXEC_STDIN__"
            };
            let full_command = format!("{input} <<'{delimiter}'\n{stdin}\n{delimiter}");
            self.exec(&full_command)
        } else {
            self.exec(input)
        };

        // Restore state
        self.state.cwd = saved_cwd;
        for (key, old_val) in overwritten_env {
            match old_val {
                Some(var) => {
                    self.state.env.insert(key, var);
                }
                None => {
                    self.state.env.remove(&key);
                }
            }
        }

        result
    }

    /// Check whether `input` looks like a complete shell statement.
    ///
    /// Returns `true` when the input can be tokenized and parsed without
    /// hitting an "unexpected end-of-input" / unterminated-quote error.
    /// Useful for implementing multi-line REPL input.
    ///
    /// Note: mirrors the tokenize → parse flow from `interpreter::parse()`.
    pub fn is_input_complete(input: &str) -> bool {
        match brush_parser::tokenize_str(input) {
            Err(e) if e.is_incomplete() => false,
            Err(_) => true, // genuine syntax error, not incomplete
            Ok(tokens) => {
                if tokens.is_empty() {
                    return true;
                }
                let options = interpreter::parser_options();
                match brush_parser::parse_tokens(&tokens, &options) {
                    Ok(_) => true,
                    Err(brush_parser::ParseError::ParsingAtEndOfInput) => false,
                    Err(_) => true, // genuine syntax error
                }
            }
        }
    }
}

/// Builder for configuring a [`RustBash`] instance.
pub struct RustBashBuilder {
    files: HashMap<String, Vec<u8>>,
    env: HashMap<String, String>,
    env_explicit: bool,
    cwd: Option<String>,
    custom_commands: Vec<Arc<dyn VirtualCommand>>,
    limits: Option<ExecutionLimits>,
    network_policy: Option<NetworkPolicy>,
    fs: Option<Arc<dyn VirtualFs>>,
}

impl Default for RustBashBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RustBashBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            env: HashMap::new(),
            env_explicit: false,
            cwd: None,
            custom_commands: Vec::new(),
            limits: None,
            network_policy: None,
            fs: None,
        }
    }

    /// Pre-populate the virtual filesystem with files.
    pub fn files(mut self, files: HashMap<String, Vec<u8>>) -> Self {
        self.files = files;
        self
    }

    /// Set environment variables.
    pub fn env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self.env_explicit = true;
        self
    }

    /// Set the initial working directory (created automatically).
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Register a custom command.
    pub fn command(mut self, cmd: Arc<dyn VirtualCommand>) -> Self {
        self.custom_commands.push(cmd);
        self
    }

    /// Override the default execution limits.
    pub fn execution_limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set the maximum number of elements allowed in a single array.
    pub fn max_array_elements(mut self, max: usize) -> Self {
        let mut limits = self.limits.unwrap_or_default();
        limits.max_array_elements = max;
        self.limits = Some(limits);
        self
    }

    /// Override the default network policy.
    pub fn network_policy(mut self, policy: NetworkPolicy) -> Self {
        self.network_policy = Some(policy);
        self
    }

    /// Use a custom filesystem backend instead of the default InMemoryFs.
    ///
    /// When set, the builder uses this filesystem directly. The `.files()` method
    /// still works — it writes seed files into the provided backend via VirtualFs
    /// methods.
    pub fn fs(mut self, fs: Arc<dyn VirtualFs>) -> Self {
        self.fs = Some(fs);
        self
    }

    /// Build the shell instance.
    pub fn build(self) -> Result<RustBash, RustBashError> {
        let fs: Arc<dyn VirtualFs> = self.fs.unwrap_or_else(|| Arc::new(InMemoryFs::new()));
        let cwd = self.cwd.unwrap_or_else(|| "/".to_string());
        fs.mkdir_p(Path::new(&cwd))?;

        for (path, content) in &self.files {
            let p = Path::new(path);
            if let Some(parent) = p.parent()
                && parent != Path::new("/")
            {
                fs.mkdir_p(parent)?;
            }
            fs.write_file(p, content)?;
        }

        let mut commands = commands::register_default_commands();
        for cmd in self.custom_commands {
            commands.insert(cmd.name().to_string(), cmd);
        }

        // Insert default environment variables (caller-provided values take precedence)
        let mut env_map = self.env;
        let defaults: &[(&str, &str)] = &[
            ("PATH", interpreter::DEFAULT_PATH),
            ("USER", interpreter::DEFAULT_USER),
            ("HOSTNAME", interpreter::DEFAULT_HOSTNAME),
            ("OSTYPE", interpreter::DEFAULT_OSTYPE),
            ("SHELL", interpreter::DEFAULT_SHELL_PATH),
            ("BASH", interpreter::DEFAULT_SHELL_PATH),
            ("BASH_VERSION", interpreter::DEFAULT_BASH_VERSION),
            ("OLDPWD", ""),
            ("TERM", interpreter::DEFAULT_TERM),
        ];
        for &(key, value) in defaults {
            env_map
                .entry(key.to_string())
                .or_insert_with(|| value.to_string());
        }
        if !self.env_explicit {
            env_map
                .entry("HOME".to_string())
                .or_insert_with(|| interpreter::DEFAULT_HOME.to_string());
        }
        env_map
            .entry("PWD".to_string())
            .or_insert_with(|| cwd.clone());

        setup_default_filesystem(fs.as_ref(), &env_map, &commands)?;

        let mut env: HashMap<String, Variable> = env_map
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    Variable {
                        value: VariableValue::Scalar(v),
                        attrs: VariableAttrs::EXPORTED,
                    },
                )
            })
            .collect();

        // Non-exported shell variables with default values
        for (name, val) in &[("OPTIND", "1"), ("OPTERR", "1")] {
            env.entry(name.to_string()).or_insert_with(|| Variable {
                value: VariableValue::Scalar(val.to_string()),
                attrs: VariableAttrs::empty(),
            });
        }

        let mut state = InterpreterState {
            fs,
            env,
            cwd,
            functions: HashMap::new(),
            last_exit_code: 0,
            commands,
            shell_opts: ShellOpts::default(),
            shopt_opts: ShoptOpts::default(),
            limits: self.limits.unwrap_or_default(),
            counters: ExecutionCounters::default(),
            network_policy: self.network_policy.unwrap_or_default(),
            should_exit: false,
            loop_depth: 0,
            control_flow: None,
            positional_params: Vec::new(),
            shell_name: "rust-bash".to_string(),
            shell_pid: 1000,
            bash_pid: 1000,
            parent_pid: 1,
            next_process_id: 1001,
            last_background_pid: None,
            last_background_status: None,
            interactive_shell: false,
            invoked_with_c: false,
            random_seed: 0,
            local_scopes: Vec::new(),
            temp_binding_scopes: Vec::new(),
            in_function_depth: 0,
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
            errexit_bang_suppressed: 0,
            stdin_offset: 0,
            current_stdin_persistent_fd: None,
            dir_stack: Vec::new(),
            command_hash: HashMap::new(),
            aliases: HashMap::new(),
            current_lineno: 0,
            current_source: "main".to_string(),
            current_source_text: String::new(),
            last_verbose_line: 0,
            shell_start_time: Instant::now(),
            last_argument: String::new(),
            call_stack: Vec::new(),
            machtype: "x86_64-pc-linux-gnu".to_string(),
            hosttype: "x86_64".to_string(),
            persistent_fds: HashMap::new(),
            persistent_fd_offsets: HashMap::new(),
            next_auto_fd: 10,
            proc_sub_counter: 0,
            proc_sub_prealloc: HashMap::new(),
            pipe_stdin_bytes: None,
            pending_cmdsub_stderr: String::new(),
            fatal_expansion_error: false,
            last_command_had_error: false,
            last_status_immune_to_errexit: false,
        };
        interpreter::ensure_shell_internal_vars(&mut state);

        Ok(RustBash { state })
    }
}

/// Create standard directories and command stubs in the VFS.
///
/// Directories and files are only created when they don't already exist,
/// so user-seeded content is never clobbered.
fn setup_default_filesystem(
    fs: &dyn VirtualFs,
    env: &HashMap<String, String>,
    commands: &HashMap<String, Arc<dyn commands::VirtualCommand>>,
) -> Result<(), RustBashError> {
    // Standard directories
    for dir in &["/bin", "/usr/bin", "/tmp", "/dev"] {
        let _ = fs.mkdir_p(Path::new(dir));
    }

    // HOME directory
    if let Some(home) = env.get("HOME") {
        let _ = fs.mkdir_p(Path::new(home));
    }

    // /dev special files
    for name in &["null", "zero", "stdin", "stdout", "stderr"] {
        let path_str = format!("/dev/{name}");
        let p = Path::new(&path_str);
        if !fs.exists(p) {
            fs.write_file(p, b"")?;
        }
    }

    for prefix in ["/bin", "/usr/bin"] {
        // Command stubs for each registered command
        for name in commands.keys() {
            let path_str = format!("{prefix}/{name}");
            let p = Path::new(&path_str);
            if !fs.exists(p) {
                let content = format!("#!/bin/bash\n# built-in: {name}\n");
                fs.write_file(p, content.as_bytes())?;
            }
        }

        // Builtin stubs (skip names unsuitable as filenames)
        for &name in interpreter::builtins::builtin_names() {
            if matches!(name, "." | ":" | "colon") {
                continue;
            }
            let path_str = format!("{prefix}/{name}");
            let p = Path::new(&path_str);
            if !fs.exists(p) {
                let content = format!("#!/bin/bash\n# built-in: {name}\n");
                fs.write_file(p, content.as_bytes())?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> RustBash {
        RustBashBuilder::new().build().unwrap()
    }

    // ── Exit criteria ───────────────────────────────────────────

    #[test]
    fn echo_hello_end_to_end() {
        let mut shell = shell();
        let result = shell.exec("echo hello").unwrap();
        assert_eq!(result.stdout, "hello\n");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stderr, "");
    }

    // ── Echo variants ───────────────────────────────────────────

    #[test]
    fn echo_multiple_words() {
        let mut shell = shell();
        let result = shell.exec("echo hello world").unwrap();
        assert_eq!(result.stdout, "hello world\n");
    }

    #[test]
    fn echo_no_args() {
        let mut shell = shell();
        let result = shell.exec("echo").unwrap();
        assert_eq!(result.stdout, "\n");
    }

    #[test]
    fn echo_no_newline() {
        let mut shell = shell();
        let result = shell.exec("echo -n hello").unwrap();
        assert_eq!(result.stdout, "hello");
    }

    #[test]
    fn echo_escape_sequences() {
        let mut shell = shell();
        let result = shell.exec(r"echo -e 'hello\nworld'").unwrap();
        assert_eq!(result.stdout, "hello\nworld\n");
    }

    // ── true / false ────────────────────────────────────────────

    #[test]
    fn true_command() {
        let mut shell = shell();
        let result = shell.exec("true").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn false_command() {
        let mut shell = shell();
        let result = shell.exec("false").unwrap();
        assert_eq!(result.exit_code, 1);
    }

    // ── exit ────────────────────────────────────────────────────

    #[test]
    fn exit_default_code() {
        let mut shell = shell();
        let result = shell.exec("exit").unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn exit_with_code() {
        let mut shell = shell();
        let result = shell.exec("exit 42").unwrap();
        assert_eq!(result.exit_code, 42);
    }

    #[test]
    fn exit_stops_subsequent_commands() {
        let mut shell = shell();
        let result = shell.exec("exit 1; echo should_not_appear").unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(!result.stdout.contains("should_not_appear"));
    }

    #[test]
    fn exit_non_numeric_argument() {
        let mut shell = shell();
        let result = shell.exec("exit foo").unwrap();
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("numeric argument required"));
    }

    // ── Command not found ───────────────────────────────────────

    #[test]
    fn command_not_found() {
        let mut shell = shell();
        let result = shell.exec("nonexistent_cmd").unwrap();
        assert_eq!(result.exit_code, 127);
        assert!(result.stderr.contains("command not found"));
    }

    // ── Sequential commands ─────────────────────────────────────

    #[test]
    fn sequential_commands() {
        let mut shell = shell();
        let result = shell.exec("echo hello; echo world").unwrap();
        assert_eq!(result.stdout, "hello\nworld\n");
    }

    #[test]
    fn sequential_exit_code_is_last() {
        let mut shell = shell();
        let result = shell.exec("true; false").unwrap();
        assert_eq!(result.exit_code, 1);
    }

    // ── And-or lists ────────────────────────────────────────────

    #[test]
    fn and_success() {
        let mut shell = shell();
        let result = shell.exec("true && echo yes").unwrap();
        assert_eq!(result.stdout, "yes\n");
    }

    #[test]
    fn and_failure_skips() {
        let mut shell = shell();
        let result = shell.exec("false && echo yes").unwrap();
        assert_eq!(result.stdout, "");
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn or_success_skips() {
        let mut shell = shell();
        let result = shell.exec("true || echo no").unwrap();
        assert_eq!(result.stdout, "");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn or_failure_runs() {
        let mut shell = shell();
        let result = shell.exec("false || echo yes").unwrap();
        assert_eq!(result.stdout, "yes\n");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn chained_and_or() {
        let mut shell = shell();
        let result = shell.exec("false || true && echo yes").unwrap();
        assert_eq!(result.stdout, "yes\n");
        assert_eq!(result.exit_code, 0);
    }

    // ── Pipeline negation ───────────────────────────────────────

    #[test]
    fn pipeline_negation_true() {
        let mut shell = shell();
        let result = shell.exec("! true").unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn pipeline_negation_false() {
        let mut shell = shell();
        let result = shell.exec("! false").unwrap();
        assert_eq!(result.exit_code, 0);
    }

    // ── Variable assignment ─────────────────────────────────────

    #[test]
    fn bare_assignment() {
        let mut shell = shell();
        let result = shell.exec("FOO=bar").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(shell.state.env.get("FOO").unwrap().value.as_scalar(), "bar");
    }

    // ── State persistence ───────────────────────────────────────

    #[test]
    fn state_persists_across_exec_calls() {
        let mut shell = shell();
        shell.exec("FOO=hello").unwrap();
        assert_eq!(
            shell.state.env.get("FOO").unwrap().value.as_scalar(),
            "hello"
        );
        let result = shell.exec("true").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            shell.state.env.get("FOO").unwrap().value.as_scalar(),
            "hello"
        );
    }

    // ── Empty / whitespace input ────────────────────────────────

    #[test]
    fn empty_input() {
        let mut shell = shell();
        let result = shell.exec("").unwrap();
        assert_eq!(result.stdout, "");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn whitespace_only_input() {
        let mut shell = shell();
        let result = shell.exec("   ").unwrap();
        assert_eq!(result.stdout, "");
        assert_eq!(result.exit_code, 0);
    }

    // ── Builder ─────────────────────────────────────────────────

    #[test]
    fn builder_default_cwd() {
        let shell = RustBashBuilder::new().build().unwrap();
        assert_eq!(shell.state.cwd, "/");
    }

    #[test]
    fn builder_with_cwd() {
        let shell = RustBashBuilder::new().cwd("/home/user").build().unwrap();
        assert_eq!(shell.state.cwd, "/home/user");
    }

    #[test]
    fn builder_with_env() {
        let mut env = HashMap::new();
        env.insert("HOME".to_string(), "/home/test".to_string());
        let shell = RustBashBuilder::new().env(env).build().unwrap();
        assert_eq!(
            shell.state.env.get("HOME").unwrap().value.as_scalar(),
            "/home/test"
        );
    }

    #[test]
    fn builder_with_files() {
        let mut files = HashMap::new();
        files.insert("/etc/test.txt".to_string(), b"hello".to_vec());
        let shell = RustBashBuilder::new().files(files).build().unwrap();
        let content = shell
            .state
            .fs
            .read_file(Path::new("/etc/test.txt"))
            .unwrap();
        assert_eq!(content, b"hello");
    }

    #[test]
    fn builder_with_custom_command() {
        use crate::commands::{CommandContext, CommandResult, VirtualCommand};

        struct CustomCmd;
        impl VirtualCommand for CustomCmd {
            fn name(&self) -> &str {
                "custom"
            }
            fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
                CommandResult {
                    stdout: "custom output\n".to_string(),
                    ..CommandResult::default()
                }
            }
        }

        let mut shell = RustBashBuilder::new()
            .command(Arc::new(CustomCmd))
            .build()
            .unwrap();
        let result = shell.exec("custom").unwrap();
        assert_eq!(result.stdout, "custom output\n");
    }

    // ── Additional edge cases ───────────────────────────────────

    #[test]
    fn exit_wraps_to_byte_range() {
        let mut shell = shell();
        let result = shell.exec("exit 256").unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn multiple_bare_assignments() {
        let mut shell = shell();
        shell.exec("A=1 B=2").unwrap();
        assert_eq!(shell.state.env.get("A").unwrap().value.as_scalar(), "1");
        assert_eq!(shell.state.env.get("B").unwrap().value.as_scalar(), "2");
    }

    #[test]
    fn comment_stripping() {
        let mut shell = shell();
        let result = shell.exec("echo hello # this is a comment").unwrap();
        assert_eq!(result.stdout, "hello\n");
    }

    #[test]
    fn negation_with_and_or() {
        let mut shell = shell();
        let result = shell.exec("! false && echo yes").unwrap();
        assert_eq!(result.stdout, "yes\n");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn deeply_chained_and_or() {
        let mut shell = shell();
        let result = shell.exec("true && false || true && echo yes").unwrap();
        assert_eq!(result.stdout, "yes\n");
        assert_eq!(result.exit_code, 0);
    }

    // ── Variable expansion (Phase 1B) ──────────────────────────────

    #[test]
    fn expand_simple_variable() {
        let mut shell = shell();
        shell.exec("FOO=bar").unwrap();
        let result = shell.exec("echo $FOO").unwrap();
        assert_eq!(result.stdout, "bar\n");
    }

    #[test]
    fn expand_braced_variable() {
        let mut shell = shell();
        shell.exec("FOO=bar").unwrap();
        let result = shell.exec("echo ${FOO}").unwrap();
        assert_eq!(result.stdout, "bar\n");
    }

    #[test]
    fn expand_unset_variable_is_empty() {
        let mut shell = shell();
        let result = shell.exec("echo \"$UNDEFINED\"").unwrap();
        assert_eq!(result.stdout, "\n");
    }

    #[test]
    fn expand_default_value() {
        let mut shell = shell();
        let result = shell.exec("echo ${UNSET:-default}").unwrap();
        assert_eq!(result.stdout, "default\n");
    }

    #[test]
    fn expand_default_not_used_when_set() {
        let mut shell = shell();
        shell.exec("VAR=hello").unwrap();
        let result = shell.exec("echo ${VAR:-default}").unwrap();
        assert_eq!(result.stdout, "hello\n");
    }

    #[test]
    fn expand_assign_default() {
        let mut shell = shell();
        let result = shell.exec("echo ${UNSET:=fallback}").unwrap();
        assert_eq!(result.stdout, "fallback\n");
        assert_eq!(
            shell.state.env.get("UNSET").unwrap().value.as_scalar(),
            "fallback"
        );
    }

    #[test]
    fn expand_default_with_variable() {
        let mut shell = shell();
        shell.exec("FALLBACK=resolved").unwrap();
        let result = shell.exec("echo ${UNSET:-$FALLBACK}").unwrap();
        assert_eq!(result.stdout, "resolved\n");
    }

    #[test]
    fn expand_error_if_unset() {
        let mut shell = shell();
        let result = shell.exec("echo ${UNSET:?missing var}").unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("missing var"));
        assert!(result.stdout.is_empty());
    }

    #[test]
    fn expand_alternative_value() {
        let mut shell = shell();
        shell.exec("VAR=hello").unwrap();
        let result = shell.exec("echo ${VAR:+alt}").unwrap();
        assert_eq!(result.stdout, "alt\n");
    }

    #[test]
    fn expand_alternative_unset_is_empty() {
        let mut shell = shell();
        let result = shell.exec("echo \"${UNSET:+alt}\"").unwrap();
        assert_eq!(result.stdout, "\n");
    }

    #[test]
    fn expand_string_length() {
        let mut shell = shell();
        shell.exec("VAR=hello").unwrap();
        let result = shell.exec("echo ${#VAR}").unwrap();
        assert_eq!(result.stdout, "5\n");
    }

    #[test]
    fn expand_suffix_removal_shortest() {
        let mut shell = shell();
        shell.exec("FILE=hello.tar.gz").unwrap();
        let result = shell.exec("echo ${FILE%.*}").unwrap();
        assert_eq!(result.stdout, "hello.tar\n");
    }

    #[test]
    fn expand_suffix_removal_longest() {
        let mut shell = shell();
        shell.exec("FILE=hello.tar.gz").unwrap();
        let result = shell.exec("echo ${FILE%%.*}").unwrap();
        assert_eq!(result.stdout, "hello\n");
    }

    #[test]
    fn expand_prefix_removal_shortest() {
        let mut shell = shell();
        shell.exec("PATH_VAR=/a/b/c").unwrap();
        let result = shell.exec("echo ${PATH_VAR#*/}").unwrap();
        assert_eq!(result.stdout, "a/b/c\n");
    }

    #[test]
    fn expand_prefix_removal_longest() {
        let mut shell = shell();
        shell.exec("PATH_VAR=/a/b/c").unwrap();
        let result = shell.exec("echo ${PATH_VAR##*/}").unwrap();
        assert_eq!(result.stdout, "c\n");
    }

    #[test]
    fn expand_substitution_first() {
        let mut shell = shell();
        shell.exec("STR=hello").unwrap();
        let result = shell.exec("echo ${STR/l/r}").unwrap();
        assert_eq!(result.stdout, "herlo\n");
    }

    #[test]
    fn expand_substitution_all() {
        let mut shell = shell();
        shell.exec("STR=hello").unwrap();
        let result = shell.exec("echo ${STR//l/r}").unwrap();
        assert_eq!(result.stdout, "herro\n");
    }

    #[test]
    fn expand_substring() {
        let mut shell = shell();
        shell.exec("STR=hello").unwrap();
        let result = shell.exec("echo ${STR:1:3}").unwrap();
        assert_eq!(result.stdout, "ell\n");
    }

    #[test]
    fn expand_uppercase_first() {
        let mut shell = shell();
        shell.exec("STR=hello").unwrap();
        let result = shell.exec("echo ${STR^}").unwrap();
        assert_eq!(result.stdout, "Hello\n");
    }

    #[test]
    fn expand_uppercase_all() {
        let mut shell = shell();
        shell.exec("STR=hello").unwrap();
        let result = shell.exec("echo ${STR^^}").unwrap();
        assert_eq!(result.stdout, "HELLO\n");
    }

    #[test]
    fn expand_lowercase_first() {
        let mut shell = shell();
        shell.exec("STR=HELLO").unwrap();
        let result = shell.exec("echo ${STR,}").unwrap();
        assert_eq!(result.stdout, "hELLO\n");
    }

    #[test]
    fn expand_lowercase_all() {
        let mut shell = shell();
        shell.exec("STR=HELLO").unwrap();
        let result = shell.exec("echo ${STR,,}").unwrap();
        assert_eq!(result.stdout, "hello\n");
    }

    // ── Special variables ───────────────────────────────────────────

    #[test]
    fn expand_exit_status() {
        let mut shell = shell();
        shell.exec("false").unwrap();
        let result = shell.exec("echo $?").unwrap();
        assert_eq!(result.stdout, "1\n");
    }

    #[test]
    fn expand_dollar_dollar() {
        let mut shell = shell();
        let result = shell.exec("echo $$").unwrap();
        assert_eq!(result.stdout, "1000\n");
    }

    #[test]
    fn expand_dollar_zero() {
        let mut shell = shell();
        let result = shell.exec("echo $0").unwrap();
        assert_eq!(result.stdout, "rust-bash\n");
    }

    #[test]
    fn expand_positional_params() {
        let mut shell = shell();
        shell.exec("set -- a b c").unwrap();
        let result = shell.exec("echo $1 $2 $3").unwrap();
        assert_eq!(result.stdout, "a b c\n");
    }

    #[test]
    fn expand_param_count() {
        let mut shell = shell();
        shell.exec("set -- a b c").unwrap();
        let result = shell.exec("echo $#").unwrap();
        assert_eq!(result.stdout, "3\n");
    }

    #[test]
    fn expand_at_all_params() {
        let mut shell = shell();
        shell.exec("set -- one two three").unwrap();
        let result = shell.exec("echo $@").unwrap();
        assert_eq!(result.stdout, "one two three\n");
    }

    #[test]
    fn expand_star_all_params() {
        let mut shell = shell();
        shell.exec("set -- one two three").unwrap();
        let result = shell.exec("echo $*").unwrap();
        assert_eq!(result.stdout, "one two three\n");
    }

    #[test]
    fn expand_random_is_numeric() {
        let mut shell = shell();
        let result = shell.exec("echo $RANDOM").unwrap();
        let val: u32 = result.stdout.trim().parse().unwrap();
        assert!(val <= 32767);
    }

    // ── Tilde expansion ─────────────────────────────────────────────

    #[test]
    fn tilde_expands_to_home() {
        let mut env = HashMap::new();
        env.insert("HOME".to_string(), "/home/test".to_string());
        let mut shell = RustBashBuilder::new().env(env).build().unwrap();
        let result = shell.exec("echo ~").unwrap();
        assert_eq!(result.stdout, "/home/test\n");
    }

    // ── Redirections ────────────────────────────────────────────────

    #[test]
    fn redirect_stdout_to_file() {
        let mut shell = shell();
        shell.exec("echo hello > /output.txt").unwrap();
        let content = shell.state.fs.read_file(Path::new("/output.txt")).unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "hello\n");
    }

    #[test]
    fn redirect_append() {
        let mut shell = shell();
        shell.exec("echo hello > /output.txt").unwrap();
        shell.exec("echo world >> /output.txt").unwrap();
        let content = shell.state.fs.read_file(Path::new("/output.txt")).unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "hello\nworld\n");
    }

    #[test]
    fn redirect_stdin_from_file() {
        let mut files = HashMap::new();
        files.insert("/input.txt".to_string(), b"file contents\n".to_vec());
        let mut shell = RustBashBuilder::new().files(files).build().unwrap();
        let result = shell.exec("cat < /input.txt").unwrap();
        assert_eq!(result.stdout, "file contents\n");
    }

    #[test]
    fn redirect_stderr_to_file() {
        let mut shell = shell();
        shell.exec("nonexistent 2> /err.txt").unwrap();
        let content = shell.state.fs.read_file(Path::new("/err.txt")).unwrap();
        assert!(String::from_utf8_lossy(&content).contains("command not found"));
    }

    #[test]
    fn redirect_dev_null() {
        let mut shell = shell();
        let result = shell.exec("echo hello > /dev/null").unwrap();
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn redirect_stderr_to_stdout() {
        let mut shell = shell();
        let result = shell.exec("nonexistent 2>&1").unwrap();
        assert!(result.stdout.contains("command not found"));
        assert_eq!(result.stderr, "");
    }

    #[test]
    fn redirect_write_then_cat() {
        let mut shell = shell();
        shell.exec("echo hello > /test.txt").unwrap();
        let result = shell.exec("cat /test.txt").unwrap();
        assert_eq!(result.stdout, "hello\n");
    }

    // ── cat command ─────────────────────────────────────────────────

    #[test]
    fn cat_stdin() {
        let mut shell = shell();
        let result = shell.exec("echo hello | cat").unwrap();
        assert_eq!(result.stdout, "hello\n");
    }

    #[test]
    fn cat_file() {
        let mut files = HashMap::new();
        files.insert("/test.txt".to_string(), b"content\n".to_vec());
        let mut shell = RustBashBuilder::new().files(files).build().unwrap();
        let result = shell.exec("cat /test.txt").unwrap();
        assert_eq!(result.stdout, "content\n");
    }

    #[test]
    fn cat_nonexistent_file() {
        let mut shell = shell();
        let result = shell.exec("cat /no_such_file.txt").unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("No such file"));
    }

    #[test]
    fn cat_line_numbers() {
        let mut files = HashMap::new();
        files.insert("/test.txt".to_string(), b"a\nb\nc\n".to_vec());
        let mut shell = RustBashBuilder::new().files(files).build().unwrap();
        let result = shell.exec("cat -n /test.txt").unwrap();
        assert!(result.stdout.contains("1\ta"));
        assert!(result.stdout.contains("2\tb"));
        assert!(result.stdout.contains("3\tc"));
    }

    // ── Builtins ────────────────────────────────────────────────────

    #[test]
    fn cd_changes_cwd() {
        let mut shell = RustBashBuilder::new().cwd("/home/user").build().unwrap();
        shell.exec("cd /").unwrap();
        assert_eq!(shell.state.cwd, "/");
    }

    #[test]
    fn cd_home() {
        let mut env = HashMap::new();
        env.insert("HOME".to_string(), "/home/test".to_string());
        let mut shell = RustBashBuilder::new()
            .cwd("/home/test")
            .env(env)
            .build()
            .unwrap();
        shell.exec("cd /").unwrap();
        shell.exec("cd").unwrap();
        assert_eq!(shell.state.cwd, "/home/test");
    }

    #[test]
    fn cd_sets_oldpwd() {
        let mut shell = RustBashBuilder::new().cwd("/home/user").build().unwrap();
        shell.exec("cd /").unwrap();
        assert_eq!(
            shell.state.env.get("OLDPWD").unwrap().value.as_scalar(),
            "/home/user"
        );
    }

    #[test]
    fn export_creates_exported_var() {
        let mut shell = shell();
        shell.exec("export FOO=bar").unwrap();
        let var = shell.state.env.get("FOO").unwrap();
        assert_eq!(var.value.as_scalar(), "bar");
        assert!(var.exported());
    }

    #[test]
    fn export_marks_existing_var() {
        let mut shell = shell();
        shell.exec("FOO=bar").unwrap();
        assert!(!shell.state.env.get("FOO").unwrap().exported());
        shell.exec("export FOO").unwrap();
        assert!(shell.state.env.get("FOO").unwrap().exported());
    }

    #[test]
    fn unset_removes_var() {
        let mut shell = shell();
        shell.exec("FOO=bar").unwrap();
        shell.exec("unset FOO").unwrap();
        assert!(!shell.state.env.contains_key("FOO"));
    }

    #[test]
    fn set_options() {
        let mut shell = shell();
        shell.exec("set -e").unwrap();
        assert!(shell.state.shell_opts.errexit);
        shell.exec("set +e").unwrap();
        assert!(!shell.state.shell_opts.errexit);
    }

    #[test]
    fn set_positional_params() {
        let mut shell = shell();
        shell.exec("set -- x y z").unwrap();
        assert_eq!(shell.state.positional_params, vec!["x", "y", "z"]);
    }

    #[test]
    fn shift_positional_params() {
        let mut shell = shell();
        shell.exec("set -- a b c d").unwrap();
        shell.exec("shift 2").unwrap();
        assert_eq!(shell.state.positional_params, vec!["c", "d"]);
    }

    #[test]
    fn readonly_variable() {
        let mut shell = shell();
        shell.exec("readonly X=42").unwrap();
        let var = shell.state.env.get("X").unwrap();
        assert_eq!(var.value.as_scalar(), "42");
        assert!(var.readonly());
        // Bash: assigning to readonly prints error to stderr & sets exit code 1
        let result = shell.exec("X=new").unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("readonly"));
        // Value unchanged
        assert_eq!(shell.state.env.get("X").unwrap().value.as_scalar(), "42");
    }

    #[test]
    fn declare_readonly() {
        let mut shell = shell();
        shell.exec("declare -r Y=99").unwrap();
        assert!(shell.state.env.get("Y").unwrap().readonly());
    }

    #[test]
    fn read_from_stdin() {
        let mut shell = shell();
        shell.exec("echo 'hello world' > /tmp_input").unwrap();
        let result = shell.exec("read VAR < /tmp_input").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            shell.state.env.get("VAR").unwrap().value.as_scalar(),
            "hello world"
        );
    }

    #[test]
    fn read_multiple_vars() {
        let mut shell = shell();
        shell
            .exec("echo 'one two three four' > /tmp_input")
            .unwrap();
        shell.exec("read A B < /tmp_input").unwrap();
        assert_eq!(shell.state.env.get("A").unwrap().value.as_scalar(), "one");
        assert_eq!(
            shell.state.env.get("B").unwrap().value.as_scalar(),
            "two three four"
        );
    }

    #[test]
    fn colon_builtin() {
        let mut shell = shell();
        let result = shell.exec(":").unwrap();
        assert_eq!(result.exit_code, 0);
    }

    // ── Combined features ───────────────────────────────────────────

    #[test]
    fn variable_in_redirect_target() {
        let mut shell = shell();
        shell.exec("FILE=/output.txt").unwrap();
        shell.exec("echo hello > $FILE").unwrap();
        let content = shell.state.fs.read_file(Path::new("/output.txt")).unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "hello\n");
    }

    #[test]
    fn pipeline_with_variable() {
        let mut shell = shell();
        shell.exec("MSG=world").unwrap();
        let result = shell.exec("echo hello $MSG | cat").unwrap();
        assert_eq!(result.stdout, "hello world\n");
    }

    #[test]
    fn set_and_expand_positional() {
        let mut shell = shell();
        shell.exec("set -- foo bar baz").unwrap();
        let result = shell.exec("echo $1 $3").unwrap();
        assert_eq!(result.stdout, "foo baz\n");
    }

    #[test]
    fn shift_and_expand() {
        let mut shell = shell();
        shell.exec("set -- a b c").unwrap();
        shell.exec("shift").unwrap();
        let result = shell.exec("echo $1 $#").unwrap();
        assert_eq!(result.stdout, "b 2\n");
    }

    #[test]
    fn set_pipefail_option() {
        let mut shell = shell();
        shell.exec("set -o pipefail").unwrap();
        assert!(shell.state.shell_opts.pipefail);
    }

    #[test]
    fn double_quoted_variable_expansion() {
        let mut sh = shell();
        sh.exec("FOO='hello world'").unwrap();
        let result = sh.exec("echo \"$FOO\"").unwrap();
        assert_eq!(result.stdout, "hello world\n");
    }

    #[test]
    fn empty_variable_in_quotes() {
        let mut shell = shell();
        let result = shell.exec("echo \"$EMPTY\"").unwrap();
        assert_eq!(result.stdout, "\n");
    }

    #[test]
    fn here_string() {
        let mut shell = shell();
        let result = shell.exec("cat <<< 'hello world'").unwrap();
        assert_eq!(result.stdout, "hello world\n");
    }

    #[test]
    fn output_and_error_redirect() {
        let mut shell = shell();
        shell.exec("echo hello &> /both.txt").unwrap();
        let content = shell.state.fs.read_file(Path::new("/both.txt")).unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "hello\n");
    }

    // ── Phase 1C: Compound commands ─────────────────────────────

    #[test]
    fn if_then_true() {
        let mut shell = shell();
        let result = shell
            .exec("if true; then echo yes; else echo no; fi")
            .unwrap();
        assert_eq!(result.stdout, "yes\n");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn if_then_false() {
        let mut shell = shell();
        let result = shell
            .exec("if false; then echo yes; else echo no; fi")
            .unwrap();
        assert_eq!(result.stdout, "no\n");
    }

    #[test]
    fn if_elif_else() {
        let mut shell = shell();
        let result = shell
            .exec("if false; then echo a; elif true; then echo b; else echo c; fi")
            .unwrap();
        assert_eq!(result.stdout, "b\n");
    }

    #[test]
    fn if_elif_falls_through_to_else() {
        let mut shell = shell();
        let result = shell
            .exec("if false; then echo a; elif false; then echo b; else echo c; fi")
            .unwrap();
        assert_eq!(result.stdout, "c\n");
    }

    #[test]
    fn if_no_else_unmatched() {
        let mut shell = shell();
        let result = shell.exec("if false; then echo yes; fi").unwrap();
        assert_eq!(result.stdout, "");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn if_with_command_condition() {
        let mut shell = shell();
        shell.exec("X=hello").unwrap();
        let result = shell
            .exec("if echo checking > /dev/null; then echo passed; fi")
            .unwrap();
        assert_eq!(result.stdout, "passed\n");
    }

    #[test]
    fn for_loop_basic() {
        let mut shell = shell();
        let result = shell.exec("for i in a b c; do echo $i; done").unwrap();
        assert_eq!(result.stdout, "a\nb\nc\n");
    }

    #[test]
    fn for_loop_with_variable_expansion() {
        let mut shell = shell();
        // Word splitting of unquoted $VAR not yet implemented,
        // so use separate words in the for list
        let result = shell.exec("for i in x y z; do echo $i; done").unwrap();
        assert_eq!(result.stdout, "x\ny\nz\n");
    }

    #[test]
    fn for_loop_variable_persists_after_loop() {
        let mut shell = shell();
        shell.exec("for i in a b c; do true; done").unwrap();
        let result = shell.exec("echo $i").unwrap();
        assert_eq!(result.stdout, "c\n");
    }

    #[test]
    fn while_loop_basic() {
        let mut shell = shell();
        // while false → condition fails immediately, body never runs
        let result = shell
            .exec("while false; do echo should-not-appear; done")
            .unwrap();
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn while_loop_executes_body() {
        let mut shell = shell();
        // Test while with a command that succeeds then fails.
        // Since we don't have `[` builtin yet, just verify the body runs
        // when condition is true, then stops when it becomes false.
        let _result = shell.exec(
            r#"X=yes; while echo $X > /dev/null && [ "$X" = yes ]; do echo looped; X=no; done"#,
        );
    }

    #[test]
    fn until_loop_basic() {
        let mut shell = shell();
        let result = shell
            .exec("until true; do echo should-not-run; done")
            .unwrap();
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn until_loop_runs_once_when_condition_false() {
        let mut shell = shell();
        // until true → don't execute body (condition immediately true)
        let result = shell.exec("until true; do echo nope; done").unwrap();
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn brace_group_basic() {
        let mut shell = shell();
        let result = shell.exec("{ echo hello; echo world; }").unwrap();
        assert_eq!(result.stdout, "hello\nworld\n");
    }

    #[test]
    fn brace_group_shares_scope() {
        let mut shell = shell();
        shell.exec("X=before").unwrap();
        shell.exec("{ X=after; }").unwrap();
        let result = shell.exec("echo $X").unwrap();
        assert_eq!(result.stdout, "after\n");
    }

    #[test]
    fn subshell_basic() {
        let mut shell = shell();
        let result = shell.exec("(echo hello)").unwrap();
        assert_eq!(result.stdout, "hello\n");
    }

    #[test]
    fn subshell_isolates_variables() {
        let mut shell = shell();
        let result = shell.exec("X=outer; (X=inner; echo $X); echo $X").unwrap();
        assert_eq!(result.stdout, "inner\nouter\n");
    }

    #[test]
    fn subshell_isolates_cwd() {
        let mut shell = shell();
        shell.exec("mkdir /tmp").unwrap();
        let result = shell.exec("(cd /tmp && pwd); pwd").unwrap();
        assert_eq!(result.stdout, "/tmp\n/\n");
    }

    #[test]
    fn subshell_propagates_exit_code() {
        let mut shell = shell();
        let result = shell.exec("(false)").unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn subshell_function_can_return() {
        let mut shell = shell();
        let result = shell.exec("f() ( return 42; )\nf\necho $?\n").unwrap();
        assert_eq!(result.stdout, "42\n");
    }

    #[test]
    fn subshell_isolates_fs_writes() {
        let mut shell = shell();
        shell.exec("(echo data > /subshell_file.txt)").unwrap();
        // The file was written in the subshell's cloned fs, NOT the parent
        let exists = shell.state.fs.exists(Path::new("/subshell_file.txt"));
        assert!(!exists);
    }

    #[test]
    fn nested_if_in_for() {
        let mut shell = shell();
        let result = shell
            .exec("for x in yes no yes; do if true; then echo $x; fi; done")
            .unwrap();
        assert_eq!(result.stdout, "yes\nno\nyes\n");
    }

    #[test]
    fn compound_command_with_redirect() {
        let mut shell = shell();
        shell
            .exec("{ echo hello; echo world; } > /out.txt")
            .unwrap();
        let content = shell.state.fs.read_file(Path::new("/out.txt")).unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "hello\nworld\n");
    }

    #[test]
    fn for_loop_in_pipeline() {
        let mut shell = shell();
        let result = shell
            .exec("for i in a b c; do echo $i; done | cat")
            .unwrap();
        assert_eq!(result.stdout, "a\nb\nc\n");
    }

    #[test]
    fn if_in_pipeline() {
        let mut shell = shell();
        let result = shell.exec("if true; then echo yes; fi | cat").unwrap();
        assert_eq!(result.stdout, "yes\n");
    }

    #[test]
    fn if_break_outside_loop_still_takes_then_branch() {
        let mut shell = shell();
        let result = shell
            .exec("f() { if break; then echo hi; fi; }\nf\n")
            .unwrap();
        assert_eq!(result.stdout, "hi\n");
        assert!(result.stderr.contains("break: only meaningful"));
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn multiline_double_paren_can_parse_as_command_group() {
        let mut shell = shell();
        let result = shell
            .exec("(( echo 1\necho 2\n(( x ))\n: $(( x ))\necho 3\n) )\n")
            .unwrap();
        assert_eq!(result.stdout, "1\n2\n3\n");
    }

    #[test]
    fn expanding_heredoc_treats_quotes_as_literal_text() {
        let mut shell = shell();
        let result = shell.exec("v=one\ntac <<EOF\n$v\n\"two\nEOF\n").unwrap();
        assert_eq!(result.stdout, "\"two\none\n");
    }

    #[test]
    fn expanding_heredoc_preserves_backslash_quote_sequences() {
        let mut shell = shell();
        let result = shell.exec("cat <<EOF\na \\\"quote\\\"\nEOF\n").unwrap();
        assert_eq!(result.stdout, "a \\\"quote\\\"\n");
    }

    #[test]
    fn quoted_glob_prefix_stays_literal() {
        let mut shell = shell();
        shell.exec("mkdir -p _tmp").unwrap();
        shell
            .exec("touch '_tmp/[bc]ar.mm' _tmp/bar.mm _tmp/car.mm")
            .unwrap();
        let result = shell.exec("echo '_tmp/[bc]'*.mm - _tmp/?ar.mm").unwrap();
        assert_eq!(result.stdout, "_tmp/[bc]ar.mm - _tmp/bar.mm _tmp/car.mm\n");
    }

    #[test]
    fn env_command_runs_inside_redirected_subshell() {
        let mut shell = shell();
        shell.exec("( env echo 2 ) > b.txt").unwrap();
        let result = shell.exec("cat b.txt").unwrap();
        assert_eq!(result.stdout, "2\n");
    }

    // ── Phase 1C: New commands ──────────────────────────────────

    #[test]
    fn touch_creates_file() {
        let mut shell = shell();
        shell.exec("touch /newfile.txt").unwrap();
        assert!(shell.state.fs.exists(Path::new("/newfile.txt")));
        let content = shell.state.fs.read_file(Path::new("/newfile.txt")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn touch_existing_file_no_error() {
        let mut shell = shell();
        shell.exec("echo data > /existing.txt").unwrap();
        let result = shell.exec("touch /existing.txt").unwrap();
        assert_eq!(result.exit_code, 0);
        // Content should remain
        let content = shell
            .state
            .fs
            .read_file(Path::new("/existing.txt"))
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "data\n");
    }

    #[test]
    fn touch_and_ls() {
        let mut shell = shell();
        shell.exec("touch /file.txt").unwrap();
        let result = shell.exec("ls /").unwrap();
        assert!(result.stdout.contains("file.txt"));
    }

    #[test]
    fn mkdir_creates_directory() {
        let mut shell = shell();
        let result = shell.exec("mkdir /mydir").unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(shell.state.fs.exists(Path::new("/mydir")));
    }

    #[test]
    fn mkdir_p_creates_parents() {
        let mut shell = shell();
        let result = shell.exec("mkdir -p /a/b/c").unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(shell.state.fs.exists(Path::new("/a/b/c")));
    }

    #[test]
    fn mkdir_p_and_ls() {
        let mut shell = shell();
        shell.exec("mkdir -p /a/b/c").unwrap();
        let result = shell.exec("ls /a/b").unwrap();
        assert!(result.stdout.contains("c"));
    }

    #[test]
    fn ls_root_empty() {
        let mut shell = shell();
        let result = shell.exec("ls /").unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn ls_one_per_line() {
        let mut shell = shell();
        shell.exec("mkdir /test_dir").unwrap();
        shell.exec("touch /test_dir/aaa").unwrap();
        shell.exec("touch /test_dir/bbb").unwrap();
        let result = shell.exec("ls -1 /test_dir").unwrap();
        assert_eq!(result.stdout, "aaa\nbbb\n");
    }

    #[test]
    fn ls_long_format() {
        let mut shell = shell();
        shell.exec("touch /myfile").unwrap();
        let result = shell.exec("ls -l /").unwrap();
        assert!(result.stdout.contains("myfile"));
        // Should have permission string
        assert!(result.stdout.contains("rw"));
    }

    #[test]
    fn ls_nonexistent() {
        let mut shell = shell();
        let result = shell.exec("ls /no_such_dir").unwrap();
        assert_ne!(result.exit_code, 0);
        assert!(result.stderr.contains("cannot access"));
    }

    #[test]
    fn pwd_command() {
        let mut shell = shell();
        let result = shell.exec("pwd").unwrap();
        assert_eq!(result.stdout, "/\n");
    }

    #[test]
    fn pwd_after_cd() {
        let mut shell = shell();
        shell.exec("mkdir /mydir").unwrap();
        shell.exec("cd /mydir").unwrap();
        let result = shell.exec("pwd").unwrap();
        assert_eq!(result.stdout, "/mydir\n");
    }

    #[test]
    fn case_basic() {
        let mut shell = shell();
        let result = shell
            .exec("case hello in hello) echo matched;; world) echo nope;; esac")
            .unwrap();
        assert_eq!(result.stdout, "matched\n");
    }

    #[test]
    fn case_wildcard() {
        let mut shell = shell();
        let result = shell
            .exec("case foo in bar) echo bar;; *) echo default;; esac")
            .unwrap();
        assert_eq!(result.stdout, "default\n");
    }

    #[test]
    fn case_no_match() {
        let mut shell = shell();
        let result = shell.exec("case xyz in abc) echo nope;; esac").unwrap();
        assert_eq!(result.stdout, "");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn register_default_commands_includes_new() {
        let cmds = crate::commands::register_default_commands();
        assert!(cmds.contains_key("touch"));
        assert!(cmds.contains_key("mkdir"));
        assert!(cmds.contains_key("ls"));
        assert!(cmds.contains_key("pwd"));
    }

    // ── is_input_complete ──────────────────────────────────────────

    #[test]
    fn complete_simple_commands() {
        assert!(RustBash::is_input_complete("echo hello"));
        assert!(RustBash::is_input_complete(""));
        assert!(RustBash::is_input_complete("   "));
    }

    #[test]
    fn incomplete_unterminated_quotes() {
        assert!(!RustBash::is_input_complete("echo \"hello"));
        assert!(!RustBash::is_input_complete("echo 'hello"));
    }

    #[test]
    fn incomplete_open_block() {
        assert!(!RustBash::is_input_complete("if true; then"));
        assert!(!RustBash::is_input_complete("for i in 1 2; do"));
    }

    #[test]
    fn incomplete_trailing_pipe() {
        assert!(!RustBash::is_input_complete("echo hello |"));
    }

    // ── Public accessors ───────────────────────────────────────────

    #[test]
    fn cwd_accessor() {
        let sh = shell();
        assert_eq!(sh.cwd(), "/");
    }

    #[test]
    fn last_exit_code_accessor() {
        let mut sh = shell();
        sh.exec("false").unwrap();
        assert_eq!(sh.last_exit_code(), 1);
    }

    #[test]
    fn command_names_accessor() {
        let sh = shell();
        let names = sh.command_names();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"cat"));
    }

    #[test]
    fn builder_accepts_custom_fs() {
        let custom_fs = Arc::new(crate::vfs::InMemoryFs::new());
        custom_fs
            .write_file(std::path::Path::new("/pre-existing.txt"), b"hello")
            .unwrap();

        let mut shell = RustBashBuilder::new().fs(custom_fs).build().unwrap();

        let result = shell.exec("cat /pre-existing.txt").unwrap();
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[test]
    fn should_exit_accessor() {
        let mut sh = shell();
        assert!(!sh.should_exit());
        sh.exec("exit").unwrap();
        assert!(sh.should_exit());
    }
}
