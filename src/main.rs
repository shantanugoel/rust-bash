use std::borrow::Cow;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use rust_bash::{ExecResult, RustBash, RustBashBuilder};
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{CompletionType, Config, Context, Editor, Helper};
use serde_json::json;

/// A sandboxed bash interpreter with a virtual filesystem
#[derive(Parser)]
#[command(name = "rust-bash", version)]
struct Cli {
    /// Execute a command string and exit
    #[arg(short = 'c')]
    command: Option<String>,

    /// Seed VFS from host files/directories (HOST:VFS or HOST_DIR)
    #[arg(long = "files", value_name = "MAPPING")]
    file_mappings: Vec<String>,

    /// Set initial working directory
    #[arg(long, value_name = "DIR")]
    cwd: Option<String>,

    /// Set an environment variable (KEY=VALUE, repeatable)
    #[arg(long, value_name = "KEY=VALUE")]
    env: Vec<String>,

    /// Output results as JSON: {"stdout":"...","stderr":"...","exit_code":N}
    #[arg(long)]
    json: bool,

    /// Start an MCP (Model Context Protocol) server over stdio
    #[arg(long)]
    mcp: bool,

    /// Script file to execute, followed by optional positional arguments
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

// ── Readline helper (completion + validation) ───────────────────────

struct ShellHelper {
    commands: Vec<String>,
    last_exit: i32,
}

impl Completer for ShellHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        let prefix = &line[..pos];
        let start = prefix
            .rfind(|c: char| c.is_whitespace())
            .map_or(0, |i| i + 1);
        // Only complete the first token (command name).
        if start != 0 {
            return Ok((pos, vec![]));
        }
        let word = &prefix[start..];
        let matches: Vec<String> = self
            .commands
            .iter()
            .filter(|c| c.starts_with(word))
            .cloned()
            .collect();
        Ok((start, matches))
    }
}

impl Validator for ShellHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        let input = ctx.input();
        if input.is_empty() {
            return Ok(ValidationResult::Valid(None));
        }
        if RustBash::is_input_complete(input) {
            Ok(ValidationResult::Valid(None))
        } else {
            Ok(ValidationResult::Incomplete)
        }
    }
}

impl Hinter for ShellHelper {
    type Hint = String;
}

impl Highlighter for ShellHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        let color = if self.last_exit == 0 {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };
        Cow::Owned(format!("{color}{prompt}\x1b[0m"))
    }
}

impl Helper for ShellHelper {}

// ── Prompt and history ──────────────────────────────────────────────

fn make_prompt(cwd: &str) -> String {
    format!("rust-bash:{cwd}$ ")
}

fn history_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".rust_bash_history"))
}

/// Execute a command string and produce output according to the `--json` flag.
fn execute_and_output(shell: &mut RustBash, source: &str, json_mode: bool) -> ExitCode {
    match shell.exec(source) {
        Ok(result) => output_result(&result, json_mode),
        Err(e) => {
            eprintln!("rust-bash: {e}");
            ExitCode::from(2)
        }
    }
}

/// Format an `ExecResult` as JSON or plain text, returning the appropriate exit code.
fn output_result(result: &ExecResult, json_mode: bool) -> ExitCode {
    if json_mode {
        let obj = json!({
            "stdout": result.stdout,
            "stderr": result.stderr,
            "exit_code": result.exit_code,
        });
        println!("{obj}");
    } else {
        if !result.stdout.is_empty() {
            print!("{}", result.stdout);
        }
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }
    }
    ExitCode::from((result.exit_code & 0xFF) as u8)
}

/// Recursively load all files from a host directory into a map keyed by VFS paths.
fn load_host_dir(dir: &Path, prefix: &str) -> HashMap<String, Vec<u8>> {
    let mut files = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
            if path.is_file() {
                if let Ok(data) = std::fs::read(&path) {
                    files.insert(name, data);
                }
            } else if path.is_dir() {
                files.extend(load_host_dir(&path, &name));
            }
        }
    }
    files
}

/// Parse `--files` mappings into a VFS file map.
fn parse_file_mappings(mappings: &[String]) -> Result<HashMap<String, Vec<u8>>, (String, u8)> {
    let mut files = HashMap::new();
    for mapping in mappings {
        if let Some((host_path, vfs_path)) = mapping.split_once(':') {
            let vfs_path = vfs_path.trim_end_matches('/');
            let vfs_path = if vfs_path.is_empty() { "/" } else { vfs_path };
            let path = Path::new(host_path);
            if !path.exists() {
                return Err((format!("rust-bash: path not found: {host_path}"), 2));
            }
            if path.is_file() {
                let data = std::fs::read(path)
                    .map_err(|e| (format!("rust-bash: error reading {host_path}: {e}"), 2))?;
                files.insert(vfs_path.to_string(), data);
            } else if path.is_dir() {
                files.extend(load_host_dir(path, vfs_path));
            } else {
                return Err((
                    format!("rust-bash: not a file or directory: {host_path}"),
                    2,
                ));
            }
        } else {
            let path = Path::new(mapping.as_str());
            if !path.exists() {
                return Err((format!("rust-bash: path not found: {mapping}"), 2));
            }
            if !path.is_dir() {
                return Err((format!("rust-bash: not a file or directory: {mapping}"), 2));
            }
            files.extend(load_host_dir(path, ""));
        }
    }
    Ok(files)
}

