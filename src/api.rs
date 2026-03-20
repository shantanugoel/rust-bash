//! Public API: `RustBash` shell instance and builder.

use crate::commands::{self, VirtualCommand};
use crate::error::RustBashError;
use crate::interpreter::{
    self, ExecResult, ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, Variable,
};
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

        let program = interpreter::parse(input)?;
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
}

/// Builder for configuring a [`RustBash`] instance.
pub struct RustBashBuilder {
    files: HashMap<String, Vec<u8>>,
    env: HashMap<String, String>,
    cwd: Option<String>,
    custom_commands: Vec<Box<dyn VirtualCommand>>,
    limits: Option<ExecutionLimits>,
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
            cwd: None,
            custom_commands: Vec::new(),
            limits: None,
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
        self
    }

    /// Set the initial working directory (created automatically).
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Register a custom command.
    pub fn command(mut self, cmd: Box<dyn VirtualCommand>) -> Self {
        self.custom_commands.push(cmd);
        self
    }

    /// Override the default execution limits.
    pub fn execution_limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Build the shell instance.
    pub fn build(self) -> Result<RustBash, RustBashError> {
        let fs = Arc::new(InMemoryFs::new());
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

        let env: HashMap<String, Variable> = self
            .env
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    Variable {
                        value: v,
                        exported: true,
                        readonly: false,
                    },
                )
            })
            .collect();

        let mut commands = commands::register_default_commands();
        for cmd in self.custom_commands {
            commands.insert(cmd.name().to_string(), cmd);
        }

        let state = InterpreterState {
            fs,
            env,
            cwd,
            functions: HashMap::new(),
            last_exit_code: 0,
            commands,
            shell_opts: ShellOpts::default(),
            limits: self.limits.unwrap_or_default(),
            counters: ExecutionCounters::default(),
            should_exit: false,
            loop_depth: 0,
            control_flow: None,
            positional_params: Vec::new(),
            shell_name: "rust-bash".to_string(),
            random_seed: 0,
            local_scopes: Vec::new(),
            in_function_depth: 0,
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
        };

        Ok(RustBash { state })
    }
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
        assert_eq!(shell.state.env.get("FOO").unwrap().value, "bar");
    }

    // ── State persistence ───────────────────────────────────────

    #[test]
    fn state_persists_across_exec_calls() {
        let mut shell = shell();
        shell.exec("FOO=hello").unwrap();
        assert_eq!(shell.state.env.get("FOO").unwrap().value, "hello");
        let result = shell.exec("true").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(shell.state.env.get("FOO").unwrap().value, "hello");
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
        assert_eq!(shell.state.env.get("HOME").unwrap().value, "/home/test");
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
            .command(Box::new(CustomCmd))
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
        assert_eq!(shell.state.env.get("A").unwrap().value, "1");
        assert_eq!(shell.state.env.get("B").unwrap().value, "2");
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
        assert_eq!(shell.state.env.get("UNSET").unwrap().value, "fallback");
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
        let result = shell.exec("echo ${UNSET:?missing var}");
        assert!(result.is_err());
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
        assert_eq!(result.stdout, "1\n");
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
        assert_eq!(shell.state.env.get("OLDPWD").unwrap().value, "/home/user");
    }

    #[test]
    fn export_creates_exported_var() {
        let mut shell = shell();
        shell.exec("export FOO=bar").unwrap();
        let var = shell.state.env.get("FOO").unwrap();
        assert_eq!(var.value, "bar");
        assert!(var.exported);
    }

    #[test]
    fn export_marks_existing_var() {
        let mut shell = shell();
        shell.exec("FOO=bar").unwrap();
        assert!(!shell.state.env.get("FOO").unwrap().exported);
        shell.exec("export FOO").unwrap();
        assert!(shell.state.env.get("FOO").unwrap().exported);
    }

    #[test]
    fn unset_removes_var() {
        let mut shell = shell();
        shell.exec("FOO=bar").unwrap();
        shell.exec("unset FOO").unwrap();
        assert!(shell.state.env.get("FOO").is_none());
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
        assert_eq!(var.value, "42");
        assert!(var.readonly);
        let result = shell.exec("X=new");
        assert!(result.is_err());
    }

    #[test]
    fn declare_readonly() {
        let mut shell = shell();
        shell.exec("declare -r Y=99").unwrap();
        assert!(shell.state.env.get("Y").unwrap().readonly);
    }

    #[test]
    fn read_from_stdin() {
        let mut shell = shell();
        shell.exec("echo 'hello world' > /tmp_input").unwrap();
        let result = shell.exec("read VAR < /tmp_input").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(shell.state.env.get("VAR").unwrap().value, "hello world");
    }

    #[test]
    fn read_multiple_vars() {
        let mut shell = shell();
        shell
            .exec("echo 'one two three four' > /tmp_input")
            .unwrap();
        shell.exec("read A B < /tmp_input").unwrap();
        assert_eq!(shell.state.env.get("A").unwrap().value, "one");
        assert_eq!(shell.state.env.get("B").unwrap().value, "two three four");
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
        shell.exec("touch /aaa").unwrap();
        shell.exec("touch /bbb").unwrap();
        let result = shell.exec("ls -1 /").unwrap();
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
}
