//! Integration tests for the rust-bash public API.
//!
//! These tests exercise `RustBash` and `RustBashBuilder` end-to-end,
//! covering commands, compound statements, redirections, state
//! persistence, and builder configuration.

use rust_bash::{RustBash, RustBashBuilder};
use std::collections::HashMap;

// ── Helpers ────────────────────────────────────────────────────────

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

// ── 1. Basic execution ─────────────────────────────────────────────

#[test]
fn basic_echo() {
    let mut sh = shell();
    let r = sh.exec("echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stderr, "");
}

// ── 2. Sequential commands ─────────────────────────────────────────

#[test]
fn sequential_commands() {
    let mut sh = shell();
    let r = sh.exec("echo a; echo b").unwrap();
    assert_eq!(r.stdout, "a\nb\n");
}

// ── 3. Pipeline ────────────────────────────────────────────────────

#[test]
fn pipeline_echo_cat() {
    let mut sh = shell();
    let r = sh.exec("echo hello | cat").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

// ── 4. Redirections ────────────────────────────────────────────────

#[test]
fn redirect_to_file_and_cat() {
    let mut sh = shell();
    let r = sh.exec("echo hello > /file.txt && cat /file.txt").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert_eq!(r.exit_code, 0);
}

// ── 5. State persistence ──────────────────────────────────────────

#[test]
fn state_persists_across_exec_calls() {
    let mut sh = shell();
    sh.exec("FOO=hello").unwrap();
    sh.exec("echo data > /persist.txt").unwrap();
    sh.exec("mkdir -p /mydir && cd /mydir").unwrap();

    // Variables survive
    let r = sh.exec("echo $FOO").unwrap();
    assert_eq!(r.stdout, "hello\n");

    // Files survive
    let r = sh.exec("cat /persist.txt").unwrap();
    assert_eq!(r.stdout, "data\n");

    // CWD survives
    let r = sh.exec("pwd").unwrap();
    assert_eq!(r.stdout, "/mydir\n");
}

// ── 6. If/else ─────────────────────────────────────────────────────

#[test]
fn if_else_true_branch() {
    let mut sh = shell();
    let r = sh.exec("if true; then echo yes; else echo no; fi").unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn if_else_false_branch() {
    let mut sh = shell();
    let r = sh
        .exec("if false; then echo yes; else echo no; fi")
        .unwrap();
    assert_eq!(r.stdout, "no\n");
}

#[test]
fn if_elif_else() {
    let mut sh = shell();
    let r = sh
        .exec("if false; then echo a; elif true; then echo b; else echo c; fi")
        .unwrap();
    assert_eq!(r.stdout, "b\n");
}

// ── 7. For loop ────────────────────────────────────────────────────

#[test]
fn for_loop_basic() {
    let mut sh = shell();
    let r = sh.exec("for i in a b c; do echo $i; done").unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

// ── 8. Variable assignment ─────────────────────────────────────────

#[test]
fn variable_assignment_and_expansion() {
    let mut sh = shell();
    let r = sh.exec("FOO=bar; echo $FOO").unwrap();
    assert_eq!(r.stdout, "bar\n");
}

// ── 9. Pre-command assignment ──────────────────────────────────────

#[test]
fn pre_command_assignment_not_persisted() {
    let mut sh = shell();
    let r = sh.exec("FOO=bar echo done; echo $FOO").unwrap();
    assert_eq!(r.stdout, "done\n\n");
}

// ── 10. Subshell isolation ─────────────────────────────────────────

#[test]
fn subshell_isolates_variables() {
    let mut sh = shell();
    let r = sh.exec("X=outer; (X=inner; echo $X); echo $X").unwrap();
    assert_eq!(r.stdout, "inner\nouter\n");
}

#[test]
fn subshell_isolates_cwd() {
    let mut sh = shell();
    sh.exec("mkdir /tmp").unwrap();
    let r = sh.exec("(cd /tmp && pwd); pwd").unwrap();
    assert_eq!(r.stdout, "/tmp\n/\n");
}

#[test]
fn subshell_isolates_fs_writes() {
    let mut sh = shell();
    sh.exec("(echo data > /subshell_only.txt)").unwrap();
    let r = sh.exec("cat /subshell_only.txt").unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(r.stderr.contains("No such file"));
}

// ── 11. Error cases ────────────────────────────────────────────────

#[test]
fn parse_error() {
    let mut sh = shell();
    let r = sh.exec("if; then; fi; ;;");
    assert!(r.is_err());
}

#[test]
fn command_not_found_exit_127() {
    let mut sh = shell();
    let r = sh.exec("nonexistent_cmd").unwrap();
    assert_eq!(r.exit_code, 127);
    assert!(r.stderr.contains("command not found"));
}

// ── 12. While loop ─────────────────────────────────────────────────

#[test]
fn while_loop_simple() {
    let mut sh = shell();
    let r = sh
        .exec("x=true; while $x; do echo loop; x=false; done")
        .unwrap();
    assert_eq!(r.stdout, "loop\n");
}

#[test]
fn while_false_never_runs() {
    let mut sh = shell();
    let r = sh.exec("while false; do echo nope; done").unwrap();
    assert_eq!(r.stdout, "");
}

// ── 13. Case statement ─────────────────────────────────────────────

#[test]
fn case_exact_match() {
    let mut sh = shell();
    let r = sh
        .exec("case foo in foo) echo matched;; *) echo nope;; esac")
        .unwrap();
    assert_eq!(r.stdout, "matched\n");
}

#[test]
fn case_wildcard_fallthrough() {
    let mut sh = shell();
    let r = sh
        .exec("case xyz in abc) echo no;; *) echo default;; esac")
        .unwrap();
    assert_eq!(r.stdout, "default\n");
}

#[test]
fn case_no_match() {
    let mut sh = shell();
    let r = sh.exec("case xyz in abc) echo nope;; esac").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

// ── 14. Brace group ───────────────────────────────────────────────

#[test]
fn brace_group() {
    let mut sh = shell();
    let r = sh.exec("{ echo a; echo b; }").unwrap();
    assert_eq!(r.stdout, "a\nb\n");
}

#[test]
fn brace_group_shares_scope() {
    let mut sh = shell();
    sh.exec("V=before").unwrap();
    sh.exec("{ V=after; }").unwrap();
    let r = sh.exec("echo $V").unwrap();
    assert_eq!(r.stdout, "after\n");
}

// ── 15. cd + pwd ──────────────────────────────────────────────────

#[test]
fn cd_and_pwd() {
    let mut sh = shell();
    let r = sh.exec("mkdir -p /tmp && cd /tmp && pwd").unwrap();
    assert_eq!(r.stdout, "/tmp\n");
}

// ── 16. Export + env visibility ────────────────────────────────────

#[test]
fn export_and_expand() {
    let mut sh = shell();
    let r = sh.exec("export FOO=bar; echo $FOO").unwrap();
    assert_eq!(r.stdout, "bar\n");
}

#[test]
fn export_marks_variable() {
    let mut sh = shell();
    sh.exec("FOO=bar").unwrap();
    sh.exec("export FOO").unwrap();
    let r = sh.exec("export").unwrap();
    assert!(r.stdout.contains("FOO"));
}

// ── 17. Readonly ──────────────────────────────────────────────────

#[test]
fn readonly_prevents_reassignment() {
    let mut sh = shell();
    sh.exec("readonly X=5").unwrap();
    let r = sh.exec("X=6");
    assert!(r.is_err());
}

#[test]
fn readonly_preserves_value() {
    let mut sh = shell();
    sh.exec("readonly X=5").unwrap();
    let r = sh.exec("echo $X").unwrap();
    assert_eq!(r.stdout, "5\n");
}

// ── 18. Nested pipelines ──────────────────────────────────────────

#[test]
fn nested_pipelines() {
    let mut sh = shell();
    let r = sh.exec(r"echo -e 'c\na\nb' | cat | cat").unwrap();
    assert_eq!(r.stdout, "c\na\nb\n");
}

// ── 19. Redirect to /dev/null ─────────────────────────────────────

#[test]
fn redirect_to_dev_null() {
    let mut sh = shell();
    let r = sh.exec("echo hidden > /dev/null").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

// ── 20. Builder with files ────────────────────────────────────────

#[test]
fn builder_with_files() {
    let mut files = HashMap::new();
    files.insert("/data/config.txt".to_string(), b"key=value\n".to_vec());
    files.insert("/readme.md".to_string(), b"# Hello\n".to_vec());

    let mut sh = RustBashBuilder::new().files(files).build().unwrap();

    let r = sh.exec("cat /data/config.txt").unwrap();
    assert_eq!(r.stdout, "key=value\n");

    let r = sh.exec("cat /readme.md").unwrap();
    assert_eq!(r.stdout, "# Hello\n");
}

// ── 21. Builder with env ──────────────────────────────────────────

#[test]
fn builder_with_env() {
    let mut env = HashMap::new();
    env.insert("GREETING".to_string(), "hello world".to_string());
    env.insert("HOME".to_string(), "/home/test".to_string());

    let mut sh = RustBashBuilder::new().env(env).build().unwrap();

    let r = sh.exec("echo $GREETING").unwrap();
    assert_eq!(r.stdout, "hello world\n");

    let r = sh.exec("echo ~").unwrap();
    assert_eq!(r.stdout, "/home/test\n");
}

// ── 22. Multiple exec calls ───────────────────────────────────────

#[test]
fn multiple_exec_calls_state_persists() {
    let mut sh = shell();

    // First call: set up state
    sh.exec("A=1").unwrap();
    sh.exec("export B=2").unwrap();
    sh.exec("echo file-data > /state.txt").unwrap();
    sh.exec("mkdir -p /workdir && cd /workdir").unwrap();

    // Second call: verify everything persists
    let r = sh.exec("echo $A $B").unwrap();
    assert_eq!(r.stdout, "1 2\n");

    let r = sh.exec("cat /state.txt").unwrap();
    assert_eq!(r.stdout, "file-data\n");

    let r = sh.exec("pwd").unwrap();
    assert_eq!(r.stdout, "/workdir\n");

    // Third call: modify and verify
    sh.exec("A=changed").unwrap();
    let r = sh.exec("echo $A").unwrap();
    assert_eq!(r.stdout, "changed\n");
}

// ── 23. ls output ─────────────────────────────────────────────────

#[test]
fn ls_shows_created_files() {
    let mut sh = shell();
    sh.exec("touch /a.txt").unwrap();
    sh.exec("touch /b.txt").unwrap();
    let r = sh.exec("ls /").unwrap();
    assert!(r.stdout.contains("a.txt"));
    assert!(r.stdout.contains("b.txt"));
}

#[test]
fn ls_one_per_line() {
    let mut sh = shell();
    sh.exec("touch /alpha").unwrap();
    sh.exec("touch /beta").unwrap();
    let r = sh.exec("ls -1 /").unwrap();
    assert_eq!(r.stdout, "alpha\nbeta\n");
}

// ── Additional edge cases ─────────────────────────────────────────

#[test]
fn and_or_lists() {
    let mut sh = shell();

    // && short-circuits on failure
    let r = sh.exec("false && echo nope").unwrap();
    assert_eq!(r.stdout, "");

    // || runs alternative on failure
    let r = sh.exec("false || echo fallback").unwrap();
    assert_eq!(r.stdout, "fallback\n");

    // chained
    let r = sh.exec("false || true && echo yes").unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn pipeline_negation() {
    let mut sh = shell();
    let r = sh.exec("! true").unwrap();
    assert_eq!(r.exit_code, 1);
    let r = sh.exec("! false").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn here_string() {
    let mut sh = shell();
    let r = sh.exec("cat <<< 'hello world'").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn here_string_with_variable() {
    let mut sh = shell();
    let r = sh.exec(r#"X=hello; cat <<< "$X world""#).unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn here_string_unquoted_variable() {
    let mut sh = shell();
    let r = sh.exec("X=hello; cat <<< $X").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn heredoc_basic() {
    let mut sh = shell();
    let r = sh.exec("cat <<EOF\nhello world\nEOF").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn heredoc_multiline() {
    let mut sh = shell();
    let r = sh.exec("cat <<EOF\nline1\nline2\nline3\nEOF").unwrap();
    assert_eq!(r.stdout, "line1\nline2\nline3\n");
}

#[test]
fn heredoc_empty() {
    let mut sh = shell();
    let r = sh.exec("cat <<EOF\nEOF").unwrap();
    assert_eq!(r.stdout, "");
}

#[test]
fn heredoc_variable_expansion() {
    let mut sh = shell();
    let r = sh.exec("X=hello; cat <<EOF\n$X world\nEOF").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn heredoc_quoted_delimiter_no_expansion() {
    let mut sh = shell();
    let r = sh.exec("X=hello; cat <<'EOF'\n$X world\nEOF").unwrap();
    assert_eq!(r.stdout, "$X world\n");
}

#[test]
fn heredoc_double_quoted_delimiter_no_expansion() {
    let mut sh = shell();
    let r = sh.exec("X=hello; cat <<\"EOF\"\n$X world\nEOF").unwrap();
    assert_eq!(r.stdout, "$X world\n");
}

#[test]
fn heredoc_tab_stripping() {
    let mut sh = shell();
    let r = sh.exec("cat <<-EOF\n\thello\n\tworld\nEOF").unwrap();
    assert_eq!(r.stdout, "hello\nworld\n");
}

#[test]
fn heredoc_tab_stripping_mixed() {
    let mut sh = shell();
    let r = sh
        .exec("cat <<-EOF\n\t\tindented\nnot indented\nEOF")
        .unwrap();
    assert_eq!(r.stdout, "indented\nnot indented\n");
}

#[test]
fn heredoc_command_substitution() {
    let mut sh = shell();
    let r = sh.exec("cat <<EOF\n$(echo hello)\nEOF").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn heredoc_piped() {
    let mut sh = shell();
    let r = sh.exec("cat <<EOF | cat\nhello\nEOF").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn heredoc_with_special_chars() {
    let mut sh = shell();
    let r = sh.exec("cat <<EOF\n* ? [ ] { }\nEOF").unwrap();
    assert_eq!(r.stdout, "* ? [ ] { }\n");
}

#[test]
fn redirect_append() {
    let mut sh = shell();
    sh.exec("echo first > /out.txt").unwrap();
    sh.exec("echo second >> /out.txt").unwrap();
    let r = sh.exec("cat /out.txt").unwrap();
    assert_eq!(r.stdout, "first\nsecond\n");
}

#[test]
fn redirect_stderr_to_stdout() {
    let mut sh = shell();
    let r = sh.exec("nonexistent 2>&1").unwrap();
    assert!(r.stdout.contains("command not found"));
    assert_eq!(r.stderr, "");
}

#[test]
fn compound_command_with_redirect() {
    let mut sh = shell();
    sh.exec("{ echo a; echo b; } > /out.txt").unwrap();
    let r = sh.exec("cat /out.txt").unwrap();
    assert_eq!(r.stdout, "a\nb\n");
}

#[test]
fn for_loop_in_pipeline() {
    let mut sh = shell();
    let r = sh.exec("for i in x y z; do echo $i; done | cat").unwrap();
    assert_eq!(r.stdout, "x\ny\nz\n");
}

#[test]
fn nested_if_in_for() {
    let mut sh = shell();
    let r = sh
        .exec("for x in yes no; do if true; then echo $x; fi; done")
        .unwrap();
    assert_eq!(r.stdout, "yes\nno\n");
}

#[test]
fn special_variables() {
    let mut sh = shell();

    // $? tracks last exit code
    sh.exec("false").unwrap();
    let r = sh.exec("echo $?").unwrap();
    assert_eq!(r.stdout, "1\n");

    // $0 is shell name
    let r = sh.exec("echo $0").unwrap();
    assert_eq!(r.stdout, "rust-bash\n");

    // $$ is PID (we use 1)
    let r = sh.exec("echo $$").unwrap();
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn positional_params() {
    let mut sh = shell();
    sh.exec("set -- a b c").unwrap();
    let r = sh.exec("echo $1 $2 $3").unwrap();
    assert_eq!(r.stdout, "a b c\n");

    let r = sh.exec("echo $#").unwrap();
    assert_eq!(r.stdout, "3\n");

    let r = sh.exec("echo $@").unwrap();
    assert_eq!(r.stdout, "a b c\n");

    sh.exec("shift").unwrap();
    let r = sh.exec("echo $1 $#").unwrap();
    assert_eq!(r.stdout, "b 2\n");
}

#[test]
fn variable_expansion_operators() {
    let mut sh = shell();

    // default value
    let r = sh.exec("echo ${UNSET:-fallback}").unwrap();
    assert_eq!(r.stdout, "fallback\n");

    // string length
    sh.exec("W=hello").unwrap();
    let r = sh.exec("echo ${#W}").unwrap();
    assert_eq!(r.stdout, "5\n");

    // suffix removal
    sh.exec("F=file.tar.gz").unwrap();
    let r = sh.exec("echo ${F%.*}").unwrap();
    assert_eq!(r.stdout, "file.tar\n");

    // prefix removal
    sh.exec("P=/a/b/c").unwrap();
    let r = sh.exec("echo ${P##*/}").unwrap();
    assert_eq!(r.stdout, "c\n");

    // substitution
    sh.exec("S=hello").unwrap();
    let r = sh.exec("echo ${S/l/r}").unwrap();
    assert_eq!(r.stdout, "herlo\n");
}

#[test]
fn echo_flags() {
    let mut sh = shell();

    // -n suppresses newline
    let r = sh.exec("echo -n hello").unwrap();
    assert_eq!(r.stdout, "hello");

    // -e interprets escapes
    let r = sh.exec(r"echo -e 'a\tb'").unwrap();
    assert_eq!(r.stdout, "a\tb\n");
}

#[test]
fn empty_and_whitespace_input() {
    let mut sh = shell();

    let r = sh.exec("").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);

    let r = sh.exec("   ").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn exit_builtin() {
    let mut sh = shell();

    let r = sh.exec("exit").unwrap();
    assert_eq!(r.exit_code, 0);

    let r = sh.exec("exit 42").unwrap();
    assert_eq!(r.exit_code, 42);

    // exit stops subsequent commands
    let r = sh.exec("exit 1; echo nope").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(!r.stdout.contains("nope"));
}

#[test]
fn builder_custom_cwd() {
    let mut sh = RustBashBuilder::new().cwd("/home/user").build().unwrap();
    let r = sh.exec("pwd").unwrap();
    assert_eq!(r.stdout, "/home/user\n");
}

#[test]
fn until_loop() {
    let mut sh = shell();
    let r = sh.exec("until true; do echo nope; done").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn case_with_variable() {
    let mut sh = shell();
    sh.exec("VAL=hello").unwrap();
    let r = sh
        .exec("case $VAL in hello) echo matched;; *) echo no;; esac")
        .unwrap();
    assert_eq!(r.stdout, "matched\n");
}

#[test]
fn declare_readonly() {
    let mut sh = shell();
    sh.exec("declare -r Y=99").unwrap();
    let r = sh.exec("echo $Y").unwrap();
    assert_eq!(r.stdout, "99\n");
    let r = sh.exec("Y=100");
    assert!(r.is_err());
}

#[test]
fn read_from_file() {
    let mut sh = shell();
    sh.exec("echo 'hello world' > /input.txt").unwrap();
    sh.exec("read VAR < /input.txt").unwrap();
    let r = sh.exec("echo $VAR").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn colon_builtin() {
    let mut sh = shell();
    let r = sh.exec(":").unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "");
}

#[test]
fn comments_stripped() {
    let mut sh = shell();
    let r = sh.exec("echo hello # this is a comment").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn double_quoted_expansion() {
    let mut sh = shell();
    sh.exec("MSG='hello world'").unwrap();
    let r = sh.exec("echo \"$MSG\"").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

// ── Snapshot tests ─────────────────────────────────────────────────

#[test]
fn snapshot_ls_long() {
    let mut sh = shell();
    sh.exec("touch /alpha.txt").unwrap();
    sh.exec("touch /beta.txt").unwrap();
    sh.exec("mkdir /docs").unwrap();
    let r = sh.exec("ls -l /").unwrap();
    insta::assert_snapshot!("ls_long", r.stdout);
}

#[test]
fn snapshot_ls_long_all() {
    let mut sh = shell();
    sh.exec("touch /alpha.txt").unwrap();
    sh.exec("touch /beta.txt").unwrap();
    sh.exec("mkdir /docs").unwrap();
    let r = sh.exec("ls -la /").unwrap();
    insta::assert_snapshot!("ls_long_all", r.stdout);
}

#[test]
fn snapshot_set_output() {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), "/home/user".to_string());
    env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
    let mut sh = RustBashBuilder::new().env(env).build().unwrap();

    // Add a non-exported var too
    sh.exec("LOCAL_VAR=local_value").unwrap();
    let r = sh.exec("set").unwrap();
    insta::assert_snapshot!("set_output", r.stdout);
}

#[test]
fn snapshot_export_output() {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), "/home/user".to_string());
    env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
    let mut sh = RustBashBuilder::new().env(env).build().unwrap();

    sh.exec("export APP_NAME=myapp").unwrap();
    let r = sh.exec("export").unwrap();
    insta::assert_snapshot!("export_output", r.stdout);
}

// ── Phase 2: Word Splitting & Quoting ──────────────────────────────

#[test]
fn word_split_unquoted_variable() {
    let mut sh = shell();
    sh.exec(r#"VAR="a b c""#).unwrap();
    let r = sh.exec("for w in $VAR; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

#[test]
fn word_split_quoted_variable_no_split() {
    let mut sh = shell();
    sh.exec(r#"VAR="a b c""#).unwrap();
    let r = sh.exec(r#"for w in "$VAR"; do echo $w; done"#).unwrap();
    assert_eq!(r.stdout, "a b c\n");
}

#[test]
fn word_split_dollar_at_quoted() {
    let mut sh = shell();
    sh.exec("set -- x y z").unwrap();
    let r = sh.exec(r#"for w in "$@"; do echo $w; done"#).unwrap();
    assert_eq!(r.stdout, "x\ny\nz\n");
}

#[test]
fn word_split_dollar_at_unquoted() {
    let mut sh = shell();
    sh.exec("set -- x y z").unwrap();
    let r = sh.exec("for w in $@; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "x\ny\nz\n");
}

#[test]
fn word_split_ifs_override() {
    let mut sh = shell();
    sh.exec(r#"IFS=:; VAR="a:b:c""#).unwrap();
    let r = sh.exec("for w in $VAR; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

#[test]
fn word_split_empty_variable_no_iterations() {
    let mut sh = shell();
    sh.exec(r#"VAR="""#).unwrap();
    let r = sh.exec("for w in $VAR; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "");
}

#[test]
fn word_split_dollar_at_preserves_spaces() {
    let mut sh = shell();
    sh.exec(r#"set -- "a b" "c d""#).unwrap();
    let r = sh.exec(r#"for w in "$@"; do echo $w; done"#).unwrap();
    assert_eq!(r.stdout, "a b\nc d\n");
}

#[test]
fn word_split_unset_variable_no_iterations() {
    let mut sh = shell();
    let r = sh.exec("for w in $UNSET_VAR; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "");
}

#[test]
fn word_split_multiple_spaces_collapse() {
    let mut sh = shell();
    sh.exec(r#"VAR="a   b   c""#).unwrap();
    let r = sh.exec("for w in $VAR; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

#[test]
fn word_split_newlines_in_value() {
    let mut sh = shell();
    // Set a variable with embedded newlines via printf-like assignment
    sh.exec("V=\"a\nb\nc\"").unwrap();
    let r = sh.exec("for w in $V; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

#[test]
fn word_split_quoted_preserves_newlines() {
    let mut sh = shell();
    sh.exec("V=\"a\nb\nc\"").unwrap();
    let r = sh.exec(r#"for w in "$V"; do echo "$w"; done"#).unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

#[test]
fn word_split_dollar_star_quoted_joins_with_ifs() {
    let mut sh = shell();
    sh.exec("set -- x y z").unwrap();
    let r = sh.exec(r#"IFS=:; echo "$*""#).unwrap();
    assert_eq!(r.stdout, "x:y:z\n");
}

#[test]
fn word_split_dollar_star_unquoted() {
    let mut sh = shell();
    sh.exec("set -- x y z").unwrap();
    let r = sh.exec("for w in $*; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "x\ny\nz\n");
}

#[test]
fn word_split_ifs_empty_no_splitting() {
    let mut sh = shell();
    sh.exec(r#"IFS=; VAR="a b c""#).unwrap();
    let r = sh.exec("for w in $VAR; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "a b c\n");
}

#[test]
fn word_split_unquoted_in_echo_args() {
    let mut sh = shell();
    sh.exec(r#"VAR="hello world""#).unwrap();
    let r = sh.exec("echo $VAR").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn word_split_quoted_in_echo_args() {
    let mut sh = shell();
    sh.exec(r#"VAR="hello   world""#).unwrap();
    let r = sh.exec(r#"echo "$VAR""#).unwrap();
    assert_eq!(r.stdout, "hello   world\n");
}

#[test]
fn word_split_assignment_no_split() {
    let mut sh = shell();
    sh.exec(r#"A="x  y  z""#).unwrap();
    sh.exec("B=$A").unwrap();
    let r = sh.exec(r#"echo "$B""#).unwrap();
    assert_eq!(r.stdout, "x  y  z\n");
}

#[test]
fn word_split_adjacent_text_and_expansion() {
    let mut sh = shell();
    sh.exec(r#"VAR="a b""#).unwrap();
    let r = sh.exec("for w in pre${VAR}post; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "prea\nbpost\n");
}

#[test]
fn word_split_ifs_colon_with_empty_fields() {
    let mut sh = shell();
    sh.exec(r#"IFS=:; VAR=":a::b:""#).unwrap();
    let r = sh.exec(r#"for w in $VAR; do echo "[$w]"; done"#).unwrap();
    assert_eq!(r.stdout, "[]\n[a]\n[]\n[b]\n[]\n");
}

#[test]
fn word_split_dollar_at_zero_params() {
    let mut sh = shell();
    sh.exec("set --").unwrap();
    let r = sh.exec(r#"for w in "$@"; do echo "$w"; done"#).unwrap();
    assert_eq!(r.stdout, "");
}

#[test]
fn word_split_dollar_star_empty_ifs() {
    let mut sh = shell();
    sh.exec("set -- a b c").unwrap();
    let r = sh.exec(r#"IFS=''; echo "$*""#).unwrap();
    assert_eq!(r.stdout, "abc\n");
}

#[test]
fn word_split_dollar_at_unquoted_with_ifs_chars() {
    let mut sh = shell();
    sh.exec(r#"set -- "a b" "c d""#).unwrap();
    let r = sh.exec("for w in $@; do echo $w; done").unwrap();
    assert_eq!(r.stdout, "a\nb\nc\nd\n");
}

// ── Command substitution ────────────────────────────────────────────

#[test]
fn cmd_subst_basic() {
    let mut sh = shell();
    let r = sh.exec("echo $(echo hello)").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn cmd_subst_assign_and_use() {
    let mut sh = shell();
    sh.exec("x=$(echo world)").unwrap();
    let r = sh.exec(r#"echo "hello $x""#).unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn cmd_subst_nested() {
    let mut sh = shell();
    let r = sh.exec("echo $(echo $(echo deep))").unwrap();
    assert_eq!(r.stdout, "deep\n");
}

#[test]
fn cmd_subst_trailing_newline_stripping() {
    let mut sh = shell();
    // echo produces "abc\n"; command substitution strips the trailing newline
    let r = sh.exec(r#"x=$(echo abc); echo "$x""#).unwrap();
    assert_eq!(r.stdout, "abc\n");
}

#[test]
fn cmd_subst_multiple_trailing_newlines_stripped() {
    let mut sh = shell();
    // echo -n "abc\n\n" produces "abc\n\n"; substitution strips both trailing newlines
    let r = sh
        .exec(r#"x=$(echo abc; echo ""; echo ""); echo "[$x]""#)
        .unwrap();
    assert_eq!(r.stdout, "[abc]\n");
}

#[test]
fn cmd_subst_in_double_quotes_preserves_spaces() {
    let mut sh = shell();
    let r = sh.exec(r#"echo "$(echo 'a  b')""#).unwrap();
    assert_eq!(r.stdout, "a  b\n");
}

#[test]
fn cmd_subst_unquoted_ifs_splitting() {
    let mut sh = shell();
    let r = sh
        .exec(r#"for w in $(echo "a b c"); do echo "[$w]"; done"#)
        .unwrap();
    assert_eq!(r.stdout, "[a]\n[b]\n[c]\n");
}

#[test]
fn cmd_subst_exit_code_reflected() {
    let mut sh = shell();
    let r = sh.exec("$(false); echo $?").unwrap();
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn cmd_subst_empty() {
    let mut sh = shell();
    let r = sh.exec(r#"echo "$(true)""#).unwrap();
    assert_eq!(r.stdout, "\n");
}

#[test]
fn cmd_subst_backtick_syntax() {
    let mut sh = shell();
    let r = sh.exec("echo `echo hello`").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn cmd_subst_subshell_isolation() {
    let mut sh = shell();
    sh.exec("x=before").unwrap();
    sh.exec("y=$(x=inside; echo $x)").unwrap();
    let r = sh.exec(r#"echo "$x $y""#).unwrap();
    assert_eq!(r.stdout, "before inside\n");
}

#[test]
fn cmd_subst_in_pipeline() {
    let mut sh = shell();
    let r = sh.exec("echo $(echo hello | cat)").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn cmd_subst_multiple_in_one_word() {
    let mut sh = shell();
    let r = sh.exec(r#"echo "$(echo hello) $(echo world)""#).unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

// ── eval builtin ────────────────────────────────────────────────────

#[test]
fn eval_simple_echo() {
    let mut sh = shell();
    let r = sh.exec("eval 'echo hello'").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn eval_with_variable_expansion() {
    let mut sh = shell();
    sh.exec("HOME=/my/home").unwrap();
    let r = sh.exec(r#"eval "echo $HOME""#).unwrap();
    assert_eq!(r.stdout, "/my/home\n");
}

#[test]
fn eval_assignment_persists() {
    let mut sh = shell();
    sh.exec("eval 'X=42'").unwrap();
    let r = sh.exec("echo $X").unwrap();
    assert_eq!(r.stdout, "42\n");
}

#[test]
fn eval_empty_string_is_noop() {
    let mut sh = shell();
    let r = sh.exec("eval ''").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn eval_no_args_is_noop() {
    let mut sh = shell();
    let r = sh.exec("eval").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn eval_multiple_args_concatenated() {
    let mut sh = shell();
    let r = sh.exec("eval echo hello world").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn eval_cd_persists() {
    let mut sh = shell();
    sh.exec("mkdir /newdir").unwrap();
    sh.exec("eval 'cd /newdir'").unwrap();
    let r = sh.exec("pwd").unwrap();
    assert_eq!(r.stdout, "/newdir\n");
}

// ── source builtin ──────────────────────────────────────────────────

#[test]
fn source_script_side_effects_persist() {
    let mut sh = shell();
    sh.exec("echo 'FOO=sourced' > /script.sh").unwrap();
    sh.exec("source /script.sh").unwrap();
    let r = sh.exec("echo $FOO").unwrap();
    assert_eq!(r.stdout, "sourced\n");
}

#[test]
fn source_dot_alias() {
    let mut sh = shell();
    sh.exec("echo 'BAR=dotted' > /script.sh").unwrap();
    sh.exec(". /script.sh").unwrap();
    let r = sh.exec("echo $BAR").unwrap();
    assert_eq!(r.stdout, "dotted\n");
}

#[test]
fn source_nonexistent_file_error() {
    let mut sh = shell();
    let r = sh.exec("source /nonexistent").unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(r.stderr.contains("No such file"));
}

#[test]
fn source_no_args_error() {
    let mut sh = shell();
    let r = sh.exec("source").unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(r.stderr.contains("filename argument required"));
}

#[test]
fn source_multiple_commands_in_file() {
    let mut sh = shell();
    sh.exec("echo 'A=1\nB=2\nC=3' > /multi.sh").unwrap();
    sh.exec("source /multi.sh").unwrap();
    let r = sh.exec(r#"echo "$A $B $C""#).unwrap();
    assert_eq!(r.stdout, "1 2 3\n");
}

#[test]
fn source_script_can_source_another() {
    let mut sh = shell();
    sh.exec("echo 'INNER=yes' > /inner.sh").unwrap();
    sh.exec("echo 'source /inner.sh' > /outer.sh").unwrap();
    sh.exec("source /outer.sh").unwrap();
    let r = sh.exec("echo $INNER").unwrap();
    assert_eq!(r.stdout, "yes\n");
}

// ── exec callback on CommandContext ─────────────────────────────────

#[test]
fn exec_callback_available_to_commands() {
    use rust_bash::{CommandContext, CommandResult, RustBashBuilder, VirtualCommand};

    struct ExecTestCmd;
    impl VirtualCommand for ExecTestCmd {
        fn name(&self) -> &str {
            "exectest"
        }
        fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
            match ctx.exec {
                Some(exec) => match exec("echo from-callback") {
                    Ok(r) => CommandResult {
                        stdout: r.stdout,
                        stderr: r.stderr,
                        exit_code: r.exit_code,
                    },
                    Err(e) => CommandResult {
                        stderr: format!("{e}\n"),
                        exit_code: 1,
                        ..CommandResult::default()
                    },
                },
                None => CommandResult {
                    stderr: "no exec callback\n".to_string(),
                    exit_code: 1,
                    ..CommandResult::default()
                },
            }
        }
    }

    let mut sh = RustBashBuilder::new()
        .command(Box::new(ExecTestCmd))
        .build()
        .unwrap();
    let r = sh.exec("exectest").unwrap();
    assert_eq!(r.stdout, "from-callback\n");
    assert_eq!(r.exit_code, 0);
}

// ── Phase 5: Conditional tests ────────────────────────────────────

// ── 5a. test / [ command ──────────────────────────────────────────

#[test]
fn test_file_exists() {
    let mut sh = shell();
    sh.exec("touch /existing.txt").unwrap();
    let r = sh.exec("test -f /existing.txt").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_file_not_exists() {
    let mut sh = shell();
    let r = sh.exec("test -f /nonexistent.txt").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn test_dir_exists() {
    let mut sh = shell();
    sh.exec("mkdir /mydir").unwrap();
    let r = sh.exec("[ -d /mydir ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_dir_not_file() {
    let mut sh = shell();
    sh.exec("mkdir /mydir").unwrap();
    let r = sh.exec("[ -f /mydir ]").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn test_exists_file() {
    let mut sh = shell();
    sh.exec("touch /file.txt").unwrap();
    let r = sh.exec("test -e /file.txt").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_exists_dir() {
    let mut sh = shell();
    sh.exec("mkdir /adir").unwrap();
    let r = sh.exec("test -e /adir").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_size_nonzero() {
    let mut sh = shell();
    sh.exec("echo hello > /file.txt").unwrap();
    let r = sh.exec("test -s /file.txt").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_size_zero() {
    let mut sh = shell();
    sh.exec("touch /empty.txt").unwrap();
    let r = sh.exec("test -s /empty.txt").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn test_readable_writable_executable() {
    let mut sh = shell();
    sh.exec("touch /file.txt").unwrap();
    let r = sh.exec("[ -r /file.txt ]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[ -w /file.txt ]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[ -x /file.txt ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_string_zero_length() {
    let mut sh = shell();
    let r = sh.exec("[ -z \"\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_string_non_zero_length() {
    let mut sh = shell();
    let r = sh.exec("[ -n \"hello\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_string_zero_nonempty_fails() {
    let mut sh = shell();
    let r = sh.exec("[ -z \"hello\" ]").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn test_string_equal() {
    let mut sh = shell();
    let r = sh.exec("[ \"abc\" = \"abc\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_string_not_equal() {
    let mut sh = shell();
    let r = sh.exec("[ \"abc\" != \"def\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_string_equal_double_eq() {
    let mut sh = shell();
    let r = sh.exec("[ \"abc\" == \"abc\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_string_less_than() {
    let mut sh = shell();
    let r = sh.exec("[ \"abc\" \\< \"def\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_string_greater_than() {
    let mut sh = shell();
    let r = sh.exec("[ \"def\" \\> \"abc\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_numeric_equal() {
    let mut sh = shell();
    let r = sh.exec("[ 5 -eq 5 ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_numeric_not_equal() {
    let mut sh = shell();
    let r = sh.exec("[ 5 -ne 3 ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_numeric_gt() {
    let mut sh = shell();
    let r = sh.exec("[ 5 -gt 3 ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_numeric_lt() {
    let mut sh = shell();
    let r = sh.exec("[ 3 -lt 5 ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_numeric_ge() {
    let mut sh = shell();
    let r = sh.exec("[ 5 -ge 5 ]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[ 6 -ge 5 ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_numeric_le() {
    let mut sh = shell();
    let r = sh.exec("[ 5 -le 5 ]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[ 4 -le 5 ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_negation_with_bang_pipeline() {
    let mut sh = shell();
    let r = sh.exec("! test -f /nonexistent").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_negation_inside_bracket() {
    let mut sh = shell();
    let r = sh.exec("[ ! -f /nonexistent ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_logical_and() {
    let mut sh = shell();
    sh.exec("touch /file.txt").unwrap();
    sh.exec("mkdir /dir").unwrap();
    let r = sh.exec("[ -f /file.txt -a -d /dir ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_logical_or() {
    let mut sh = shell();
    let r = sh.exec("[ -f /nonexistent -o -z \"\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_bracket_missing_close() {
    let mut sh = shell();
    let r = sh.exec("[ -f /file").unwrap();
    assert_eq!(r.exit_code, 2);
    assert!(r.stderr.contains("missing ']'"));
}

#[test]
fn test_no_args_is_false() {
    let mut sh = shell();
    let r = sh.exec("test").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn test_single_nonempty_string_is_true() {
    let mut sh = shell();
    let r = sh.exec("test hello").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn test_in_if_condition() {
    let mut sh = shell();
    sh.exec("touch /file.txt").unwrap();
    let r = sh.exec("if [ -f /file.txt ]; then echo found; fi").unwrap();
    assert_eq!(r.stdout, "found\n");
}

#[test]
fn test_in_if_condition_false() {
    let mut sh = shell();
    let r = sh
        .exec("if [ -f /nofile ]; then echo found; else echo missing; fi")
        .unwrap();
    assert_eq!(r.stdout, "missing\n");
}

#[test]
fn test_with_variable_expansion() {
    let mut sh = shell();
    sh.exec("X=hello").unwrap();
    let r = sh.exec("[ -n \"$X\" ]").unwrap();
    assert_eq!(r.exit_code, 0);
}

// ── 5b. [[ extended test ─────────────────────────────────────────

#[test]
fn extended_test_string_equal() {
    let mut sh = shell();
    let r = sh.exec("[[ \"hello\" == \"hello\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_string_not_equal() {
    let mut sh = shell();
    let r = sh.exec("[[ \"hello\" != \"world\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_pattern_match() {
    let mut sh = shell();
    let r = sh.exec("[[ \"hello\" == hel* ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_pattern_no_match() {
    let mut sh = shell();
    let r = sh.exec("[[ \"hello\" == wor* ]]").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn extended_test_regex_match() {
    let mut sh = shell();
    let r = sh.exec("[[ \"abc123\" =~ ^[a-z]+([0-9]+)$ ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    // BASH_REMATCH should contain the whole match
    let r2 = sh.exec("echo $BASH_REMATCH").unwrap();
    assert_eq!(r2.stdout, "abc123\n");
    // First capture group
    let r3 = sh.exec("echo $BASH_REMATCH_1").unwrap();
    assert_eq!(r3.stdout, "123\n");
}

#[test]
fn extended_test_regex_no_match() {
    let mut sh = shell();
    let r = sh.exec("[[ \"hello\" =~ ^[0-9]+$ ]]").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn extended_test_file_test() {
    let mut sh = shell();
    sh.exec("touch /testfile").unwrap();
    let r = sh.exec("[[ -f /testfile ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_logical_and() {
    let mut sh = shell();
    sh.exec("touch /file").unwrap();
    sh.exec("mkdir /dir").unwrap();
    let r = sh.exec("[[ -f /file && -d /dir ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_logical_or() {
    let mut sh = shell();
    let r = sh.exec("[[ -f /nofile || -z \"\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_logical_not() {
    let mut sh = shell();
    let r = sh.exec("[[ ! -f /nofile ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_numeric_comparison() {
    let mut sh = shell();
    let r = sh.exec("[[ 5 -gt 3 ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ 3 -lt 5 ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ 5 -eq 5 ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_string_comparison() {
    let mut sh = shell();
    let r = sh.exec("[[ \"abc\" < \"def\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ \"def\" > \"abc\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_with_variable() {
    let mut sh = shell();
    sh.exec("X=hello").unwrap();
    let r = sh.exec("[[ -n \"$X\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ \"$X\" == \"hello\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn extended_test_in_if() {
    let mut sh = shell();
    sh.exec("touch /file.txt").unwrap();
    let r = sh
        .exec("if [[ -f /file.txt ]]; then echo yes; else echo no; fi")
        .unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn extended_test_combined_and_or() {
    let mut sh = shell();
    let r = sh.exec("[[ 1 -eq 1 && 2 -lt 3 ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ 1 -eq 2 || 2 -lt 3 ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ 1 -eq 2 && 2 -lt 3 ]]").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn extended_test_empty_string_z() {
    let mut sh = shell();
    let r = sh.exec("[[ -z \"\" ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ -z \"hello\" ]]").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn extended_test_pattern_question_mark() {
    let mut sh = shell();
    let r = sh.exec("[[ \"abc\" == a?c ]]").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("[[ \"abc\" == a?d ]]").unwrap();
    assert_eq!(r.exit_code, 1);
}

// ── Loop control: break/continue ───────────────────────────────────

#[test]
fn break_in_for_loop() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2 3; do if [ $i = 2 ]; then break; fi; echo $i; done")
        .unwrap();
    assert_eq!(r.stdout, "1\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn continue_in_for_loop() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2 3; do if [ $i = 2 ]; then continue; fi; echo $i; done")
        .unwrap();
    assert_eq!(r.stdout, "1\n3\n");
}

#[test]
fn break_preserves_variable() {
    let mut sh = shell();
    let r = sh
        .exec("x=0; for i in a b c; do x=$i; break; done; echo $x")
        .unwrap();
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn break_exits_two_levels() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2; do for j in a b c; do if [ $j = b ]; then break 2; fi; echo $i$j; done; done")
        .unwrap();
    assert_eq!(r.stdout, "1a\n");
}

#[test]
fn continue_two_levels() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2 3; do for j in a b; do if [ $i = 2 ]; then continue 2; fi; echo $i$j; done; done")
        .unwrap();
    assert_eq!(r.stdout, "1a\n1b\n3a\n3b\n");
}

#[test]
fn break_outside_loop_error() {
    let mut sh = shell();
    let r = sh.exec("break").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("break"));
}

#[test]
fn continue_outside_loop_error() {
    let mut sh = shell();
    let r = sh.exec("continue").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("continue"));
}

#[test]
fn break_zero_error() {
    let mut sh = shell();
    let r = sh.exec("for i in 1 2 3; do break 0; done").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("loop count"));
}

#[test]
fn continue_zero_error() {
    let mut sh = shell();
    let r = sh.exec("for i in 1 2 3; do continue 0; done").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("loop count"));
}

#[test]
fn break_negative_error() {
    let mut sh = shell();
    let r = sh.exec("for i in 1; do break -1; done").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("loop count"));
}

#[test]
fn break_non_numeric_error() {
    let mut sh = shell();
    let r = sh.exec("for i in 1; do break abc; done").unwrap();
    assert!(r.exit_code != 0);
    assert!(r.stderr.contains("numeric argument"));
}

#[test]
fn break_in_while_loop() {
    let mut sh = shell();
    // Use a for loop to feed values to simulate a while with break
    let r = sh
        .exec("for i in 1 2 3 4 5; do if [ $i = 3 ]; then break; fi; echo $i; done")
        .unwrap();
    assert_eq!(r.stdout, "1\n2\n");
}

#[test]
fn continue_in_while_loop_via_for() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2 3 4 5; do if [ $i = 3 ]; then continue; fi; echo $i; done")
        .unwrap();
    assert_eq!(r.stdout, "1\n2\n4\n5\n");
}

#[test]
fn break_does_not_abort_script() {
    let mut sh = shell();
    let r = sh.exec("break; echo after").unwrap();
    assert!(r.stderr.contains("break"));
    assert_eq!(r.stdout, "after\n");
}

#[test]
fn continue_does_not_abort_script() {
    let mut sh = shell();
    let r = sh.exec("continue; echo after").unwrap();
    assert!(r.stderr.contains("continue"));
    assert_eq!(r.stdout, "after\n");
}

#[test]
fn break_large_n_exits_all_loops() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2; do for j in a b; do break 99; echo nope; done; echo nope2; done; echo done")
        .unwrap();
    assert_eq!(r.stdout, "done\n");
}

#[test]
fn break_in_until_loop() {
    let mut sh = shell();
    let r = sh
        .exec("x=yes; until [ $x = no ]; do echo once; break; done")
        .unwrap();
    assert_eq!(r.stdout, "once\n");
}

#[test]
fn nested_continue_inner_loop() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2; do for j in a b c; do if [ $j = b ]; then continue; fi; echo $i$j; done; done")
        .unwrap();
    assert_eq!(r.stdout, "1a\n1c\n2a\n2c\n");
}

// ── Glob expansion (Phase 7a) ──────────────────────────────────────

#[test]
fn glob_star_txt() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo a > /tmp/a.txt && echo b > /tmp/b.txt && echo c > /tmp/c.md")
        .unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo *.txt").unwrap();
    assert_eq!(r.stdout, "a.txt b.txt\n");
}

#[test]
fn glob_no_match_literal() {
    let mut sh = shell();
    let r = sh.exec("echo *.xyz").unwrap();
    assert_eq!(r.stdout, "*.xyz\n");
}

#[test]
fn glob_quoted_no_expand() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo a > /tmp/a.txt").unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo \"*.txt\"").unwrap();
    assert_eq!(r.stdout, "*.txt\n");
}

#[test]
fn glob_single_quoted_no_expand() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo a > /tmp/a.txt").unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo '*.txt'").unwrap();
    assert_eq!(r.stdout, "*.txt\n");
}

#[test]
fn glob_bracket_pattern() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo a > /tmp/a.txt && echo b > /tmp/b.txt && echo c > /tmp/c.txt")
        .unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo [ab].txt").unwrap();
    assert_eq!(r.stdout, "a.txt b.txt\n");
}

#[test]
fn glob_question_mark() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo x > /tmp/a.txt && echo x > /tmp/bb.txt")
        .unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo ?.txt").unwrap();
    assert_eq!(r.stdout, "a.txt\n");
}

#[test]
fn glob_absolute_path() {
    let mut sh = shell();
    sh.exec("mkdir -p /data && echo x > /data/f1.log && echo x > /data/f2.log")
        .unwrap();
    let r = sh.exec("echo /data/*.log").unwrap();
    assert_eq!(r.stdout, "/data/f1.log /data/f2.log\n");
}

#[test]
fn glob_ls_command() {
    let mut sh = shell();
    sh.exec("mkdir -p /lstest/sub && echo a > /lstest/sub/x.txt && echo b > /lstest/sub/y.txt")
        .unwrap();
    // Glob expansion should resolve /lstest/s* to /lstest/sub, then ls lists that dir
    let r = sh.exec("ls /lstest/s*").unwrap();
    assert!(r.stdout.contains("x.txt"), "stdout was: {}", r.stdout);
    assert!(r.stdout.contains("y.txt"), "stdout was: {}", r.stdout);
    assert_eq!(r.exit_code, 0);
}

#[test]
fn glob_recursive_doublestar() {
    let mut sh = shell();
    sh.exec("mkdir -p /proj/src/sub && echo x > /proj/README.md && echo x > /proj/src/lib.md && echo x > /proj/src/sub/deep.md")
        .unwrap();
    let r = sh.exec("echo /proj/**/*.md").unwrap();
    assert_eq!(
        r.stdout,
        "/proj/README.md /proj/src/lib.md /proj/src/sub/deep.md\n"
    );
}

#[test]
fn glob_hidden_files_skipped() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo x > /tmp/.hidden && echo x > /tmp/visible")
        .unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo *").unwrap();
    // Should not include .hidden
    assert!(!r.stdout.contains(".hidden"));
    assert!(r.stdout.contains("visible"));
}

#[test]
fn glob_hidden_files_explicit_dot() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo x > /tmp/.hidden && echo x > /tmp/.other")
        .unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo .*").unwrap();
    assert!(r.stdout.contains(".hidden"));
    assert!(r.stdout.contains(".other"));
}

#[test]
fn glob_var_expansion_then_glob() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo a > /tmp/a.txt && echo b > /tmp/b.txt")
        .unwrap();
    sh.exec("cd /tmp").unwrap();
    sh.exec("PAT='*.txt'").unwrap();
    // Unquoted $PAT should glob-expand
    let r = sh.exec("echo $PAT").unwrap();
    assert_eq!(r.stdout, "a.txt b.txt\n");
}

#[test]
fn glob_var_quoted_no_expand() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo a > /tmp/a.txt").unwrap();
    sh.exec("cd /tmp").unwrap();
    sh.exec("PAT='*.txt'").unwrap();
    // Quoted "$PAT" should NOT glob-expand
    let r = sh.exec("echo \"$PAT\"").unwrap();
    assert_eq!(r.stdout, "*.txt\n");
}

#[test]
fn glob_doublestar_skips_hidden_dirs() {
    let mut sh = shell();
    sh.exec("mkdir -p /d/.hidden && echo x > /d/.hidden/secret.md && echo x > /d/visible.md")
        .unwrap();
    let r = sh.exec("echo /d/**/*.md").unwrap();
    assert_eq!(r.stdout, "/d/visible.md\n");
}

// ── Brace expansion ───────────────────────────────────────────────

#[test]
fn brace_comma_alternation() {
    let mut sh = shell();
    let r = sh.exec("echo {a,b,c}").unwrap();
    assert_eq!(r.stdout, "a b c\n");
}

#[test]
fn brace_numeric_sequence() {
    let mut sh = shell();
    let r = sh.exec("echo {1..5}").unwrap();
    assert_eq!(r.stdout, "1 2 3 4 5\n");
}

#[test]
fn brace_numeric_sequence_with_step() {
    let mut sh = shell();
    let r = sh.exec("echo {1..10..2}").unwrap();
    assert_eq!(r.stdout, "1 3 5 7 9\n");
}

#[test]
fn brace_char_sequence() {
    let mut sh = shell();
    let r = sh.exec("echo {a..z}").unwrap();
    assert_eq!(
        r.stdout,
        "a b c d e f g h i j k l m n o p q r s t u v w x y z\n"
    );
}

#[test]
fn brace_char_sequence_reverse() {
    let mut sh = shell();
    let r = sh.exec("echo {z..a}").unwrap();
    assert_eq!(
        r.stdout,
        "z y x w v u t s r q p o n m l k j i h g f e d c b a\n"
    );
}

#[test]
fn brace_with_prefix_suffix() {
    let mut sh = shell();
    let r = sh.exec("echo file{1,2,3}.txt").unwrap();
    assert_eq!(r.stdout, "file1.txt file2.txt file3.txt\n");
}

#[test]
fn brace_nested() {
    let mut sh = shell();
    let r = sh.exec("echo {a,b{1,2}}").unwrap();
    assert_eq!(r.stdout, "a b1 b2\n");
}

#[test]
fn brace_single_item_no_expansion() {
    let mut sh = shell();
    let r = sh.exec("echo {a}").unwrap();
    assert_eq!(r.stdout, "{a}\n");
}

#[test]
fn brace_empty_alternative() {
    let mut sh = shell();
    let r = sh.exec("echo {a,}").unwrap();
    assert_eq!(r.stdout, "a \n");
}

#[test]
fn brace_no_interference_with_parameter_expansion() {
    let mut sh = shell();
    sh.exec("X=hello").unwrap();
    let r = sh.exec("echo ${X}").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn brace_combined_with_variable() {
    let mut sh = shell();
    sh.exec("X=test").unwrap();
    let r = sh.exec("echo ${X}{a,b}").unwrap();
    assert_eq!(r.stdout, "testa testb\n");
}

#[test]
fn brace_pre_and_post() {
    let mut sh = shell();
    let r = sh.exec("echo pre{a,b}post").unwrap();
    assert_eq!(r.stdout, "preapost prebpost\n");
}

#[test]
fn brace_multiple_groups() {
    let mut sh = shell();
    let r = sh.exec("echo {a,b}{1,2}").unwrap();
    assert_eq!(r.stdout, "a1 a2 b1 b2\n");
}

// ── Arithmetic expansion ───────────────────────────────────────────

#[test]
fn arith_basic_addition() {
    let mut sh = shell();
    let r = sh.exec("echo $((1 + 2))").unwrap();
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn arith_all_operators() {
    let mut sh = shell();
    assert_eq!(sh.exec("echo $((5 * 3))").unwrap().stdout, "15\n");
    assert_eq!(sh.exec("echo $((10 / 3))").unwrap().stdout, "3\n");
    assert_eq!(sh.exec("echo $((10 % 3))").unwrap().stdout, "1\n");
    assert_eq!(sh.exec("echo $((2 ** 10))").unwrap().stdout, "1024\n");
}

#[test]
fn arith_comparisons() {
    let mut sh = shell();
    assert_eq!(sh.exec("echo $((5 > 3))").unwrap().stdout, "1\n");
    assert_eq!(sh.exec("echo $((5 < 3))").unwrap().stdout, "0\n");
    assert_eq!(sh.exec("echo $((3 <= 3))").unwrap().stdout, "1\n");
    assert_eq!(sh.exec("echo $((3 >= 4))").unwrap().stdout, "0\n");
}

#[test]
fn arith_boolean() {
    let mut sh = shell();
    assert_eq!(sh.exec("echo $((1 && 0))").unwrap().stdout, "0\n");
    assert_eq!(sh.exec("echo $((1 || 0))").unwrap().stdout, "1\n");
}

#[test]
fn arith_bitwise() {
    let mut sh = shell();
    assert_eq!(sh.exec("echo $((0xFF & 0x0F))").unwrap().stdout, "15\n");
}

#[test]
fn arith_ternary() {
    let mut sh = shell();
    assert_eq!(sh.exec("echo $((5 > 3 ? 10 : 20))").unwrap().stdout, "10\n");
}

#[test]
fn arith_variables() {
    let mut sh = shell();
    let r = sh.exec("x=5; echo $((x + 3))").unwrap();
    assert_eq!(r.stdout, "8\n");
}

#[test]
fn arith_assignment() {
    let mut sh = shell();
    let r = sh.exec("echo $((x = 5)); echo $x").unwrap();
    assert_eq!(r.stdout, "5\n5\n");
}

#[test]
fn arith_compound_assignment() {
    let mut sh = shell();
    let r = sh.exec("x=10; echo $((x += 5))").unwrap();
    assert_eq!(r.stdout, "15\n");
}

#[test]
fn arith_pre_increment() {
    let mut sh = shell();
    let r = sh.exec("x=5; echo $((++x))").unwrap();
    assert_eq!(r.stdout, "6\n");
}

#[test]
fn arith_post_increment() {
    let mut sh = shell();
    let r = sh.exec("x=5; echo $((x++)); echo $x").unwrap();
    assert_eq!(r.stdout, "5\n6\n");
}

#[test]
fn arith_nested_parens() {
    let mut sh = shell();
    let r = sh.exec("echo $(( (1 + 2) * 3 ))").unwrap();
    assert_eq!(r.stdout, "9\n");
}

#[test]
fn arith_division_by_zero() {
    let mut sh = shell();
    let r = sh.exec("echo $((1 / 0))");
    assert!(r.is_err());
}

#[test]
fn arith_hex_octal() {
    let mut sh = shell();
    assert_eq!(sh.exec("echo $((0xFF))").unwrap().stdout, "255\n");
    assert_eq!(sh.exec("echo $((077))").unwrap().stdout, "63\n");
}

#[test]
fn arith_unary() {
    let mut sh = shell();
    assert_eq!(sh.exec("echo $((-5))").unwrap().stdout, "-5\n");
    assert_eq!(sh.exec("echo $((~0))").unwrap().stdout, "-1\n");
    assert_eq!(sh.exec("echo $((! 0))").unwrap().stdout, "1\n");
}

// ── let builtin ────────────────────────────────────────────────────

#[test]
fn let_builtin_basic() {
    let mut sh = shell();
    let r = sh.exec("let \"x = 5 + 3\"; echo $x").unwrap();
    assert_eq!(r.stdout, "8\n");
}

#[test]
fn let_exit_code_nonzero_result() {
    let mut sh = shell();
    let r = sh.exec("let \"5 + 3\"").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn let_exit_code_zero_result() {
    let mut sh = shell();
    let r = sh.exec("let \"0\"").unwrap();
    assert_eq!(r.exit_code, 1);
}

// ── (( )) compound command ─────────────────────────────────────────

#[test]
fn arith_command_basic() {
    let mut sh = shell();
    let r = sh.exec("x=5; (( x++ )); echo $x").unwrap();
    assert_eq!(r.stdout, "6\n");
}

#[test]
fn arith_command_exit_zero() {
    let mut sh = shell();
    let r = sh.exec("(( 0 ))").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn arith_command_exit_nonzero() {
    let mut sh = shell();
    let r = sh.exec("(( 1 ))").unwrap();
    assert_eq!(r.exit_code, 0);
}

// ── C-style for loop ──────────────────────────────────────────────

#[test]
fn arith_for_loop() {
    let mut sh = shell();
    let r = sh
        .exec("for (( i=0; i<5; i++ )); do echo $i; done")
        .unwrap();
    assert_eq!(r.stdout, "0\n1\n2\n3\n4\n");
}

#[test]
fn arith_for_loop_decrement() {
    let mut sh = shell();
    let r = sh
        .exec("for (( i=3; i>0; i-- )); do echo $i; done")
        .unwrap();
    assert_eq!(r.stdout, "3\n2\n1\n");
}

#[test]
fn arith_for_loop_step() {
    let mut sh = shell();
    let r = sh
        .exec("for (( i=0; i<10; i+=3 )); do echo $i; done")
        .unwrap();
    assert_eq!(r.stdout, "0\n3\n6\n9\n");
}

// ── 8. Functions & local variables ─────────────────────────────────

#[test]
fn function_define_and_call() {
    let mut sh = shell();
    let r = sh
        .exec("greet() { echo \"Hello $1\"; }; greet world")
        .unwrap();
    assert_eq!(r.stdout, "Hello world\n");
}

#[test]
fn function_positional_params() {
    let mut sh = shell();
    let r = sh.exec("f() { echo \"$1 $2 $#\"; }; f a b").unwrap();
    assert_eq!(r.stdout, "a b 2\n");
}

#[test]
fn function_local_variable_scoping() {
    let mut sh = shell();
    let r = sh
        .exec("X=global; f() { local X=local; echo $X; }; f; echo $X")
        .unwrap();
    assert_eq!(r.stdout, "local\nglobal\n");
}

#[test]
fn function_recursive_factorial() {
    let mut sh = shell();
    let r = sh.exec("fact() { if [ $1 -le 1 ]; then echo 1; return; fi; prev=$(fact $(($1 - 1))); echo $(($1 * prev)); }; fact 5").unwrap();
    assert_eq!(r.stdout, "120\n");
}

#[test]
fn function_return_exit_code() {
    let mut sh = shell();
    let r = sh.exec("f() { return 42; }; f; echo $?").unwrap();
    assert_eq!(r.stdout, "42\n");
}

#[test]
fn function_shadows_command() {
    let mut sh = shell();
    let r = sh
        .exec("cat() { echo \"custom cat\"; }; cat /dev/null")
        .unwrap();
    assert_eq!(r.stdout, "custom cat\n");
}

#[test]
fn function_dynamic_scoping() {
    let mut sh = shell();
    let r = sh.exec("X=outer; f() { echo $X; }; f").unwrap();
    assert_eq!(r.stdout, "outer\n");
}

#[test]
fn function_local_doesnt_leak() {
    let mut sh = shell();
    let r = sh
        .exec("f() { local Y=inner; }; f; echo \"${Y:-unset}\"")
        .unwrap();
    assert_eq!(r.stdout, "unset\n");
}

#[test]
fn function_nested_calls_with_locals() {
    let mut sh = shell();
    let r = sh.exec("outer() { local X=outer_val; inner; echo $X; }; inner() { local X=inner_val; echo $X; }; outer").unwrap();
    assert_eq!(r.stdout, "inner_val\nouter_val\n");
}

#[test]
fn function_return_no_args_uses_last_exit_code() {
    let mut sh = shell();
    let r = sh.exec("f() { false; return; }; f; echo $?").unwrap();
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn function_return_outside_function_error() {
    let mut sh = shell();
    let r = sh.exec("return 0").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("return"));
}

#[test]
fn function_positional_params_restored() {
    let mut sh = shell();
    let r = sh
        .exec("set -- a b c; f() { echo $1; }; f x; echo $1")
        .unwrap();
    assert_eq!(r.stdout, "x\na\n");
}

#[test]
fn function_keyword_syntax() {
    let mut sh = shell();
    let r = sh
        .exec("function greet { echo \"Hi $1\"; }; greet world")
        .unwrap();
    assert_eq!(r.stdout, "Hi world\n");
}

#[test]
fn function_multiple_locals() {
    let mut sh = shell();
    let r = sh
        .exec("A=1; B=2; f() { local A=10 B=20; echo $A $B; }; f; echo $A $B")
        .unwrap();
    assert_eq!(r.stdout, "10 20\n1 2\n");
}

#[test]
fn function_local_without_value() {
    let mut sh = shell();
    let r = sh
        .exec("X=global; f() { local X; echo \"${X}\"; }; f; echo $X")
        .unwrap();
    // local X without value keeps existing value empty string (bash behavior: variable exists but empty)
    assert_eq!(r.stdout, "\nglobal\n");
}

#[test]
fn function_return_in_loop() {
    let mut sh = shell();
    let r = sh.exec("f() { for i in 1 2 3; do if [ $i -eq 2 ]; then return 0; fi; echo $i; done; echo done; }; f; echo after").unwrap();
    assert_eq!(r.stdout, "1\nafter\n");
}

#[test]
fn function_call_depth_limit() {
    let mut sh = shell();
    let r = sh.exec("f() { f; }; f");
    assert!(r.is_err());
}

#[test]
fn function_modify_global_variable() {
    let mut sh = shell();
    let r = sh.exec("X=old; f() { X=new; }; f; echo $X").unwrap();
    assert_eq!(r.stdout, "new\n");
}

#[test]
fn function_dollar_hash_inside() {
    let mut sh = shell();
    let r = sh.exec("f() { echo $#; }; f a b c").unwrap();
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn function_dollar_at_inside() {
    let mut sh = shell();
    let r = sh
        .exec("f() { for x in \"$@\"; do echo $x; done; }; f hello world")
        .unwrap();
    assert_eq!(r.stdout, "hello\nworld\n");
}

#[test]
fn function_redefine() {
    let mut sh = shell();
    let r = sh
        .exec("f() { echo first; }; f; f() { echo second; }; f")
        .unwrap();
    assert_eq!(r.stdout, "first\nsecond\n");
}

#[test]
fn function_return_value_255() {
    let mut sh = shell();
    let r = sh.exec("f() { return 255; }; f; echo $?").unwrap();
    assert_eq!(r.stdout, "255\n");
}

// ── Case statement tests (Phase 9) ─────────────────────────────────────

#[test]
fn case_alternation() {
    let mut sh = shell();
    let r = sh.exec("case b in a|b|c) echo matched;; esac").unwrap();
    assert_eq!(r.stdout, "matched\n");
}

#[test]
fn case_fall_through() {
    let mut sh = shell();
    let r = sh
        .exec("case a in a) echo first;& b) echo second;; esac")
        .unwrap();
    assert_eq!(r.stdout, "first\nsecond\n");
}

#[test]
fn case_fall_through_chained() {
    let mut sh = shell();
    let r = sh
        .exec("case a in a) echo first;& b) echo second;& c) echo third;; esac")
        .unwrap();
    assert_eq!(r.stdout, "first\nsecond\nthird\n");
}

#[test]
fn case_continue_pattern_testing() {
    let mut sh = shell();
    let r = sh
        .exec("case abc in *a*) echo has_a;;& *b*) echo has_b;;& *c*) echo has_c;; esac")
        .unwrap();
    assert_eq!(r.stdout, "has_a\nhas_b\nhas_c\n");
}

#[test]
fn case_continue_skips_nonmatching() {
    let mut sh = shell();
    let r = sh
        .exec("case abc in *a*) echo has_a;;& *z*) echo has_z;;& *c*) echo has_c;; esac")
        .unwrap();
    assert_eq!(r.stdout, "has_a\nhas_c\n");
}

#[test]
fn case_glob_pattern() {
    let mut sh = shell();
    let r = sh
        .exec("case file.txt in *.txt) echo text;; *.md) echo markdown;; esac")
        .unwrap();
    assert_eq!(r.stdout, "text\n");
}

#[test]
fn case_empty() {
    let mut sh = shell();
    let r = sh.exec("case foo in esac").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn case_question_mark_glob() {
    let mut sh = shell();
    let r = sh
        .exec("case cat in ?at) echo matched;; *) echo no;; esac")
        .unwrap();
    assert_eq!(r.stdout, "matched\n");
}

#[test]
fn case_char_class() {
    let mut sh = shell();
    let r = sh
        .exec("case 3 in [0-9]) echo digit;; *) echo other;; esac")
        .unwrap();
    assert_eq!(r.stdout, "digit\n");
}

// ── Phase 10e: xargs and find ──────────────────────────────────────

#[test]
fn xargs_default_echo() {
    let mut sh = shell();
    let r = sh.exec("echo -e \"a\\nb\\nc\" | xargs echo").unwrap();
    assert_eq!(r.stdout, "a b c\n");
}

#[test]
fn xargs_replace_mode() {
    let mut sh = shell();
    let r = sh
        .exec("echo -e \"a\\nb\\nc\" | xargs -I {} echo \"item: {}\"")
        .unwrap();
    assert_eq!(r.stdout, "item: a\nitem: b\nitem: c\n");
}

#[test]
fn xargs_max_args() {
    let mut sh = shell();
    let r = sh
        .exec("echo -e \"1\\n2\\n3\" | xargs -n 1 echo \"num:\"")
        .unwrap();
    assert_eq!(r.stdout, "num: 1\nnum: 2\nnum: 3\n");
}

#[test]
fn xargs_with_pipeline_command() {
    let mut sh = shell();
    let r = sh.exec("echo -e \"hello\\nworld\" | xargs echo").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn find_lists_all_files() {
    let mut sh = shell();
    sh.exec("mkdir -p /d1/d2 && touch /d1/a.txt /d1/d2/b.txt")
        .unwrap();
    let r = sh.exec("find /d1").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/d1\n"));
    assert!(r.stdout.contains("/d1/a.txt"));
    assert!(r.stdout.contains("/d1/d2"));
    assert!(r.stdout.contains("/d1/d2/b.txt"));
}

#[test]
fn find_name_filter() {
    let mut sh = shell();
    sh.exec("touch /a.txt /b.md").unwrap();
    let r = sh.exec("find / -maxdepth 1 -name '*.txt'").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/a.txt"));
    assert!(!r.stdout.contains("/b.md"));
}

#[test]
fn find_type_d() {
    let mut sh = shell();
    sh.exec("mkdir -p /tdir/sub && touch /tdir/f.txt").unwrap();
    let r = sh.exec("find /tdir -type d").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/tdir\n"));
    assert!(r.stdout.contains("/tdir/sub"));
    assert!(!r.stdout.contains("f.txt"));
}

#[test]
fn find_maxdepth_one() {
    let mut sh = shell();
    sh.exec("mkdir -p /md/sub && touch /md/a.txt /md/sub/b.txt")
        .unwrap();
    let r = sh.exec("find /md -maxdepth 1").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/md\n"));
    assert!(r.stdout.contains("/md/a.txt"));
    assert!(r.stdout.contains("/md/sub"));
    assert!(!r.stdout.contains("/md/sub/b.txt"));
}

#[test]
fn find_exec_cat() {
    let mut sh = shell();
    sh.exec("mkdir /fe && echo hello > /fe/a.txt && echo world > /fe/b.txt")
        .unwrap();
    let r = sh.exec("find /fe -name '*.txt' -exec cat {} \\;").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("hello"));
    assert!(r.stdout.contains("world"));
}

#[test]
fn find_not_name() {
    let mut sh = shell();
    sh.exec("mkdir /fn && touch /fn/a.txt /fn/b.md /fn/c.txt")
        .unwrap();
    let r = sh.exec("find /fn -type f -not -name '*.txt'").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/fn/b.md"));
    assert!(!r.stdout.contains("/fn/a.txt"));
    assert!(!r.stdout.contains("/fn/c.txt"));
}

#[test]
fn find_empty_predicate() {
    let mut sh = shell();
    sh.exec("mkdir -p /emp/sub && touch /emp/empty.txt && echo data > /emp/full.txt")
        .unwrap();
    let r = sh.exec("find /emp -empty").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/emp/empty.txt"));
    assert!(r.stdout.contains("/emp/sub"));
    assert!(!r.stdout.contains("/emp/full.txt"));
}

#[test]
fn find_or_predicate() {
    let mut sh = shell();
    sh.exec("mkdir /fo && touch /fo/a.txt /fo/b.md /fo/c.rs")
        .unwrap();
    let r = sh.exec("find /fo -name '*.txt' -o -name '*.md'").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/fo/a.txt"));
    assert!(r.stdout.contains("/fo/b.md"));
    assert!(!r.stdout.contains("/fo/c.rs"));
}

#[test]
fn find_nonexistent_path() {
    let mut sh = shell();
    let r = sh.exec("find /no_such_dir").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("No such file or directory"));
}

#[test]
fn xargs_no_input() {
    let mut sh = shell();
    let r = sh.exec("echo -n '' | xargs echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn find_mindepth() {
    let mut sh = shell();
    sh.exec("mkdir -p /mdp/sub && touch /mdp/a.txt /mdp/sub/b.txt")
        .unwrap();
    let r = sh.exec("find /mdp -mindepth 2").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/mdp/sub/b.txt"));
    assert!(!r.stdout.contains("/mdp\n"));
    assert!(!r.stdout.contains("/mdp/a.txt"));
    assert!(!r.stdout.contains("/mdp/sub\n"));
}

#[test]
fn find_pipe_to_xargs() {
    let mut sh = shell();
    sh.exec("mkdir /px && echo hello > /px/a.txt && echo world > /px/b.txt")
        .unwrap();
    let r = sh.exec("find /px -name '*.txt' | xargs cat").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("hello"));
    assert!(r.stdout.contains("world"));
}

// ── trap builtin ────────────────────────────────────────────────────

#[test]
fn trap_exit_runs_at_end_of_exec() {
    let mut sh = shell();
    let r = sh.exec("trap 'echo goodbye' EXIT; echo hello").unwrap();
    assert_eq!(r.stdout, "hello\ngoodbye\n");
}

#[test]
fn trap_err_fires_on_false() {
    let mut sh = shell();
    let r = sh.exec("trap 'echo error' ERR; false").unwrap();
    assert_eq!(r.stdout, "error\n");
}

#[test]
fn trap_empty_ignores_signal() {
    let mut sh = shell();
    let r = sh.exec("trap '' EXIT; echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn trap_reset_removes_handler() {
    let mut sh = shell();
    sh.exec("trap 'echo cleanup' EXIT").unwrap();
    sh.exec("trap - EXIT").unwrap();
    let r = sh.exec("echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn trap_no_args_lists_traps() {
    let mut sh = shell();
    sh.exec("trap 'echo cleanup' EXIT").unwrap();
    let r = sh.exec("trap").unwrap();
    assert!(r.stdout.contains("EXIT"));
    assert!(r.stdout.contains("echo cleanup"));
}

#[test]
fn trap_cleanup_exit_shows_in_list() {
    let mut sh = shell();
    let r = sh.exec("trap 'echo cleanup' EXIT; trap").unwrap();
    assert!(r.stdout.contains("trap -- 'echo cleanup' EXIT"));
}

#[test]
fn trap_multiple_exit_and_err() {
    let mut sh = shell();
    let r = sh
        .exec("trap 'echo exit' EXIT; trap 'echo err' ERR; false; true")
        .unwrap();
    assert_eq!(r.stdout, "err\nexit\n");
}

#[test]
fn trap_err_does_not_fire_on_success() {
    let mut sh = shell();
    let r = sh.exec("trap 'echo err' ERR; true").unwrap();
    assert_eq!(r.stdout, "");
}

#[test]
fn trap_list_signals() {
    let mut sh = shell();
    let r = sh.exec("trap -l").unwrap();
    assert!(r.stdout.contains("SIGINT"));
    assert!(r.stdout.contains("SIGTERM"));
    assert!(r.stdout.contains("SIGEXIT"));
}

#[test]
fn trap_exit_persists_across_exec_calls() {
    let mut sh = shell();
    sh.exec("trap 'echo bye' EXIT").unwrap();
    let r = sh.exec("echo hi").unwrap();
    assert_eq!(r.stdout, "hi\nbye\n");
}

#[test]
fn trap_err_no_infinite_recursion() {
    let mut sh = shell();
    // ERR trap itself fails — must not recurse
    let r = sh.exec("trap 'false' ERR; false").unwrap();
    assert_eq!(r.exit_code, 1);
}

#[test]
fn trap_exit_with_variable() {
    let mut sh = shell();
    let r = sh
        .exec("X=world; trap 'echo goodbye $X' EXIT; echo hello")
        .unwrap();
    assert_eq!(r.stdout, "hello\ngoodbye world\n");
}

#[test]
fn trap_replace_handler() {
    let mut sh = shell();
    sh.exec("trap 'echo first' EXIT").unwrap();
    sh.exec("trap 'echo second' EXIT").unwrap();
    let r = sh.exec("echo hello").unwrap();
    assert_eq!(r.stdout, "hello\nsecond\n");
}

// ── Phase 11: set -e (errexit) ─────────────────────────────────────

#[test]
fn errexit_stops_on_failure() {
    let mut sh = shell();
    let r = sh.exec("set -e; false; echo should_not_appear").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn errexit_if_condition_exception() {
    let mut sh = shell();
    let r = sh
        .exec("set -e; if false; then echo no; fi; echo yes")
        .unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn errexit_or_left_side_exception() {
    let mut sh = shell();
    let r = sh.exec("set -e; false || true; echo yes").unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn errexit_negation_exception() {
    let mut sh = shell();
    let r = sh.exec("set -e; ! false; echo yes").unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn errexit_and_left_side_exception() {
    let mut sh = shell();
    let r = sh
        .exec("set -e; true && false; echo should_not_appear")
        .unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn errexit_while_condition_exception() {
    let mut sh = shell();
    let r = sh
        .exec("set -e; while false; do echo no; done; echo yes")
        .unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn errexit_until_condition_exception() {
    let mut sh = shell();
    let r = sh
        .exec("set -e; until true; do echo no; done; echo yes")
        .unwrap();
    assert_eq!(r.stdout, "yes\n");
}

// ── Phase 11: set -u (nounset) ─────────────────────────────────────

#[test]
fn nounset_error_on_unset_var() {
    let mut sh = shell();
    let r = sh.exec("set -u; echo $UNDEFINED_VAR");
    assert!(
        r.is_err() || {
            let r = r.unwrap();
            r.exit_code != 0 && r.stderr.contains("unbound variable")
        }
    );
}

#[test]
fn nounset_default_value_ok() {
    let mut sh = shell();
    let r = sh.exec("set -u; echo ${UNDEFINED_VAR:-default}").unwrap();
    assert_eq!(r.stdout, "default\n");
}

#[test]
fn nounset_special_var_hash_ok() {
    let mut sh = shell();
    let r = sh.exec("set -u; echo $#").unwrap();
    assert_eq!(r.stdout, "0\n");
}

#[test]
fn nounset_special_var_question_ok() {
    let mut sh = shell();
    let r = sh.exec("set -u; echo $?").unwrap();
    assert_eq!(r.stdout, "0\n");
}

#[test]
fn nounset_special_var_at_ok() {
    let mut sh = shell();
    let r = sh.exec("set -u; echo \"$@\"").unwrap();
    assert_eq!(r.stdout, "\n");
}

#[test]
fn nounset_set_var_ok() {
    let mut sh = shell();
    let r = sh.exec("set -u; X=hello; echo $X").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

// ── Phase 11: set -o pipefail ──────────────────────────────────────

#[test]
fn pipefail_false_pipe_true() {
    let mut sh = shell();
    let r = sh.exec("set -o pipefail; false | true; echo $?").unwrap();
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn pipefail_true_false_true() {
    let mut sh = shell();
    let r = sh
        .exec("set -o pipefail; true | false | true; echo $?")
        .unwrap();
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn pipefail_all_true() {
    let mut sh = shell();
    let r = sh.exec("set -o pipefail; true | true; echo $?").unwrap();
    assert_eq!(r.stdout, "0\n");
}

// ── Phase 11: combined set -eo pipefail ────────────────────────────

// ── Review fixes: regression tests ─────────────────────────────────

#[test]
fn utf8_substring_extraction() {
    let mut sh = shell();
    let r = sh.exec(r#"VAR="café"; echo "${VAR:2:2}""#).unwrap();
    assert_eq!(r.stdout, "fé\n");
}

#[test]
fn utf8_substring_negative_offset() {
    let mut sh = shell();
    let r = sh.exec(r#"VAR="héllo"; echo "${VAR: -3}""#).unwrap();
    assert_eq!(r.stdout, "llo\n");
}

#[test]
fn utf8_substring_no_length() {
    let mut sh = shell();
    let r = sh.exec(r#"VAR="日本語テスト"; echo "${VAR:2}""#).unwrap();
    assert_eq!(r.stdout, "語テスト\n");
}

#[test]
fn execution_limit_command_count() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_command_count: 3,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("echo a; echo b; echo c; echo d; echo e");
    assert!(result.is_err());
    let err = result.unwrap_err();
    match err {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_command_count",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_command_count), got: {other:?}"),
    }
}

#[test]
fn execution_limit_output_size() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_output_size: 10,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("echo 'this is a really long string that exceeds limit'");
    assert!(result.is_err());
    let err = result.unwrap_err();
    match err {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_output_size",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_output_size), got: {other:?}"),
    }
}

#[test]
fn err_trap_not_fired_in_if_condition() {
    let mut sh = shell();
    let r = sh
        .exec("trap 'echo err' ERR; if false; then echo no; fi; echo done")
        .unwrap();
    assert!(!r.stdout.contains("err"), "stdout was: {}", r.stdout);
    assert!(r.stdout.contains("done"));
}

#[test]
fn err_trap_not_fired_in_and_or_chain() {
    let mut sh = shell();
    let r = sh
        .exec("trap 'echo err' ERR; false || true; echo done")
        .unwrap();
    assert!(!r.stdout.contains("err"), "stdout was: {}", r.stdout);
    assert!(r.stdout.contains("done"));
}

#[test]
fn err_trap_fires_on_plain_failure() {
    let mut sh = shell();
    let r = sh.exec("trap 'echo err' ERR; false; echo done").unwrap();
    assert!(r.stdout.contains("err"), "stdout was: {}", r.stdout);
    assert!(r.stdout.contains("done"));
}

#[test]
fn glob_question_mark_utf8() {
    let mut sh = shell();
    sh.exec("mkdir -p /tmp").unwrap();
    sh.exec("echo x > /tmp/café && echo x > /tmp/caff").unwrap();
    sh.exec("cd /tmp").unwrap();
    let r = sh.exec("echo caf?").unwrap();
    // `?` should match one character — both `é` (2 bytes) and `f` (1 byte)
    assert!(r.stdout.contains("café"), "stdout was: {}", r.stdout);
    assert!(r.stdout.contains("caff"), "stdout was: {}", r.stdout);
}

#[test]
fn redirect_no_auto_mkdir() {
    let mut sh = shell();
    let r = sh.exec("echo x > /nonexistent/dir/file");
    assert!(r.is_err() || r.unwrap().exit_code != 0);
}

#[test]
fn errexit_pipefail_combined() {
    let mut sh = shell();
    let r = sh
        .exec("set -eo pipefail; false | true; echo should_not_appear")
        .unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 1);
}

// ── M2 Cross-command pipeline integration tests ───────────────────

#[test]
fn pipeline_grep_recursive_wc() {
    let mut sh = shell();
    sh.exec("mkdir -p /src").unwrap();
    sh.exec("echo 'TODO fix this' > /src/main.rs").unwrap();
    sh.exec("echo 'no match here' > /src/lib.rs").unwrap();
    sh.exec("echo 'another TODO item\nand TODO again' > /src/util.rs")
        .unwrap();
    let r = sh.exec("grep -r 'TODO' /src | wc -l").unwrap();
    assert_eq!(r.stdout.trim(), "3");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn pipeline_csv_column_frequency() {
    let mut sh = shell();
    sh.exec("echo 'name,dept\nalice,eng\nbob,sales\ncarol,eng\ndave,eng\neve,sales' > /data.csv")
        .unwrap();
    let r = sh
        .exec("cat /data.csv | awk -F, '{print $2}' | sort | uniq -c | sed 's/^ *//' | sort -rn")
        .unwrap();
    let lines: Vec<&str> = r.stdout.trim().lines().collect();
    // dept column values include the header "dept" (1x), "eng" (3x), "sales" (2x)
    assert!(
        lines[0].contains("eng"),
        "eng should be most frequent, got: {:?}",
        lines
    );
    assert!(
        lines[0].starts_with('3'),
        "eng count should be 3, got: {:?}",
        lines
    );
    assert_eq!(r.exit_code, 0);
}

#[test]
fn pipeline_jq_sort() {
    let mut sh = shell();
    let r = sh
        .exec(r#"echo '{"users":[{"name":"charlie"},{"name":"alice"},{"name":"bob"}]}' | jq '.users[].name' | sort"#)
        .unwrap();
    let lines: Vec<&str> = r.stdout.trim().lines().collect();
    assert_eq!(lines, vec![r#""alice""#, r#""bob""#, r#""charlie""#]);
    assert_eq!(r.exit_code, 0);
}

#[test]
fn pipeline_jq_raw_sort() {
    let mut sh = shell();
    let r = sh
        .exec(r#"echo '{"users":[{"name":"charlie"},{"name":"alice"},{"name":"bob"}]}' | jq -r '.users[].name' | sort"#)
        .unwrap();
    assert_eq!(r.stdout, "alice\nbob\ncharlie\n");
}

#[test]
fn pipeline_sed_grep_count() {
    let mut sh = shell();
    sh.exec("echo 'old value old stuff\nkeep this\nold again' > /input.txt")
        .unwrap();
    let r = sh
        .exec("sed 's/old/new/g' /input.txt | grep -c new")
        .unwrap();
    assert_eq!(r.stdout.trim(), "2");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn pipeline_awk_sort_join() {
    let mut sh = shell();
    sh.exec("echo 'alice 100\nbob 200\nalice 50\ncarol 300' > /data")
        .unwrap();
    sh.exec("echo 'alice admin\nbob user\ncarol admin' > /reference.txt")
        .unwrap();
    let r = sh
        .exec("awk '{print $1}' /data | sort -u | join - /reference.txt")
        .unwrap();
    let lines: Vec<&str> = r.stdout.trim().lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(r.stdout.contains("alice admin"));
    assert!(r.stdout.contains("bob user"));
    assert!(r.stdout.contains("carol admin"));
}

#[test]
fn pipeline_comm_common_lines() {
    let mut sh = shell();
    sh.exec("echo 'alpha\nbravo\ncharlie\ndelta' > /sorted1")
        .unwrap();
    sh.exec("echo 'bravo\ncharlie\necho\nfoxtrot' > /sorted2")
        .unwrap();
    let r = sh.exec("comm -12 /sorted1 /sorted2").unwrap();
    assert_eq!(r.stdout, "bravo\ncharlie\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn pipeline_diff_two_files() {
    let mut sh = shell();
    sh.exec("echo 'line1\nline2\nline3' > /file1").unwrap();
    sh.exec("echo 'line1\nchanged\nline3' > /file2").unwrap();
    let r = sh.exec("diff /file1 /file2").unwrap();
    assert!(r.stdout.contains("line2"));
    assert!(r.stdout.contains("changed"));
    assert_ne!(r.exit_code, 0); // diff returns 1 when files differ
}

#[test]
fn pipeline_diff_unified_format() {
    let mut sh = shell();
    sh.exec("echo 'aaa\nbbb\nccc' > /a.txt").unwrap();
    sh.exec("echo 'aaa\nxxx\nccc' > /b.txt").unwrap();
    let r = sh.exec("diff -u /a.txt /b.txt").unwrap();
    assert!(r.stdout.contains("---"));
    assert!(r.stdout.contains("+++"));
    assert!(r.stdout.contains("-bbb"));
    assert!(r.stdout.contains("+xxx"));
}

#[test]
fn pipeline_tac_reverse_lines() {
    let mut sh = shell();
    sh.exec("echo 'first\nsecond\nthird' > /lines.txt").unwrap();
    let r = sh.exec("tac /lines.txt").unwrap();
    assert_eq!(r.stdout, "third\nsecond\nfirst\n");
}

#[test]
fn pipeline_tac_pipe_grep() {
    let mut sh = shell();
    let r = sh
        .exec("echo 'aaa\nbbb\nccc\nddd' | tac | grep -n '.'")
        .unwrap();
    let lines: Vec<&str> = r.stdout.trim().lines().collect();
    assert_eq!(lines[0], "1:ddd");
    assert_eq!(lines[3], "4:aaa");
}

#[test]
fn pipeline_sed_awk_combined() {
    let mut sh = shell();
    sh.exec("echo 'name: Alice\nage: 30\nname: Bob\nage: 25' > /info.txt")
        .unwrap();
    let r = sh
        .exec("grep 'name' /info.txt | sed 's/name: //' | awk '{print NR, $0}'")
        .unwrap();
    assert_eq!(r.stdout, "1 Alice\n2 Bob\n");
}

#[test]
fn pipeline_echo_jq_create_and_filter() {
    let mut sh = shell();
    let r = sh
        .exec(r#"echo '{"a":1,"b":2,"c":3}' | jq -r 'keys[]' | sort"#)
        .unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

#[test]
fn pipeline_awk_sum_column() {
    let mut sh = shell();
    sh.exec("echo '10\n20\n30\n40' > /nums.txt").unwrap();
    let r = sh
        .exec("cat /nums.txt | awk '{sum += $1} END {print sum}'")
        .unwrap();
    assert_eq!(r.stdout.trim(), "100");
}

#[test]
fn pipeline_grep_sed_awk_chain() {
    let mut sh = shell();
    sh.exec("echo 'ERROR: disk full\nINFO: ok\nERROR: timeout\nWARN: slow\nERROR: oom' > /log.txt")
        .unwrap();
    let r = sh
        .exec("grep 'ERROR' /log.txt | sed 's/ERROR: //' | awk '{print NR\": \"$0}'")
        .unwrap();
    assert_eq!(r.stdout, "1: disk full\n2: timeout\n3: oom\n");
}

#[test]
fn pipeline_sort_uniq_head() {
    let mut sh = shell();
    sh.exec("echo 'banana\napple\ncherry\napple\nbanana\napple' > /fruits.txt")
        .unwrap();
    let r = sh
        .exec("sort /fruits.txt | uniq -c | sed 's/^ *//' | sort -rn | head -1")
        .unwrap();
    assert!(
        r.stdout.trim().contains("apple"),
        "apple should be most frequent, got: {}",
        r.stdout
    );
    assert!(
        r.stdout.trim().starts_with('3'),
        "apple count should be 3, got: {}",
        r.stdout
    );
}

#[test]
fn pipeline_expand_unexpand_roundtrip() {
    let mut sh = shell();
    sh.exec("printf 'col1\\tcol2\\tcol3\\n' > /tabs.txt")
        .unwrap();
    let r = sh.exec("expand /tabs.txt | unexpand -a").unwrap();
    assert!(r.stdout.contains('\t'), "should re-create tabs");
    assert!(r.stdout.contains("col1"), "should preserve col1");
    assert!(r.stdout.contains("col3"), "should preserve col3");
}

#[test]
fn pipeline_column_table_formatting() {
    let mut sh = shell();
    sh.exec("echo 'name:age:city\nalice:30:ny\nbob:25:sf' > /data.csv")
        .unwrap();
    let r = sh.exec("column -t -s ':' /data.csv").unwrap();
    assert!(r.stdout.contains("name"));
    assert!(r.stdout.contains("alice"));
    // Column should align output
    let lines: Vec<&str> = r.stdout.trim().lines().collect();
    assert_eq!(lines.len(), 3);
}

#[test]
fn pipeline_fmt_wrapping() {
    let mut sh = shell();
    sh.exec("echo 'this is a very long line that should be wrapped by the fmt command to a reasonable width for display' > /long.txt").unwrap();
    let r = sh.exec("fmt -w 40 /long.txt").unwrap();
    for line in r.stdout.lines() {
        assert!(
            line.len() <= 40,
            "line too long: {} chars: {}",
            line.len(),
            line
        );
    }
}

#[test]
fn pipeline_comm_only_unique_to_first() {
    let mut sh = shell();
    sh.exec("echo 'a\nb\nc\nd' > /s1").unwrap();
    sh.exec("echo 'b\nc\ne\nf' > /s2").unwrap();
    let r = sh.exec("comm -23 /s1 /s2").unwrap();
    assert_eq!(r.stdout, "a\nd\n");
}

#[test]
fn pipeline_join_custom_field() {
    let mut sh = shell();
    sh.exec("echo '1 Alice\n2 Bob\n3 Carol' > /names.txt")
        .unwrap();
    sh.exec("echo '1 Engineering\n2 Sales\n3 Marketing' > /depts.txt")
        .unwrap();
    let r = sh.exec("join /names.txt /depts.txt").unwrap();
    assert!(r.stdout.contains("1 Alice Engineering"));
    assert!(r.stdout.contains("2 Bob Sales"));
    assert!(r.stdout.contains("3 Carol Marketing"));
}

#[test]
fn pipeline_sed_in_place_then_cat() {
    let mut sh = shell();
    sh.exec("echo 'hello world' > /greet.txt").unwrap();
    sh.exec("sed -i 's/world/rust/' /greet.txt").unwrap();
    let r = sh.exec("cat /greet.txt").unwrap();
    assert_eq!(r.stdout, "hello rust\n");
}

#[test]
fn pipeline_jq_nested_select() {
    let mut sh = shell();
    let r = sh
        .exec(r#"echo '[{"name":"alice","age":30},{"name":"bob","age":17},{"name":"carol","age":25}]' | jq -r '[.[] | select(.age >= 18)] | .[].name'"#)
        .unwrap();
    let lines: Vec<&str> = r.stdout.trim().lines().collect();
    assert_eq!(lines, vec!["alice", "carol"]);
}

#[test]
fn pipeline_multi_stage_text_processing() {
    let mut sh = shell();
    sh.exec(
        "echo 'alice:eng:100\nbob:sales:200\ncarol:eng:150\ndave:sales:300\neve:eng:250' > /employees.csv",
    )
    .unwrap();
    // Get engineering department totals
    let r = sh
        .exec("grep 'eng' /employees.csv | awk -F: '{sum += $3} END {print sum}'")
        .unwrap();
    assert_eq!(r.stdout.trim(), "500");
}

#[test]
fn pipeline_diff_identical_files() {
    let mut sh = shell();
    sh.exec("echo 'same content' > /f1").unwrap();
    sh.exec("echo 'same content' > /f2").unwrap();
    let r = sh.exec("diff /f1 /f2").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

// ── Limit enforcement integration tests ──────────────────────────

#[test]
fn limit_max_command_count_exceeded_in_loop() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_command_count: 10,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("for i in $(seq 1 20); do echo $i; done");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_command_count",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_command_count), got: {other:?}"),
    }

    // Shell remains usable after limit error
    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_loop_iterations_exceeded() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_loop_iterations: 100,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("while true; do :; done");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_loop_iterations",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_loop_iterations), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_call_depth_exceeded_recursive_function() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_call_depth: 5,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("f() { f; }; f");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_call_depth",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_call_depth), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_execution_time_exceeded() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};
    use std::time::Duration;

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_execution_time: Duration::from_millis(100),
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("sleep 999");
    assert!(result.is_err());
    // sleep caps to max_execution_time, then the next check_limits catches timeout
    match result.unwrap_err() {
        rust_bash::RustBashError::Timeout => {}
        other => panic!("expected Timeout, got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_output_size_exceeded_in_pipeline() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_output_size: 1024,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("yes | head -n 100000");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_output_size",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_output_size), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_string_length_exceeded_in_variable() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_string_length: 1024,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec(r#"x=""; for i in $(seq 1 1000); do x="${x}aaaa"; done"#);
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_string_length",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_string_length), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_substitution_depth_exceeded() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_substitution_depth: 2,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("echo $(echo $(echo $(echo x)))");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_substitution_depth",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_substitution_depth), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_heredoc_size_exceeded() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_heredoc_size: 100,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    // Generate a heredoc body larger than 100 bytes
    let big_body = "A".repeat(200);
    let script = format!("cat <<EOF\n{big_body}\nEOF");
    let result = sh.exec(&script);
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_heredoc_size",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_heredoc_size), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_glob_results_exceeded() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_glob_results: 5,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    // Create more files than the glob limit allows
    sh.exec("mkdir /globdir && cd /globdir").unwrap();
    sh.exec("for i in $(seq 1 10); do echo x > /globdir/file$i; done")
        .unwrap();

    let result = sh.exec("echo /globdir/*");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_glob_results",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_glob_results), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_brace_expansion_exceeded() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_brace_expansion: 100,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec("echo {1..10000}");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_brace_expansion",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_brace_expansion), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_subshell_command_counts_accumulate() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_command_count: 50,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    // Each subshell runs 20 echo commands; 3 iterations × 20 = 60 subshell commands
    // plus loop overhead, exceeding the limit of 50 because counts accumulate
    let result = sh.exec(
        "for i in 1 2 3; do \
            echo $(echo 1; echo 2; echo 3; echo 4; echo 5; \
                   echo 6; echo 7; echo 8; echo 9; echo 10; \
                   echo 11; echo 12; echo 13; echo 14; echo 15; \
                   echo 16; echo 17; echo 18; echo 19; echo 20); \
         done",
    );
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_command_count",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_command_count), got: {other:?}"),
    }
}

#[test]
fn limit_source_increments_call_depth() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_call_depth: 3,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    // Create a chain of source files that exceed call depth
    sh.exec("echo 'source /b.sh' > /a.sh").unwrap();
    sh.exec("echo 'source /c.sh' > /b.sh").unwrap();
    sh.exec("echo 'source /d.sh' > /c.sh").unwrap();
    sh.exec("echo 'echo deep' > /d.sh").unwrap();

    let result = sh.exec("source /a.sh");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_call_depth",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_call_depth), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_eval_increments_call_depth() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_call_depth: 2,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    let result = sh.exec(r#"eval 'eval "eval \"echo done\""'"#);
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_call_depth",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_call_depth), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_counters_reset_between_exec_calls() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_command_count: 50,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    // First exec uses many commands but stays under the limit
    sh.exec("for i in $(seq 1 10); do echo $i; done").unwrap();

    // Second exec should succeed because counters reset
    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_max_string_length_in_read_builtin() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_string_length: 100,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    // Create a string larger than the limit, then pipe it to read
    let big = "x".repeat(200);
    let script = format!("echo '{big}' | read var");
    let result = sh.exec(&script);
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_string_length",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_string_length), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn limit_here_string_size_checked() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_heredoc_size: 50,
            ..ExecutionLimits::default()
        })
        .build()
        .unwrap();

    // Build a variable larger than the heredoc limit, then use here-string
    let big = "B".repeat(100);
    sh.exec(&format!("HUGE='{big}'")).unwrap();

    let result = sh.exec("cat <<<$HUGE");
    assert!(result.is_err());
    match result.unwrap_err() {
        rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_heredoc_size",
            ..
        } => {}
        other => panic!("expected LimitExceeded(max_heredoc_size), got: {other:?}"),
    }

    let r = sh.exec("echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

// ── Network policy integration tests ─────────────────────────────

#[test]
fn network_disabled_by_default_curl_errors() {
    let mut sh = shell();
    let r = sh.exec("curl https://example.com").unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(
        r.stderr.contains("network access is disabled"),
        "expected network disabled error, got stderr: {}",
        r.stderr,
    );
}

#[test]
fn network_enabled_but_url_not_in_allowlist() {
    use rust_bash::{NetworkPolicy, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .network_policy(NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://allowed.example.com/".to_string()],
            ..Default::default()
        })
        .build()
        .unwrap();

    let r = sh.exec("curl https://evil.example.com/data").unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(
        r.stderr.contains("not allowed by network policy"),
        "expected URL rejection, got stderr: {}",
        r.stderr,
    );
}

#[test]
fn network_url_normalization_attack_rejected() {
    use rust_bash::{NetworkPolicy, RustBashBuilder};

    let mut sh = RustBashBuilder::new()
        .network_policy(NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            ..Default::default()
        })
        .build()
        .unwrap();

    // Userinfo attack: the @evil.com domain should be caught
    let r = sh.exec("curl https://api.example.com@evil.com/").unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(
        r.stderr.contains("not allowed by network policy"),
        "expected URL rejection for normalization attack, got stderr: {}",
        r.stderr,
    );
}

#[test]
fn network_method_restriction_rejects_disallowed() {
    use rust_bash::{NetworkPolicy, RustBashBuilder};
    use std::collections::HashSet;

    let mut sh = RustBashBuilder::new()
        .network_policy(NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            allowed_methods: HashSet::from(["GET".to_string()]),
            ..Default::default()
        })
        .build()
        .unwrap();

    // POST should be rejected when only GET is allowed
    let r = sh
        .exec("curl -X POST https://api.example.com/data")
        .unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(
        r.stderr.contains("method not allowed"),
        "expected method rejection, got stderr: {}",
        r.stderr,
    );
}
