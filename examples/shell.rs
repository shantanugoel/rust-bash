//! Minimal interactive REPL demonstrating library-level embedding of rust-bash.
//!
//! For the production CLI binary, run: `cargo run` or `rust-bash` (after install).
//! This example shows how to build a custom REPL using the RustBash API directly.

use rust_bash::{RustBash, RustBashBuilder};
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{CompletionType, Config, Context, Editor, Helper};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;

// ── CLI argument parsing ────────────────────────────────────────────

struct CliArgs {
    env: HashMap<String, String>,
    files_dir: Option<String>,
}

fn parse_args() -> CliArgs {
    let mut args = CliArgs {
        env: HashMap::new(),
        files_dir: None,
    };
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--env" => {
                i += 1;
                if i < raw.len()
                    && let Some((k, v)) = raw[i].split_once('=')
                {
                    args.env.insert(k.to_string(), v.to_string());
                }
            }
            "--files" => {
                i += 1;
                if i < raw.len() {
                    args.files_dir = Some(raw[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }
    args
}

// ── Readline helper (completion + validation) ───────────────────────

struct ShellHelper {
    commands: Vec<String>,
    last_exit: i32,
    is_tty: bool,
}

impl Completer for ShellHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        // Only complete the first token (command name).
        let prefix = &line[..pos];
        let start = prefix
            .rfind(|c: char| c.is_whitespace())
            .map_or(0, |i| i + 1);
        // If there's a space before cursor, we're past the command name — skip.
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
        if !self.is_tty {
            return Cow::Borrowed(prompt);
        }
        let color = if self.last_exit == 0 {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };
        Cow::Owned(format!("{color}{prompt}\x1b[0m"))
    }
}

impl Helper for ShellHelper {}

// ── VFS seeding from host directory ─────────────────────────────────

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

// ── Prompt formatting ───────────────────────────────────────────────

fn make_prompt(cwd: &str) -> String {
    format!("rust-bash:{cwd}$ ")
}

// ── History file path ───────────────────────────────────────────────

fn history_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".rust_bash_history"))
}

// ── Main ────────────────────────────────────────────────────────────

fn main() {
    let cli = parse_args();
    let is_tty = std::io::stdin().is_terminal();

    // Collect files to seed into VFS.
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    if let Some(ref dir) = cli.files_dir {
        files.extend(load_host_dir(Path::new(dir), ""));
    }

    // Default environment.
    let mut env: HashMap<String, String> = HashMap::from([
        ("HOME".into(), "/home".into()),
        ("USER".into(), "user".into()),
        ("PWD".into(), "/".into()),
    ]);
    env.extend(cli.env);

    let mut shell = RustBashBuilder::new()
        .files(files)
        .env(env)
        .build()
        .expect("failed to build RustBash instance");

    // Build the readline editor.
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut rl: Editor<ShellHelper, rustyline::history::DefaultHistory> =
        Editor::with_config(config).expect("failed to create editor");

    let mut command_names: Vec<String> = shell
        .command_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    command_names.sort();
    rl.set_helper(Some(ShellHelper {
        commands: command_names,
        last_exit: 0,
        is_tty,
    }));

    // Load history (ignore errors — file may not exist yet).
    if let Some(ref hpath) = history_path() {
        let _ = rl.load_history(hpath);
    }

    loop {
        let prompt = make_prompt(shell.cwd());
        match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                let exit_code = match shell.exec(trimmed) {
                    Ok(result) => {
                        if !result.stdout.is_empty() {
                            print!("{}", result.stdout);
                        }
                        if !result.stderr.is_empty() {
                            eprint!("{}", result.stderr);
                        }
                        if result.exit_code != 0 && is_tty && !shell.should_exit() {
                            eprintln!("[exit: {}]", result.exit_code);
                        }
                        result.exit_code
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        1
                    }
                };

                if let Some(h) = rl.helper_mut() {
                    h.last_exit = exit_code;
                }

                if shell.should_exit() {
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C: cancel current input, continue REPL.
                if is_tty {
                    println!("^C");
                }
            }
            Err(ReadlineError::Eof) => {
                // Ctrl-D: graceful exit.
                break;
            }
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }

    // Save history.
    if let Some(ref hpath) = history_path() {
        let _ = rl.save_history(hpath);
    }
}