/// Parse `--env` values and merge with defaults.
fn parse_env(env_args: &[String], cwd: &str) -> Result<HashMap<String, String>, (String, u8)> {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), "/home".to_string());
    env.insert("USER".to_string(), "user".to_string());
    env.insert("PWD".to_string(), cwd.to_string());

    for val in env_args {
        if let Some((key, value)) = val.split_once('=') {
            if key.is_empty() {
                return Err((
                    format!("rust-bash: invalid --env format, empty key: {val}"),
                    2,
                ));
            }
            env.insert(key.to_string(), value.to_string());
        } else {
            return Err((
                format!("rust-bash: invalid --env format, expected KEY=VALUE: {val}"),
                2,
            ));
        }
    }
    Ok(env)
}

fn run(cli: Cli) -> ExitCode {
    // MCP server mode — enter JSON-RPC stdio loop
    if cli.mcp {
        match rust_bash::mcp::run_mcp_server() {
            Ok(()) => return ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rust-bash: MCP server error: {e}");
                return ExitCode::from(1);
            }
        }
    }

    let files = match parse_file_mappings(&cli.file_mappings) {
        Ok(f) => f,
        Err((msg, code)) => {
            eprintln!("{msg}");
            return ExitCode::from(code);
        }
    };

    let cwd = cli.cwd.as_deref().unwrap_or("/");

    let env = match parse_env(&cli.env, cwd) {
        Ok(e) => e,
        Err((msg, code)) => {
            eprintln!("{msg}");
            return ExitCode::from(code);
        }
    };

    let builder = RustBashBuilder::new().files(files).env(env).cwd(cwd);
    let mut shell = match builder.build() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rust-bash: failed to initialize: {e}");
            return ExitCode::from(2);
        }
    };

    // Mode dispatch: -c > script file > stdin > REPL
    if let Some(cmd) = &cli.command {
        // TODO: bash sets $0 from args[0] and $1.. from args[1..] when -c is combined with positional args
        return execute_and_output(&mut shell, cmd, cli.json);
    }

    if !cli.args.is_empty() {
        let script_path = &cli.args[0];
        let source = match std::fs::read_to_string(script_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rust-bash: {script_path}: {e}");
                return ExitCode::from(2);
            }
        };

        shell.set_shell_name(script_path.clone());
        shell.set_positional_params(cli.args[1..].to_vec());

        return execute_and_output(&mut shell, &source, cli.json);
    }

    if !std::io::stdin().is_terminal() {
        let source = match std::io::read_to_string(std::io::stdin()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rust-bash: error reading stdin: {e}");
                return ExitCode::from(2);
            }
        };
        return execute_and_output(&mut shell, &source, cli.json);
    }

    // REPL mode
    if cli.json {
        eprintln!("rust-bash: --json is not supported in interactive mode");
        return ExitCode::from(2);
    }

    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut rl: Editor<ShellHelper, rustyline::history::DefaultHistory> =
        Editor::with_config(config).expect("failed to create readline editor");

    let mut command_names: Vec<String> = shell
        .command_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    command_names.sort();
    rl.set_helper(Some(ShellHelper {
        commands: command_names,
        last_exit: 0,
    }));

    if let Some(ref hpath) = history_path() {
        let _ = rl.load_history(hpath);
    }

    let mut last_exit: i32 = 0;

    loop {
        let prompt = make_prompt(shell.cwd());
        match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                last_exit = match shell.exec(trimmed) {
                    Ok(result) => {
                        if !result.stdout.is_empty() {
                            print!("{}", result.stdout);
                        }
                        if !result.stderr.is_empty() {
                            eprint!("{}", result.stderr);
                        }
                        result.exit_code
                    }
                    Err(e) => {
                        eprintln!("rust-bash: {e}");
                        1
                    }
                };

                if let Some(h) = rl.helper_mut() {
                    h.last_exit = last_exit;
                }

                if shell.should_exit() {
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("rust-bash: readline error: {e}");
                break;
            }
        }
    }

    if let Some(ref hpath) = history_path() {
        let _ = rl.save_history(hpath);
    }

    ExitCode::from((last_exit & 0xFF) as u8)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    run(cli)
}
