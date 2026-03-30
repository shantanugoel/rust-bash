//! Tests for `--help` flag support across all commands and builtins.

use rust_bash::RustBashBuilder;

fn shell() -> rust_bash::RustBash {
    RustBashBuilder::new().build().unwrap()
}

// ── Registered commands with --help ─────────────────────────────────

#[test]
fn grep_help_shows_usage_and_exits_zero() {
    let mut sh = shell();
    let r = sh.exec("grep --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"), "expected Usage header");
    assert!(r.stdout.contains("grep"), "expected command name in output");
    assert!(r.stderr.is_empty());
}

#[test]
fn ls_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("ls --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("ls"));
}

#[test]
fn cat_help_shows_options() {
    let mut sh = shell();
    let r = sh.exec("cat --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("-n"));
}

#[test]
fn sort_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("sort --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
}

#[test]
fn sed_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("sed --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("sed"));
}

#[test]
fn awk_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("awk --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("awk"));
}

#[test]
fn jq_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("jq --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
}

// ── Bash compatibility opt-outs ─────────────────────────────────────

#[test]
fn echo_help_prints_literal_help() {
    let mut sh = shell();
    let r = sh.exec("echo --help").unwrap();
    assert_eq!(
        r.stdout, "--help\n",
        "echo --help must print literal --help"
    );
    assert_eq!(r.exit_code, 0);
}

#[test]
fn true_help_exits_zero_silently() {
    let mut sh = shell();
    let r = sh.exec("true --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.is_empty(), "true --help must produce no output");
}

#[test]
fn false_help_exits_one_silently() {
    let mut sh = shell();
    let r = sh.exec("false --help").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stdout.is_empty(), "false --help must produce no output");
}

#[test]
fn test_help_works_as_expression() {
    let mut sh = shell();
    // `test --help` treats --help as a string operand (truthy → exit 0)
    let r = sh.exec("test --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.is_empty());
}

#[test]
fn bracket_help_works_as_expression() {
    let mut sh = shell();
    // `[ --help ]` treats --help as a string operand (truthy → exit 0)
    let r = sh.exec("[ --help ]").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.is_empty());
}

// ── Builtin commands with --help ────────────────────────────────────

#[test]
fn cd_help_shows_builtin_help() {
    let mut sh = shell();
    let r = sh.exec("cd --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("cd"));
}

#[test]
fn export_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("export --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("export"));
}

#[test]
fn declare_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("declare --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
}

#[test]
fn printf_help_works_via_builtin_dispatch() {
    let mut sh = shell();
    // printf is dispatched as a builtin first, so --help must be intercepted
    let r = sh.exec("printf --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("printf"));
}

#[test]
fn shopt_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("shopt --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
}

#[test]
fn read_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("read --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("read"));
}

#[test]
fn alias_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("alias --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
}

// ── All registered commands have meta() returning Some ──────────────

#[test]
fn all_registered_commands_have_meta() {
    let sh = shell();
    let commands = sh.command_names();
    let missing: Vec<&str> = commands
        .iter()
        .filter(|name| {
            // commands_meta returns None if no meta defined
            sh.command_meta(name).is_none()
        })
        .copied()
        .collect();
    assert!(missing.is_empty(), "Commands missing meta(): {:?}", missing);
}

// ── Commands without supports_help_flag fall through correctly ──────

#[test]
fn help_does_not_interfere_with_later_args() {
    let mut sh = shell();
    // --help only triggers when it's the first argument
    let r = sh.exec("grep -i --help").unwrap();
    // This should NOT show help — --help is not the first arg
    // (it will fail because no pattern, but that's OK)
    assert!(!r.stdout.contains("Usage:"));
}

// ── exec --help ─────────────────────────────────────────────────────

#[test]
fn exec_help_shows_usage() {
    let mut sh = shell();
    let r = sh.exec("exec --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("exec"));
}

// ── command/builtin wrappers pass through --help ────────────────────

#[test]
fn command_wrapper_help() {
    let mut sh = shell();
    let r = sh.exec("command printf --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("printf"));
}

#[test]
fn builtin_wrapper_help() {
    let mut sh = shell();
    let r = sh.exec("builtin cd --help").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"));
    assert!(r.stdout.contains("cd"));
}
