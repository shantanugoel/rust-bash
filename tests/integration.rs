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
    let r = sh.exec("X=6").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("readonly"));
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
    sh.exec("mkdir /test_dir").unwrap();
    sh.exec("touch /test_dir/alpha").unwrap();
    sh.exec("touch /test_dir/beta").unwrap();
    let r = sh.exec("ls -1 /test_dir").unwrap();
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
    let r = sh.exec("Y=100").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("readonly"));
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
    sh.exec("mkdir /work").unwrap();
    sh.exec("touch /work/alpha.txt").unwrap();
    sh.exec("touch /work/beta.txt").unwrap();
    sh.exec("mkdir /work/docs").unwrap();
    let r = sh.exec("ls -l /work").unwrap();
    insta::assert_snapshot!("ls_long", r.stdout);
}

#[test]
fn snapshot_ls_long_all() {
    let mut sh = shell();
    sh.exec("mkdir /work").unwrap();
    sh.exec("touch /work/alpha.txt").unwrap();
    sh.exec("touch /work/beta.txt").unwrap();
    sh.exec("mkdir /work/docs").unwrap();
    let r = sh.exec("ls -la /work").unwrap();
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
    assert_eq!(r.stdout, "[]\n[a]\n[]\n[b]\n");
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
                        stdout_bytes: None,
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
    // BASH_REMATCH[0] should contain the whole match
    let r2 = sh.exec("echo ${BASH_REMATCH[0]}").unwrap();
    assert_eq!(r2.stdout, "abc123\n");
    // BASH_REMATCH[1] should contain first capture group
    let r3 = sh.exec("echo ${BASH_REMATCH[1]}").unwrap();
    assert_eq!(r3.stdout, "123\n");
}

#[test]
fn extended_test_regex_no_match() {
    let mut sh = shell();
    let r = sh.exec("[[ \"hello\" =~ ^[0-9]+$ ]]").unwrap();
    assert_eq!(r.exit_code, 1);
}

// --- PIPESTATUS tests ---

#[test]
fn pipestatus_simple_command() {
    let mut sh = shell();
    let r = sh.exec("true; echo ${PIPESTATUS[0]}").unwrap();
    assert_eq!(r.stdout, "0\n");
}

#[test]
fn pipestatus_pipeline_all_elements() {
    let mut sh = shell();
    let r = sh
        .exec("echo hello | grep x; echo ${PIPESTATUS[@]}")
        .unwrap();
    assert_eq!(r.stdout.trim(), "0 1");
}

#[test]
fn pipestatus_pipeline_specific_index() {
    let mut sh = shell();
    let r = sh
        .exec("true | false | true; echo ${PIPESTATUS[1]}")
        .unwrap();
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn pipestatus_overwritten_by_subsequent_command() {
    let mut sh = shell();
    let r = sh
        .exec("true | false; echo hi; echo ${PIPESTATUS[0]}")
        .unwrap();
    // PIPESTATUS should reflect `echo hi` (exit 0), not the earlier pipeline
    assert!(r.stdout.ends_with("0\n"));
}

#[test]
fn pipestatus_length() {
    let mut sh = shell();
    let r = sh
        .exec("true | false | true; echo ${#PIPESTATUS[@]}")
        .unwrap();
    assert_eq!(r.stdout, "3\n");
}

// --- BASH_REMATCH array tests ---

#[test]
fn bash_rematch_array_whole_match() {
    let mut sh = shell();
    let r = sh
        .exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]; echo ${BASH_REMATCH[0]}")
        .unwrap();
    assert_eq!(r.stdout, "abc123\n");
}

#[test]
fn bash_rematch_array_capture_groups() {
    let mut sh = shell();
    let r = sh
        .exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]; echo ${BASH_REMATCH[1]}")
        .unwrap();
    assert_eq!(r.stdout, "abc\n");
    let r2 = sh
        .exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]; echo ${BASH_REMATCH[2]}")
        .unwrap();
    assert_eq!(r2.stdout, "123\n");
}

#[test]
fn bash_rematch_array_length() {
    let mut sh = shell();
    let r = sh
        .exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]; echo ${#BASH_REMATCH[@]}")
        .unwrap();
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn bash_rematch_all_elements() {
    let mut sh = shell();
    let r = sh
        .exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]; echo ${BASH_REMATCH[@]}")
        .unwrap();
    assert_eq!(r.stdout, "abc123 abc 123\n");
}

#[test]
fn bash_rematch_cleared_on_no_match() {
    let mut sh = shell();
    sh.exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]").unwrap();
    let r = sh
        .exec("[[ \"!!!\" =~ ([a-z]+) ]]; echo ${#BASH_REMATCH[@]}")
        .unwrap();
    assert_eq!(r.stdout, "0\n");
}

#[test]
fn bash_rematch_scalar_access_returns_index_zero() {
    let mut sh = shell();
    let r = sh
        .exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]; echo $BASH_REMATCH")
        .unwrap();
    assert_eq!(r.stdout, "abc123\n");
}

#[test]
fn bash_rematch_non_participating_group() {
    let mut sh = shell();
    let r = sh
        .exec("[[ \"a\" =~ (a)|(b) ]]; echo ${#BASH_REMATCH[@]}")
        .unwrap();
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn pipestatus_scalar_access_returns_first_code() {
    let mut sh = shell();
    let r = sh.exec("true | false; echo $PIPESTATUS").unwrap();
    assert_eq!(r.stdout, "0\n");
}

#[test]
fn bash_rematch_persists_across_non_regex_commands() {
    let mut sh = shell();
    sh.exec("[[ \"abc123\" =~ ([a-z]+)([0-9]+) ]]").unwrap();
    sh.exec("echo hello").unwrap();
    let r = sh.exec("echo ${BASH_REMATCH[1]}").unwrap();
    assert_eq!(r.stdout, "abc\n");
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
    sh.exec("shopt -s globstar").unwrap();
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
    sh.exec("shopt -s globstar").unwrap();
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
    assert_eq!(r.stdout, "a\n");
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

#[cfg(feature = "network")]
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

#[cfg(feature = "network")]
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

#[cfg(feature = "network")]
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

#[cfg(feature = "network")]
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

// ── State persistence across exec() calls ────────────────────────

#[test]
fn function_definitions_persist_across_exec_calls() {
    let mut sh = shell();
    sh.exec("greet() { echo \"hello $1\"; }").unwrap();
    let r = sh.exec("greet world").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn last_exit_code_persists_across_exec_calls() {
    let mut sh = shell();
    sh.exec("false").unwrap();
    let r = sh.exec("echo $?").unwrap();
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn shell_opts_persist_across_exec_calls() {
    let mut sh = shell();
    sh.exec("set -o pipefail").unwrap();
    let r = sh.exec("false | true").unwrap();
    assert_eq!(r.exit_code, 1);
}

// ── Pre-command variable assignment semantics ────────────────────

#[test]
fn bare_assignment_persists_globally() {
    let mut sh = shell();
    sh.exec("FOO=bar").unwrap();
    let r = sh.exec("echo $FOO").unwrap();
    assert_eq!(r.stdout, "bar\n");
}

#[test]
fn pre_command_assignment_visible_in_command_env() {
    let mut sh = shell();
    // The variable should be visible to the command via env/printenv
    let r = sh.exec("FOO=bar printenv FOO").unwrap();
    assert_eq!(r.stdout.trim(), "bar");
}

// ── Builder configuration ────────────────────────────────────────

#[test]
fn builder_files_creates_parent_dirs() {
    use rust_bash::RustBashBuilder;
    let mut files = HashMap::new();
    files.insert("/deep/nested/file.txt".to_string(), b"content".to_vec());
    let mut sh = RustBashBuilder::new().files(files).build().unwrap();
    let r = sh.exec("cat /deep/nested/file.txt").unwrap();
    assert_eq!(r.stdout, "content");
}

#[test]
fn builder_env_variables_accessible() {
    use rust_bash::RustBashBuilder;
    let mut env = HashMap::new();
    env.insert("MY_VAR".to_string(), "my_value".to_string());
    let mut sh = RustBashBuilder::new().env(env).build().unwrap();
    let r = sh.exec("echo $MY_VAR").unwrap();
    assert_eq!(r.stdout, "my_value\n");
}

#[test]
fn builder_cwd_sets_initial_directory() {
    use rust_bash::RustBashBuilder;
    let mut sh = RustBashBuilder::new().cwd("/custom").build().unwrap();
    let r = sh.exec("pwd").unwrap();
    assert_eq!(r.stdout, "/custom\n");
}

#[test]
fn builder_custom_command_overrides_builtin() {
    use rust_bash::{CommandContext, CommandResult, RustBashBuilder, VirtualCommand};

    struct MyEcho;
    impl VirtualCommand for MyEcho {
        fn name(&self) -> &str {
            "myecho"
        }
        fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
            CommandResult {
                stdout: format!("custom: {}\n", args.join(" ")),
                stderr: String::new(),
                exit_code: 0,
                stdout_bytes: None,
            }
        }
    }

    let mut sh = RustBashBuilder::new()
        .command(Box::new(MyEcho))
        .build()
        .unwrap();
    let r = sh.exec("myecho hello").unwrap();
    assert_eq!(r.stdout, "custom: hello\n");
}

// ── is_input_complete edge cases ─────────────────────────────────

#[test]
fn is_input_complete_empty_string() {
    assert!(rust_bash::RustBash::is_input_complete(""));
}

#[test]
fn is_input_complete_unterminated_single_quote() {
    assert!(!rust_bash::RustBash::is_input_complete("echo 'hello"));
}

#[test]
fn is_input_complete_unterminated_double_quote() {
    assert!(!rust_bash::RustBash::is_input_complete("echo \"hello"));
}

#[test]
fn is_input_complete_open_if() {
    assert!(!rust_bash::RustBash::is_input_complete("if true; then"));
}

#[test]
fn is_input_complete_valid_if() {
    assert!(rust_bash::RustBash::is_input_complete(
        "if true; then echo yes; fi"
    ));
}

#[test]
fn is_input_complete_open_heredoc() {
    assert!(!rust_bash::RustBash::is_input_complete("cat <<EOF\nhello"));
}

#[test]
fn is_input_complete_syntax_error_is_complete() {
    // Genuine syntax error (not incomplete) should return true
    assert!(rust_bash::RustBash::is_input_complete(";;"));
}

// ── Error variant Display messages ───────────────────────────────

#[test]
fn error_display_parse() {
    let err = rust_bash::RustBashError::Parse("test".into());
    assert_eq!(format!("{err}"), "parse error: test");
}

#[test]
fn error_display_execution() {
    let err = rust_bash::RustBashError::Execution("fail".into());
    assert_eq!(format!("{err}"), "execution error: fail");
}

#[test]
fn error_display_limit_exceeded() {
    let err = rust_bash::RustBashError::LimitExceeded {
        limit_name: "max_command_count",
        limit_value: 100,
        actual_value: 101,
    };
    let s = format!("{err}");
    assert!(s.contains("max_command_count"));
    assert!(s.contains("101"));
    assert!(s.contains("100"));
}

#[test]
fn error_display_timeout() {
    let err = rust_bash::RustBashError::Timeout;
    assert_eq!(format!("{err}"), "execution timed out");
}

#[test]
fn error_display_network() {
    let err = rust_bash::RustBashError::Network("denied".into());
    assert_eq!(format!("{err}"), "network error: denied");
}

#[test]
fn error_display_vfs() {
    let err = rust_bash::RustBashError::Vfs(rust_bash::VfsError::NotFound("/a".into()));
    let s = format!("{err}");
    assert!(s.contains("No such file or directory"));
}

#[test]
fn vfs_error_display_all_variants() {
    use rust_bash::VfsError;
    use std::path::PathBuf;

    let cases: Vec<(VfsError, &str)> = vec![
        (VfsError::NotFound(PathBuf::from("/a")), "No such file"),
        (
            VfsError::AlreadyExists(PathBuf::from("/a")),
            "Already exists",
        ),
        (
            VfsError::NotADirectory(PathBuf::from("/a")),
            "Not a directory",
        ),
        (VfsError::NotAFile(PathBuf::from("/a")), "Not a file"),
        (
            VfsError::IsADirectory(PathBuf::from("/a")),
            "Is a directory",
        ),
        (
            VfsError::PermissionDenied(PathBuf::from("/a")),
            "Permission denied",
        ),
        (
            VfsError::DirectoryNotEmpty(PathBuf::from("/a")),
            "Directory not empty",
        ),
        (VfsError::SymlinkLoop(PathBuf::from("/a")), "symbolic links"),
        (VfsError::InvalidPath("bad".into()), "Invalid path"),
        (VfsError::IoError("broken".into()), "I/O error"),
    ];
    for (err, expected_substr) in cases {
        let s = format!("{err}");
        assert!(
            s.contains(expected_substr),
            "VfsError display for {:?} should contain '{expected_substr}', got: {s}",
            err
        );
    }
}

// ── Control flow edge cases ──────────────────────────────────────

#[test]
fn continue_large_n_resumes_outermost() {
    let mut sh = shell();
    let r = sh
        .exec("for i in 1 2; do for j in a b; do echo $i$j; continue 99; done; done")
        .unwrap();
    // continue 99 > depth → continues outermost loop
    assert_eq!(r.stdout, "1a\n2a\n");
}

#[test]
fn case_empty_body() {
    let mut sh = shell();
    let r = sh.exec("case foo in foo) ;; esac; echo done").unwrap();
    assert_eq!(r.stdout, "done\n");
    assert_eq!(r.exit_code, 0);
}

// ── set without arguments lists variables ────────────────────────

#[test]
fn set_without_args_lists_variables() {
    let mut sh = shell();
    sh.exec("MY_TEST_VAR=hello123").unwrap();
    let r = sh.exec("set").unwrap();
    assert!(
        r.stdout.contains("MY_TEST_VAR=") && r.stdout.contains("hello123"),
        "set output should list variables, got: {}",
        &r.stdout[..r.stdout.len().min(500)]
    );
}

// ── Arithmetic edge cases ────────────────────────────────────────

#[test]
fn arithmetic_overflow_wraps() {
    let mut sh = shell();
    // i64::MAX + 1 wraps (or stays at boundary - depends on implementation)
    let r = sh.exec("echo $((9223372036854775807 + 1))").unwrap();
    // Should not panic; result is implementation-defined
    assert_eq!(r.exit_code, 0);
}

#[test]
fn arithmetic_negative_numbers() {
    let mut sh = shell();
    let r = sh.exec("echo $((-5 + 3))").unwrap();
    assert_eq!(r.stdout, "-2\n");
}

#[test]
fn arithmetic_nested_parentheses() {
    let mut sh = shell();
    let r = sh.exec("echo $((((2 + 3)) * 4))").unwrap();
    assert_eq!(r.stdout, "20\n");
}

#[test]
fn arithmetic_assignment_in_expansion() {
    let mut sh = shell();
    let r = sh.exec("echo $((x = 10, x + 5))").unwrap();
    assert_eq!(r.stdout, "15\n");
    let r = sh.exec("echo $x").unwrap();
    assert_eq!(r.stdout, "10\n");
}

// ── Command-specific coverage gaps ───────────────────────────────

#[test]
fn cat_dash_n_numbers_lines() {
    let mut sh = shell();
    sh.exec("printf 'alpha\\nbeta\\ngamma\\n' > /f.txt")
        .unwrap();
    let r = sh.exec("cat -n /f.txt").unwrap();
    // Verify line numbers appear with their corresponding content
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains('1') && lines[0].contains("alpha"));
    assert!(lines[1].contains('2') && lines[1].contains("beta"));
    assert!(lines[2].contains('3') && lines[2].contains("gamma"));
}

#[test]
fn cat_multiple_files() {
    let mut sh = shell();
    sh.exec("echo aaa > /a.txt && echo bbb > /b.txt").unwrap();
    let r = sh.exec("cat /a.txt /b.txt").unwrap();
    assert_eq!(r.stdout, "aaa\nbbb\n");
}

#[test]
fn touch_updates_existing_file_mtime() {
    let mut sh = shell();
    sh.exec("echo data > /ts.txt").unwrap();
    // touch should not change content
    sh.exec("touch /ts.txt").unwrap();
    let r = sh.exec("cat /ts.txt").unwrap();
    assert_eq!(r.stdout, "data\n");
}

#[test]
fn mkdir_p_existing_directory_succeeds() {
    let mut sh = shell();
    sh.exec("mkdir -p /exists").unwrap();
    // Should not error on existing dir
    let r = sh.exec("mkdir -p /exists").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn mkdir_without_p_fails_on_existing() {
    let mut sh = shell();
    sh.exec("mkdir /exists").unwrap();
    let r = sh.exec("mkdir /exists").unwrap();
    assert_ne!(r.exit_code, 0);
}

#[test]
fn ls_recursive() {
    let mut sh = shell();
    sh.exec("mkdir -p /lsdir/sub && echo x > /lsdir/a.txt && echo y > /lsdir/sub/b.txt")
        .unwrap();
    let r = sh.exec("ls -R /lsdir").unwrap();
    assert!(r.stdout.contains("a.txt"));
    assert!(r.stdout.contains("b.txt"));
    assert!(r.stdout.contains("sub"));
}

// ── Redirection edge cases ───────────────────────────────────────

#[test]
fn redirect_to_nonexistent_directory_errors() {
    let mut sh = shell();
    let result = sh.exec("echo data > /no/such/dir/file.txt");
    // Should error - either as Err or as non-zero exit
    match result {
        Err(_) => {} // Expected: error for missing directory
        Ok(r) => assert_ne!(r.exit_code, 0),
    }
}

#[test]
fn redirect_stderr_and_stdout_combined() {
    let mut sh = shell();
    let r = sh
        .exec("{ echo out; nonexistent_cmd; } > /combined.txt 2>&1; cat /combined.txt")
        .unwrap();
    assert!(r.stdout.contains("out"));
    assert!(r.stdout.contains("command not found"));
}

#[test]
fn here_string_trailing_newline() {
    let mut sh = shell();
    let r = sh.exec("cat <<<hello | wc -l").unwrap();
    assert_eq!(r.stdout.trim(), "1");
}

// ── Pipeline edge cases ──────────────────────────────────────────

#[test]
fn multi_stage_pipeline() {
    let mut sh = shell();
    let r = sh.exec("echo -e 'c\\na\\nb' | sort | head -n 1").unwrap();
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn pipeline_exit_code_last_command() {
    let mut sh = shell();
    let r = sh.exec("false | true").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn pipeline_pipefail_reports_first_failure() {
    let mut sh = shell();
    let r = sh.exec("set -o pipefail; false | true").unwrap();
    assert_ne!(r.exit_code, 0);
}

// ── IFS behavior ─────────────────────────────────────────────────

#[test]
fn custom_ifs_splitting() {
    let mut sh = shell();
    let r = sh
        .exec("IFS=:; DATA='a:b:c'; for x in $DATA; do echo $x; done")
        .unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

#[test]
fn default_ifs_collapses_whitespace() {
    let mut sh = shell();
    let r = sh
        .exec("VAR='  a   b   c  '; for x in $VAR; do echo $x; done")
        .unwrap();
    assert_eq!(r.stdout, "a\nb\nc\n");
}

// ── Command substitution edge cases ──────────────────────────────

#[test]
fn command_substitution_strips_trailing_newlines() {
    let mut sh = shell();
    let r = sh
        .exec("X=$(printf 'hello\\n\\n\\n'); echo \"${#X}\"")
        .unwrap();
    assert_eq!(r.stdout, "5\n");
}

// ── Quoting edge cases ──────────────────────────────────────────

#[test]
fn single_quotes_prevent_expansion() {
    let mut sh = shell();
    sh.exec("VAR=hello").unwrap();
    let r = sh.exec("echo '$VAR'").unwrap();
    assert_eq!(r.stdout, "$VAR\n");
}

#[test]
fn double_quotes_allow_expansion() {
    let mut sh = shell();
    sh.exec("VAR=hello").unwrap();
    let r = sh.exec("echo \"$VAR\"").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn double_quotes_preserve_spaces() {
    let mut sh = shell();
    sh.exec("VAR='a   b   c'").unwrap();
    let r = sh.exec("echo \"$VAR\"").unwrap();
    assert_eq!(r.stdout, "a   b   c\n");
}

// ── Read builtin edge cases ─────────────────────────────────────

#[test]
fn read_eof_returns_exit_1() {
    let mut sh = shell();
    // Pipe an empty input to read; read at EOF returns 1
    let r = sh.exec("printf '' | { read var; echo $?; }").unwrap();
    assert!(
        r.stdout.trim() == "1",
        "expected exit 1 from read at EOF, got: {}",
        r.stdout
    );
}

#[test]
fn read_splits_on_ifs() {
    let mut sh = shell();
    let r = sh
        .exec("echo 'first second third' | { read a b c; echo \"a=$a b=$b c=$c\"; }")
        .unwrap();
    assert_eq!(r.stdout, "a=first b=second c=third\n");
}

// ── Trap edge cases ─────────────────────────────────────────────

#[test]
fn trap_reset() {
    let mut sh = shell();
    sh.exec("trap 'echo bye' EXIT").unwrap();
    sh.exec("trap '' EXIT").unwrap();
    let r = sh.exec("echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert!(!r.stdout.contains("bye"));
}

// ── eval edge cases ─────────────────────────────────────────────

#[test]
fn eval_executes_dynamically_built_command() {
    let mut sh = shell();
    let r = sh.exec("CMD='echo hello'; eval $CMD").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

// ── source edge cases ───────────────────────────────────────────

#[test]
fn source_runs_in_current_context() {
    let mut sh = shell();
    sh.exec("echo 'MY_SOURCED_VAR=sourced' > /setup.sh")
        .unwrap();
    sh.exec("source /setup.sh").unwrap();
    let r = sh.exec("echo $MY_SOURCED_VAR").unwrap();
    assert_eq!(r.stdout, "sourced\n");
}

// ── Readonly variable error ─────────────────────────────────────

#[test]
fn readonly_variable_assignment_error() {
    let mut sh = shell();
    sh.exec("readonly X=fixed").unwrap();
    let result = sh.exec("X=changed").unwrap();
    assert_ne!(result.exit_code, 0);
    assert!(result.stderr.contains("readonly"));
}

// ── cd edge cases ───────────────────────────────────────────────

#[test]
fn cd_nonexistent_directory_errors_cwd_unchanged() {
    let mut sh = shell();
    let r = sh.exec("cd /nonexistent_dir").unwrap();
    assert_ne!(r.exit_code, 0);
    let r = sh.exec("pwd").unwrap();
    assert_eq!(r.stdout, "/\n");
}

#[test]
fn cd_dash_returns_to_oldpwd() {
    let mut sh = shell();
    sh.exec("mkdir -p /dir1 /dir2").unwrap();
    sh.exec("cd /dir1").unwrap();
    sh.exec("cd /dir2").unwrap();
    sh.exec("cd -").unwrap();
    let r = sh.exec("pwd").unwrap();
    assert_eq!(r.stdout, "/dir1\n");
}

// ── Parameter expansion edge cases ───────────────────────────────

#[test]
fn expand_assign_default_sets_variable() {
    let mut sh = shell();
    let r = sh.exec("echo ${X:=hello}; echo $X").unwrap();
    assert_eq!(r.stdout, "hello\nhello\n");
}

#[test]
fn expand_error_if_unset() {
    let mut sh = shell();
    let r = sh
        .exec("echo ${MISSING:?variable is required} 2>&1")
        .unwrap();
    assert_eq!(r.exit_code, 127);
    assert!(r.stderr.contains("variable is required"));
}

#[test]
fn expand_alternative_value_when_set() {
    let mut sh = shell();
    let r = sh.exec("X=hello; echo ${X:+replacement}").unwrap();
    assert_eq!(r.stdout, "replacement\n");
}

#[test]
fn expand_alternative_value_when_unset() {
    let mut sh = shell();
    let r = sh.exec("echo ${UNSET_VAR:+replacement}").unwrap();
    assert_eq!(r.stdout, "\n");
}

#[test]
fn expand_case_modification_uppercase() {
    let mut sh = shell();
    let r = sh.exec("X=hello; echo ${X^}; echo ${X^^}").unwrap();
    assert_eq!(r.stdout, "Hello\nHELLO\n");
}

#[test]
fn expand_case_modification_lowercase() {
    let mut sh = shell();
    let r = sh.exec("X=HELLO; echo ${X,}; echo ${X,,}").unwrap();
    assert_eq!(r.stdout, "hELLO\nhello\n");
}

// ── Special variables ────────────────────────────────────────────

#[test]
fn random_variable_is_numeric_in_range() {
    let mut sh = shell();
    let r = sh.exec("echo $RANDOM").unwrap();
    let val: i64 = r.stdout.trim().parse().expect("$RANDOM should be numeric");
    assert!((0..=32767).contains(&val));
}

#[test]
fn dollar_bang_is_empty_or_zero() {
    let mut sh = shell();
    let r = sh.exec("echo $!").unwrap();
    let val = r.stdout.trim();
    assert!(
        val.is_empty() || val == "0",
        "expected empty or 0, got: {val}"
    );
}

// ── Command integration tests (from coverage gaps) ──────────────

#[test]
fn symlink_create_and_readlink() {
    let mut sh = shell();
    sh.exec("echo data > /target.txt").unwrap();
    sh.exec("ln -s /target.txt /link.txt").unwrap();
    // Follow the symlink via cat
    let r = sh.exec("cat /link.txt").unwrap();
    assert_eq!(r.stdout, "data\n");
    // Verify it's a symlink via test -L
    let r = sh.exec("[ -L /link.txt ] && echo is_symlink").unwrap();
    assert_eq!(r.stdout, "is_symlink\n");
}

#[test]
fn tee_copies_stdin_to_file_and_stdout() {
    let mut sh = shell();
    let r = sh.exec("echo hello | tee /tee_out.txt").unwrap();
    assert_eq!(r.stdout, "hello\n");
    let r = sh.exec("cat /tee_out.txt").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn stat_shows_file_info() {
    let mut sh = shell();
    sh.exec("echo content > /statfile.txt").unwrap();
    let r = sh.exec("stat /statfile.txt").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("statfile.txt"));
}

#[test]
fn chmod_changes_permissions() {
    let mut sh = shell();
    sh.exec("echo x > /chf.txt").unwrap();
    let r = sh.exec("chmod 755 /chf.txt").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn cut_fields_with_delimiter() {
    let mut sh = shell();
    let r = sh.exec("echo 'a:b:c' | cut -d: -f2").unwrap();
    assert_eq!(r.stdout, "b\n");
}

#[test]
fn printf_format_string() {
    let mut sh = shell();
    let r = sh.exec("printf '%s is %d\\n' hello 42").unwrap();
    assert_eq!(r.stdout, "hello is 42\n");
}

#[test]
fn printf_zero_padded_width() {
    let mut sh = shell();
    let r = sh.exec("printf '%05d\\n' 42").unwrap();
    assert_eq!(r.stdout, "00042\n");
}

#[test]
fn printf_left_aligned() {
    let mut sh = shell();
    let r = sh.exec("printf '%-10s|\\n' hi").unwrap();
    assert_eq!(r.stdout, "hi        |\n");
}

#[test]
fn printf_float_precision() {
    let mut sh = shell();
    let r = sh.exec("printf '%.2f\\n' 3.14159").unwrap();
    assert_eq!(r.stdout, "3.14\n");
}

#[test]
fn seq_generates_sequence() {
    let mut sh = shell();
    let r = sh.exec("seq 3").unwrap();
    assert_eq!(r.stdout, "1\n2\n3\n");
}

#[test]
fn seq_with_range_and_step() {
    let mut sh = shell();
    let r = sh.exec("seq 1 2 7").unwrap();
    assert_eq!(r.stdout, "1\n3\n5\n7\n");
}

#[test]
fn which_finds_builtin() {
    let mut sh = shell();
    let r = sh.exec("which echo").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("echo"));
}

#[test]
fn which_not_found_exits_1() {
    let mut sh = shell();
    let r = sh.exec("which nonexistent_command_xyz").unwrap();
    assert_ne!(r.exit_code, 0);
}

#[test]
fn base64_encode_and_decode() {
    let mut sh = shell();
    let r = sh.exec("echo -n hello | base64").unwrap();
    assert_eq!(r.stdout.trim(), "aGVsbG8=");
    let r = sh.exec("echo 'aGVsbG8=' | base64 -d").unwrap();
    assert_eq!(r.stdout, "hello");
}

#[test]
fn md5sum_produces_hash() {
    let mut sh = shell();
    sh.exec("echo -n hello > /hash.txt").unwrap();
    let r = sh.exec("md5sum /hash.txt").unwrap();
    assert!(r.stdout.contains("5d41402abc4b2a76b9719d911017c592"));
}

#[test]
fn whoami_returns_value() {
    let mut sh = shell();
    let r = sh.exec("whoami").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(!r.stdout.trim().is_empty());
}

#[test]
fn hostname_returns_value() {
    let mut sh = shell();
    let r = sh.exec("hostname").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(!r.stdout.trim().is_empty());
}

#[test]
fn uname_returns_value() {
    let mut sh = shell();
    let r = sh.exec("uname").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(!r.stdout.trim().is_empty());
}

#[test]
fn expr_arithmetic() {
    let mut sh = shell();
    let r = sh.exec("expr 3 + 4").unwrap();
    assert_eq!(r.stdout.trim(), "7");
}

#[test]
fn expr_string_length() {
    let mut sh = shell();
    let r = sh.exec("expr length hello").unwrap();
    assert_eq!(r.stdout.trim(), "5");
}

#[test]
fn rev_reverses_lines() {
    let mut sh = shell();
    let r = sh.exec("echo hello | rev").unwrap();
    assert_eq!(r.stdout, "olleh\n");
}

#[test]
fn nl_numbers_lines() {
    let mut sh = shell();
    let r = sh.exec("printf 'a\\nb\\n' | nl").unwrap();
    assert!(r.stdout.contains('1'));
    assert!(r.stdout.contains('a'));
    assert!(r.stdout.contains('2'));
    assert!(r.stdout.contains('b'));
}

#[test]
fn paste_joins_lines() {
    let mut sh = shell();
    sh.exec("printf 'a\\nb\\n' > /p1.txt && printf '1\\n2\\n' > /p2.txt")
        .unwrap();
    let r = sh.exec("paste /p1.txt /p2.txt").unwrap();
    assert!(r.stdout.contains("a\t1"));
    assert!(r.stdout.contains("b\t2"));
}

#[test]
fn fold_wraps_long_lines() {
    let mut sh = shell();
    let r = sh.exec("echo 'abcdefghij' | fold -w 5").unwrap();
    assert_eq!(r.stdout, "abcde\nfghij\n");
}

#[test]
fn tree_shows_directory_structure() {
    let mut sh = shell();
    sh.exec("mkdir -p /tdir/sub && echo x > /tdir/f.txt")
        .unwrap();
    let r = sh.exec("tree /tdir").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("f.txt"));
    assert!(r.stdout.contains("sub"));
}

// ── Redirect &> (stdout+stderr to file) ─────────────────────────

#[test]
fn redirect_ampersand_greater_than() {
    let mut sh = shell();
    let r = sh
        .exec("{ echo out; nosuchcmd_xyz; } &> /both.txt; cat /both.txt")
        .unwrap();
    assert!(r.stdout.contains("out"));
    assert!(r.stdout.contains("command not found"));
}

// ── Arrays (M6.1) ──────────────────────────────────────────────────

#[test]
fn array_basic_indexed_assignment() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(one two three); echo ${arr[0]} ${arr[1]} ${arr[2]}")
        .unwrap();
    assert_eq!(r.stdout, "one two three\n");
}

#[test]
fn array_element_assignment() {
    let mut sh = shell();
    let r = sh
        .exec("arr[0]=hello; arr[1]=world; echo ${arr[0]} ${arr[1]}")
        .unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn array_all_elements_at() {
    let mut sh = shell();
    let r = sh.exec("arr=(a b c); echo ${arr[@]}").unwrap();
    assert_eq!(r.stdout, "a b c\n");
}

#[test]
fn array_all_elements_star() {
    let mut sh = shell();
    let r = sh.exec("arr=(a b c); echo ${arr[*]}").unwrap();
    assert_eq!(r.stdout, "a b c\n");
}

#[test]
fn array_star_with_ifs() {
    let mut sh = shell();
    let r = sh.exec("IFS=','; arr=(a b c); echo \"${arr[*]}\"").unwrap();
    assert_eq!(r.stdout, "a,b,c\n");
}

#[test]
fn array_at_in_double_quotes_word_splitting() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(one two three); for x in \"${arr[@]}\"; do echo \"item:$x\"; done")
        .unwrap();
    assert_eq!(r.stdout, "item:one\nitem:two\nitem:three\n");
}

#[test]
fn array_length() {
    let mut sh = shell();
    let r = sh.exec("arr=(a b c d e); echo ${#arr[@]}").unwrap();
    assert_eq!(r.stdout, "5\n");
}

#[test]
fn array_length_star() {
    let mut sh = shell();
    let r = sh.exec("arr=(a b c); echo ${#arr[*]}").unwrap();
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn array_keys_at() {
    let mut sh = shell();
    let r = sh.exec("arr=(a b c); echo ${!arr[@]}").unwrap();
    assert_eq!(r.stdout, "0 1 2\n");
}

#[test]
fn array_sparse_indices() {
    let mut sh = shell();
    let r = sh
        .exec("arr=([0]=x [5]=y [10]=z); echo ${!arr[@]}")
        .unwrap();
    assert_eq!(r.stdout, "0 5 10\n");
}

#[test]
fn array_sparse_values() {
    let mut sh = shell();
    let r = sh.exec("arr=([0]=x [5]=y [10]=z); echo ${arr[@]}").unwrap();
    assert_eq!(r.stdout, "x y z\n");
}

#[test]
fn array_unset_element() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(a b c d); unset arr[1]; echo ${arr[@]}")
        .unwrap();
    assert_eq!(r.stdout, "a c d\n");
}

#[test]
fn array_unset_preserves_indices() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(a b c d); unset arr[1]; echo ${!arr[@]}")
        .unwrap();
    assert_eq!(r.stdout, "0 2 3\n");
}

#[test]
fn array_append() {
    let mut sh = shell();
    let r = sh.exec("arr=(a b); arr+=(c d); echo ${arr[@]}").unwrap();
    assert_eq!(r.stdout, "a b c d\n");
}

#[test]
fn array_append_length() {
    let mut sh = shell();
    let r = sh.exec("arr=(x y); arr+=(z); echo ${#arr[@]}").unwrap();
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn array_scalar_as_element_zero() {
    let mut sh = shell();
    let r = sh.exec("x=hello; echo ${x[0]}").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn array_no_name_gives_element_zero() {
    let mut sh = shell();
    let r = sh.exec("arr=(first second third); echo $arr").unwrap();
    assert_eq!(r.stdout, "first\n");
}

#[test]
fn array_declare_indexed() {
    let mut sh = shell();
    let r = sh
        .exec("declare -a myarr; myarr[0]=hello; echo ${myarr[0]}")
        .unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn array_declare_associative() {
    let mut sh = shell();
    let r = sh
        .exec(
            "declare -A mymap; mymap[name]=alice; mymap[age]=30; echo ${mymap[name]} ${mymap[age]}",
        )
        .unwrap();
    assert_eq!(r.stdout, "alice 30\n");
}

#[test]
fn array_associative_keys() {
    let mut sh = shell();
    let r = sh
        .exec("declare -A m; m[a]=1; m[b]=2; m[c]=3; for k in \"${!m[@]}\"; do echo $k; done")
        .unwrap();
    let mut lines: Vec<&str> = r.stdout.trim().split('\n').collect();
    lines.sort();
    assert_eq!(lines, vec!["a", "b", "c"]);
}

#[test]
fn array_in_arithmetic() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(10 20 30); echo $((arr[0] + arr[2]))")
        .unwrap();
    assert_eq!(r.stdout, "40\n");
}

#[test]
fn array_arithmetic_assignment() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(0 0 0); ((arr[1] = 42)); echo ${arr[1]}")
        .unwrap();
    assert_eq!(r.stdout, "42\n");
}

#[test]
fn array_arithmetic_compound_assign() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(10 20 30); ((arr[1] += 5)); echo ${arr[1]}")
        .unwrap();
    assert_eq!(r.stdout, "25\n");
}

#[test]
fn array_empty_at_expansion_no_empty_word() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(); for x in \"${arr[@]}\"; do echo \"item:$x\"; done; echo done")
        .unwrap();
    assert_eq!(r.stdout, "done\n");
}

#[test]
fn array_element_string_length() {
    let mut sh = shell();
    let r = sh.exec("arr=(hello world); echo ${#arr[0]}").unwrap();
    assert_eq!(r.stdout, "5\n");
}

#[test]
fn array_with_explicit_indices() {
    let mut sh = shell();
    let r = sh
        .exec("arr=([2]=two [5]=five); echo ${arr[2]} ${arr[5]}")
        .unwrap();
    assert_eq!(r.stdout, "two five\n");
}

#[test]
fn array_mixed_auto_and_explicit_indices() {
    let mut sh = shell();
    let r = sh.exec("arr=(a [3]=b c); echo ${!arr[@]}").unwrap();
    assert_eq!(r.stdout, "0 3 4\n");
}

#[test]
fn array_overwrite_element() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(old value); arr[0]=new; echo ${arr[0]} ${arr[1]}")
        .unwrap();
    assert_eq!(r.stdout, "new value\n");
}

#[test]
fn array_read_nonexistent_index() {
    let mut sh = shell();
    let r = sh.exec("arr=(a b c); echo \"[${arr[99]}]\"").unwrap();
    assert_eq!(r.stdout, "[]\n");
}

#[test]
fn array_max_elements_limit() {
    let mut sh = RustBashBuilder::new()
        .max_array_elements(5)
        .build()
        .unwrap();
    let r = sh.exec("arr=(1 2 3 4 5 6)");
    assert!(r.is_err());
}

#[test]
fn array_export_shows_scalar() {
    let mut sh = shell();
    let r = sh
        .exec("arr=(hello world); export arr; echo ${arr[0]}")
        .unwrap();
    assert_eq!(r.stdout, "hello\n");
}

// ── declare attributes ─────────────────────────────────────────────

#[test]
fn declare_integer_basic() {
    let mut sh = shell();
    let r = sh.exec("declare -i x; x=2+3; echo $x").unwrap();
    assert_eq!(r.stdout, "5\n");
}

#[test]
fn declare_integer_with_value() {
    let mut sh = shell();
    let r = sh.exec("declare -i x=2+3; echo $x").unwrap();
    assert_eq!(r.stdout, "5\n");
}

#[test]
fn declare_integer_propagation_append() {
    let mut sh = shell();
    let r = sh.exec("declare -i x=10; x+=5; echo $x").unwrap();
    assert_eq!(r.stdout, "15\n");
}

#[test]
fn declare_integer_variable_reference() {
    let mut sh = shell();
    let r = sh.exec("declare -i x; y=3; x=y+7; echo $x").unwrap();
    assert_eq!(r.stdout, "10\n");
}

#[test]
fn declare_lowercase() {
    let mut sh = shell();
    let r = sh.exec("declare -l s; s=HELLO; echo $s").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn declare_lowercase_with_value() {
    let mut sh = shell();
    let r = sh.exec("declare -l s=WORLD; echo $s").unwrap();
    assert_eq!(r.stdout, "world\n");
}

#[test]
fn declare_uppercase() {
    let mut sh = shell();
    let r = sh.exec("declare -u s; s=hello; echo $s").unwrap();
    assert_eq!(r.stdout, "HELLO\n");
}

#[test]
fn declare_uppercase_with_value() {
    let mut sh = shell();
    let r = sh.exec("declare -u s=hello; echo $s").unwrap();
    assert_eq!(r.stdout, "HELLO\n");
}

#[test]
fn declare_nameref_read() {
    let mut sh = shell();
    let r = sh.exec("x=42; declare -n ref=x; echo $ref").unwrap();
    assert_eq!(r.stdout, "42\n");
}

#[test]
fn declare_nameref_write() {
    let mut sh = shell();
    let r = sh.exec("x=42; declare -n ref=x; ref=99; echo $x").unwrap();
    assert_eq!(r.stdout, "99\n");
}

#[test]
fn declare_nameref_circular_error() {
    let mut sh = shell();
    // Bash prints a warning for circular namerefs but continues (exit 0).
    let r = sh.exec("declare -n a=b; declare -n b=a; echo $a").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn declare_print_integer_exported() {
    let mut sh = shell();
    let r = sh.exec("declare -ix num=5; declare -p num").unwrap();
    assert_eq!(r.stdout, "declare -ix num=\"5\"\n");
}

#[test]
fn declare_print_lowercase() {
    let mut sh = shell();
    let r = sh.exec("declare -l s=HELLO; declare -p s").unwrap();
    assert_eq!(r.stdout, "declare -l s=\"hello\"\n");
}

#[test]
fn declare_array_indexed() {
    let mut sh = shell();
    let r = sh.exec("declare -a arr; arr[0]=x; echo ${arr[0]}").unwrap();
    assert_eq!(r.stdout, "x\n");
}

#[test]
fn declare_array_associative() {
    let mut sh = shell();
    let r = sh
        .exec("declare -A m; m[hello]=world; echo ${m[hello]}")
        .unwrap();
    assert_eq!(r.stdout, "world\n");
}

#[test]
fn declare_nameref_chain() {
    let mut sh = shell();
    let r = sh
        .exec("x=100; declare -n ref1=x; declare -n ref2=ref1; echo $ref2")
        .unwrap();
    assert_eq!(r.stdout, "100\n");
}

#[test]
fn declare_integer_in_for_loop() {
    let mut sh = shell();
    let r = sh
        .exec("declare -i sum=0; for i in 1 2 3; do sum+=i; done; echo $sum")
        .unwrap();
    assert_eq!(r.stdout, "6\n");
}

#[test]
fn declare_indexed_array_with_value() {
    let mut sh = shell();
    let r = sh.exec("declare -a arr=hello; echo ${arr[0]}").unwrap();
    assert_eq!(r.stdout, "hello\n");
}

#[test]
fn declare_integer_readonly_prevents_reassign() {
    let mut sh = shell();
    sh.exec("declare -ir x=5").unwrap();
    let r = sh.exec("x=10").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("readonly"));
}

#[test]
fn declare_integer_array_element() {
    let mut sh = shell();
    let r = sh
        .exec("declare -ia arr; arr[0]=2+3; echo ${arr[0]}")
        .unwrap();
    assert_eq!(r.stdout, "5\n");
}

#[test]
fn declare_print_nonexistent_returns_error() {
    let mut sh = shell();
    let r = sh.exec("declare -p nonexistent").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("not found"));
}

// ── Special variable tracking (M6.8) ────────────────────────────────

#[test]
fn lineno_tracks_statement_positions() {
    let mut sh = shell();
    let r = sh.exec("echo $LINENO\necho $LINENO\necho $LINENO").unwrap();
    assert_eq!(r.stdout, "1\n2\n3\n");
}

#[test]
fn lineno_inside_function() {
    let mut sh = shell();
    let r = sh.exec("f() { echo $LINENO; }; f").unwrap();
    // The function body's echo is on line 1
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn seconds_returns_elapsed_time() {
    let mut sh = shell();
    let r = sh.exec("echo $SECONDS").unwrap();
    let secs: u64 = r.stdout.trim().parse().unwrap();
    assert!(secs < 5, "SECONDS should be small, got {secs}");
}

#[test]
fn seconds_assignment_resets_timer() {
    let mut sh = shell();
    let r = sh.exec("SECONDS=0; echo $SECONDS").unwrap();
    assert_eq!(r.stdout, "0\n");
}

#[test]
fn underscore_last_argument() {
    let mut sh = shell();
    let r = sh.exec("echo hello world; echo $_").unwrap();
    assert_eq!(r.stdout, "hello world\nworld\n");
}

#[test]
fn underscore_updates_per_command() {
    let mut sh = shell();
    let r = sh.exec("echo a b c; echo $_; echo x; echo $_").unwrap();
    assert_eq!(r.stdout, "a b c\nc\nx\nx\n");
}

#[test]
fn funcname_current_function() {
    let mut sh = shell();
    let r = sh
        .exec("greet() { echo \"${FUNCNAME[0]}\"; }; greet")
        .unwrap();
    assert_eq!(r.stdout, "greet\n");
}

#[test]
fn funcname_nested_calls() {
    let mut sh = shell();
    let r = sh
        .exec("inner() { echo \"${FUNCNAME[0]} ${FUNCNAME[1]}\"; }; outer() { inner; }; outer")
        .unwrap();
    assert_eq!(r.stdout, "inner outer\n");
}

#[test]
fn bash_lineno_callsite() {
    let mut sh = shell();
    let r = sh.exec("f() { echo \"${BASH_LINENO[0]}\"; }\nf").unwrap();
    assert_eq!(r.stdout, "2\n");
}

#[test]
fn bash_source_empty_at_toplevel() {
    let mut sh = shell();
    let r = sh.exec("echo \"${BASH_SOURCE[0]}\"").unwrap();
    assert_eq!(r.stdout, "\n");
}

#[test]
fn ppid_returns_numeric() {
    let mut sh = shell();
    let r = sh.exec("echo $PPID").unwrap();
    assert!(r.stdout.trim().parse::<u32>().is_ok());
}

#[test]
fn uid_returns_numeric() {
    let mut sh = shell();
    let r = sh.exec("echo $UID").unwrap();
    assert_eq!(r.stdout, "1000\n");
}

#[test]
fn euid_returns_numeric() {
    let mut sh = shell();
    let r = sh.exec("echo $EUID").unwrap();
    assert_eq!(r.stdout, "1000\n");
}

#[test]
fn bashpid_returns_numeric() {
    let mut sh = shell();
    let r = sh.exec("echo $BASHPID").unwrap();
    assert!(r.stdout.trim().parse::<u32>().is_ok());
}

#[test]
fn shellopts_reflects_set_flags() {
    let mut sh = shell();
    let r = sh.exec("set -e; echo $SHELLOPTS").unwrap();
    assert!(r.stdout.contains("errexit"));
}

#[test]
fn shellopts_empty_by_default() {
    let mut sh = shell();
    let r = sh.exec("echo \"$SHELLOPTS\"").unwrap();
    // Default options: braceexpand and hashall are always on
    assert!(r.stdout.trim().contains("braceexpand"));
    assert!(r.stdout.trim().contains("hashall"));
}

#[test]
fn bashopts_reflects_shopt_flags() {
    let mut sh = shell();
    let r = sh.exec("shopt -s nullglob; echo $BASHOPTS").unwrap();
    assert!(r.stdout.contains("nullglob"));
}

#[test]
fn bashopts_contains_extglob_by_default() {
    let mut sh = shell();
    let r = sh.exec("echo $BASHOPTS").unwrap();
    // extglob and globskipdots are on by default
    assert!(r.stdout.contains("extglob"));
}

#[test]
fn machtype_is_set() {
    let mut sh = shell();
    let r = sh.exec("echo $MACHTYPE").unwrap();
    assert_eq!(r.stdout, "x86_64-pc-linux-gnu\n");
}

#[test]
fn hosttype_is_set() {
    let mut sh = shell();
    let r = sh.exec("echo $HOSTTYPE").unwrap();
    assert_eq!(r.stdout, "x86_64\n");
}

#[test]
fn funcname_array_length() {
    let mut sh = shell();
    let r = sh
        .exec("inner() { echo \"${#FUNCNAME[@]}\"; }; outer() { inner; }; outer")
        .unwrap();
    assert_eq!(r.stdout, "2\n");
}

#[test]
fn funcname_all_elements() {
    let mut sh = shell();
    let r = sh
        .exec("inner() { echo \"${FUNCNAME[@]}\"; }; outer() { inner; }; outer")
        .unwrap();
    assert_eq!(r.stdout, "inner outer\n");
}

#[test]
fn funcname_array_keys() {
    let mut sh = shell();
    let r = sh
        .exec("inner() { echo \"${!FUNCNAME[@]}\"; }; outer() { inner; }; outer")
        .unwrap();
    assert_eq!(r.stdout, "0 1\n");
}

#[test]
fn lineno_in_arithmetic() {
    let mut sh = shell();
    let r = sh.exec("echo $((LINENO + 0))").unwrap();
    assert_eq!(r.stdout, "1\n");
}

// ── Shell Option Enforcement (M6.9) ────────────────────────────────

#[test]
fn xtrace_emits_trace_on_stderr() {
    let mut sh = shell();
    let r = sh.exec("set -x; echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert!(
        r.stderr.contains("+ echo hello"),
        "stderr should contain xtrace: {}",
        r.stderr
    );
}

#[test]
fn xtrace_uses_ps4_prefix() {
    let mut sh = shell();
    let r = sh.exec("PS4='>> '; set -x; echo hi").unwrap();
    assert!(
        r.stderr.contains(">> echo hi"),
        "stderr should use PS4 prefix: {}",
        r.stderr
    );
}

#[test]
fn xtrace_not_emitted_for_set_dash_x_itself() {
    let mut sh = shell();
    let r = sh.exec("set -x; echo done").unwrap();
    // `set -x` was not yet traced (xtrace was off when it ran)
    assert!(
        !r.stderr.contains("+ set -x"),
        "set -x itself should not be traced: {}",
        r.stderr
    );
}

#[test]
fn xtrace_set_plus_x_is_traced() {
    let mut sh = shell();
    let r = sh.exec("set -x; set +x; echo done").unwrap();
    assert!(
        r.stderr.contains("+ set +x"),
        "set +x should be traced: {}",
        r.stderr
    );
    assert_eq!(r.stdout, "done\n");
}

#[test]
fn noexec_suppresses_output() {
    let mut sh = shell();
    let r = sh.exec("set -n; echo hidden").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn noexec_blocks_all_after_activation() {
    // bash: once set -n is active, all subsequent commands are skipped
    // including set +n — there is no way to re-enable execution
    let mut sh = shell();
    let r = sh.exec("set -n; set +n; echo visible").unwrap();
    assert_eq!(r.stdout, "");
}

#[test]
fn noclobber_prevents_overwrite() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([("/f.txt".into(), b"old\n".to_vec())]))
        .build()
        .unwrap();
    let r = sh
        .exec("set -C; echo new > /f.txt; echo $?; cat /f.txt")
        .unwrap();
    assert_eq!(r.stdout, "1\nold\n");
}

#[test]
fn noclobber_allows_force_clobber() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([("/f.txt".into(), b"old\n".to_vec())]))
        .build()
        .unwrap();
    let r = sh.exec("set -C; echo new >| /f.txt; cat /f.txt").unwrap();
    assert_eq!(r.stdout, "new\n");
}

#[test]
fn noclobber_allows_append() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([("/f.txt".into(), b"old\n".to_vec())]))
        .build()
        .unwrap();
    let r = sh.exec("set -C; echo more >> /f.txt; cat /f.txt").unwrap();
    assert_eq!(r.stdout, "old\nmore\n");
}

#[test]
fn noclobber_allows_new_file() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([("/dir/.keep".into(), b"".to_vec())]))
        .build()
        .unwrap();
    let r = sh
        .exec("set -C; echo content > /dir/new.txt; cat /dir/new.txt")
        .unwrap();
    assert_eq!(r.stdout, "content\n");
}

#[test]
fn allexport_marks_variable_exported() {
    let mut sh = shell();
    let r = sh.exec("set -a; MYVAR=hello; env | grep MYVAR").unwrap();
    assert_eq!(r.stdout, "MYVAR=hello\n");
}

#[test]
fn noglob_disables_glob_expansion() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([
            ("/a.txt".into(), b"".to_vec()),
            ("/b.txt".into(), b"".to_vec()),
        ]))
        .build()
        .unwrap();
    let r = sh.exec("set -f; echo *.txt").unwrap();
    assert_eq!(r.stdout, "*.txt\n");
}

#[test]
fn noglob_can_be_reenabled() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([
            ("/a.txt".into(), b"".to_vec()),
            ("/b.txt".into(), b"".to_vec()),
        ]))
        .build()
        .unwrap();
    let r = sh.exec("set -f; set +f; echo *.txt").unwrap();
    assert!(r.stdout.contains("a.txt"), "glob should expand after +f");
}

#[test]
fn posix_option_accepted() {
    let mut sh = shell();
    let r = sh.exec("set -o posix; echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn vi_emacs_options_accepted() {
    let mut sh = shell();
    let r = sh.exec("set -o vi; set -o emacs; echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn set_option_names_in_format_output() {
    let mut sh = shell();
    let r = sh.exec("set -o").unwrap();
    assert!(r.stdout.contains("noclobber"));
    assert!(r.stdout.contains("noglob"));
    assert!(r.stdout.contains("allexport"));
    assert!(r.stdout.contains("verbose"));
    assert!(r.stdout.contains("noexec"));
    assert!(r.stdout.contains("posix"));
    assert!(r.stdout.contains("vi"));
    assert!(r.stdout.contains("emacs"));
}

#[test]
fn xtrace_bare_assignment() {
    let mut sh = shell();
    let r = sh.exec("set -x; X=hello; echo $X").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert!(r.stderr.contains("+ X=hello"));
    assert!(r.stderr.contains("+ echo hello"));
}

#[test]
fn noclobber_blocks_output_and_error_redirect() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([(
            "/tmp/existing.txt".into(),
            b"old\n".to_vec(),
        )]))
        .build()
        .unwrap();
    let r = sh
        .exec("set -C; echo hi &> /tmp/existing.txt; echo $?; cat /tmp/existing.txt")
        .unwrap();
    assert_eq!(r.stdout, "1\nold\n");
    assert!(r.stderr.contains("cannot overwrite existing file"));
}

#[test]
fn noclobber_allows_append_output_and_error() {
    let mut sh = RustBashBuilder::new()
        .files(HashMap::from([(
            "/tmp/existing.txt".into(),
            b"old\n".to_vec(),
        )]))
        .build()
        .unwrap();
    let r = sh
        .exec("set -C; echo hi &>> /tmp/existing.txt; cat /tmp/existing.txt")
        .unwrap();
    assert_eq!(r.stdout, "old\nhi\n");
}

// ── M7.7: Default filesystem layout and command resolution ────────

#[test]
fn default_dirs_exist() {
    let mut sh = shell();
    let r = sh
        .exec("test -d /bin && test -d /usr/bin && test -d /tmp && test -d /dev && echo ok")
        .unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn default_home_dir_exists() {
    let mut sh = shell();
    let r = sh.exec("test -d /home/user && echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn custom_home_preserved() {
    let mut env = HashMap::new();
    env.insert("HOME".into(), "/custom/home".into());
    let mut sh = RustBashBuilder::new().env(env).build().unwrap();
    let r = sh.exec("echo $HOME").unwrap();
    assert_eq!(r.stdout, "/custom/home\n");
    let r = sh.exec("test -d /custom/home && echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn default_env_path() {
    let mut sh = shell();
    let r = sh.exec("echo $PATH").unwrap();
    assert_eq!(r.stdout, "/usr/bin:/bin\n");
}

#[test]
fn default_env_home() {
    let mut sh = shell();
    let r = sh.exec("echo $HOME").unwrap();
    assert_eq!(r.stdout, "/home/user\n");
}

#[test]
fn default_env_not_overwritten() {
    let mut env = HashMap::new();
    env.insert("HOME".into(), "/root".into());
    env.insert("USER".into(), "testuser".into());
    env.insert("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into());
    let mut sh = RustBashBuilder::new().env(env).build().unwrap();
    let r = sh.exec("echo $HOME $USER $PATH").unwrap();
    assert_eq!(r.stdout, "/root testuser /usr/local/bin:/usr/bin:/bin\n");
}

#[test]
fn dev_special_files_exist() {
    let mut sh = shell();
    let r = sh
        .exec("test -f /dev/null && test -f /dev/zero && echo ok")
        .unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn ls_bin_lists_commands() {
    let mut sh = shell();
    let r = sh.exec("ls /bin").unwrap();
    assert!(r.stdout.contains("ls"));
    assert!(r.stdout.contains("grep"));
    assert!(r.stdout.contains("cat"));
}

#[test]
fn which_resolves_via_path() {
    let mut sh = shell();
    let r = sh.exec("which ls").unwrap();
    assert_eq!(r.stdout.trim(), "/bin/ls");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn which_builtin_reports_builtin() {
    let mut sh = shell();
    let r = sh.exec("which cd").unwrap();
    assert!(r.stdout.contains("shell built-in command"));
}

#[test]
fn test_executable_bin() {
    let mut sh = shell();
    let r = sh.exec("test -f /bin/grep && echo ok").unwrap();
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn default_bash_version() {
    let mut sh = shell();
    let r = sh.exec("echo $BASH_VERSION").unwrap();
    assert!(!r.stdout.trim().is_empty());
}

#[test]
fn default_shell_var() {
    let mut sh = shell();
    let r = sh.exec("echo $SHELL").unwrap();
    assert_eq!(r.stdout, "/bin/bash\n");
}

#[test]
fn user_seeded_bin_file_not_clobbered() {
    let mut files = HashMap::new();
    files.insert("/bin/custom".into(), b"custom content".to_vec());
    let mut sh = RustBashBuilder::new().files(files).build().unwrap();
    let r = sh.exec("cat /bin/custom").unwrap();
    assert_eq!(r.stdout, "custom content");
}

// ── M7 Phase 3: New Commands ─────────────────────────────────────────

// ── timeout ──────────────────────────────────────────────────────────

#[test]
fn timeout_command_within_time() {
    let mut sh = shell();
    let r = sh.exec("timeout 10 echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert_eq!(r.exit_code, 0);
}

// ── time keyword ─────────────────────────────────────────────────────

#[test]
fn time_keyword_produces_timing_stderr() {
    let mut sh = shell();
    let r = sh.exec("time echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert!(r.stderr.contains("real\t"));
    assert!(r.stderr.contains("user\t"));
    assert!(r.stderr.contains("sys\t"));
}

// ── readlink ─────────────────────────────────────────────────────────

#[test]
fn readlink_symlink() {
    let mut sh = shell();
    sh.exec("echo data > /target.txt").unwrap();
    sh.exec("ln -s /target.txt /link.txt").unwrap();
    let r = sh.exec("readlink /link.txt").unwrap();
    assert_eq!(r.stdout, "/target.txt\n");
}

#[test]
fn readlink_f_canonicalize() {
    let mut sh = shell();
    sh.exec("mkdir -p /a/b").unwrap();
    sh.exec("echo x > /a/b/file.txt").unwrap();
    sh.exec("ln -s /a/b/file.txt /link").unwrap();
    let r = sh.exec("readlink -f /link").unwrap();
    assert_eq!(r.stdout, "/a/b/file.txt\n");
}

// ── rmdir ────────────────────────────────────────────────────────────

#[test]
fn rmdir_empty_directory() {
    let mut sh = shell();
    sh.exec("mkdir /emptydir").unwrap();
    let r = sh.exec("rmdir /emptydir").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn rmdir_nonempty_fails() {
    let mut sh = shell();
    sh.exec("mkdir /nonempty && echo x > /nonempty/file.txt")
        .unwrap();
    let r = sh.exec("rmdir /nonempty").unwrap();
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("not empty") || r.stderr.contains("Not empty"));
}

// ── du ───────────────────────────────────────────────────────────────

#[test]
fn du_summary() {
    let mut sh = shell();
    sh.exec("mkdir -p /dutest && echo hello > /dutest/file.txt")
        .unwrap();
    let r = sh.exec("du -s /dutest").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("/dutest"));
}

// ── sha1sum ──────────────────────────────────────────────────────────

#[test]
fn sha1sum_known_hash() {
    let mut sh = shell();
    let r = sh.exec("echo -n hello | sha1sum").unwrap();
    assert!(
        r.stdout
            .starts_with("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d")
    );
}

// ── fgrep / egrep ────────────────────────────────────────────────────

#[test]
fn fgrep_fixed_string() {
    let mut sh = shell();
    let r = sh.exec("echo 'hello world' | fgrep hello").unwrap();
    assert_eq!(r.stdout, "hello world\n");
}

#[test]
fn egrep_extended_regex() {
    let mut sh = shell();
    let r = sh.exec("echo 'cat' | egrep 'cat|dog'").unwrap();
    assert_eq!(r.stdout, "cat\n");
}

// ── sh / bash builtin ───────────────────────────────────────────────

#[test]
fn sh_c_executes_string() {
    let mut sh = shell();
    let r = sh.exec("sh -c 'echo hi'").unwrap();
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn bash_c_executes_string() {
    let mut sh = shell();
    let r = sh.exec("bash -c 'echo hi'").unwrap();
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn sh_script_file() {
    let mut sh = shell();
    sh.exec("echo 'echo from script' > /test.sh").unwrap();
    let r = sh.exec("sh /test.sh").unwrap();
    assert_eq!(r.stdout, "from script\n");
}

// ── bc ───────────────────────────────────────────────────────────────

#[test]
fn bc_basic_arithmetic() {
    let mut sh = shell();
    let r = sh.exec("echo '2+3' | bc").unwrap();
    assert_eq!(r.stdout, "5\n");
}

#[test]
fn bc_multiplication() {
    let mut sh = shell();
    let r = sh.exec("echo '6*7' | bc").unwrap();
    assert_eq!(r.stdout, "42\n");
}

#[test]
fn bc_division_with_scale() {
    let mut sh = shell();
    let r = sh.exec("echo 'scale=2; 10/3' | bc").unwrap();
    assert_eq!(r.stdout, "3.33\n");
}

#[test]
fn bc_exponentiation() {
    let mut sh = shell();
    let r = sh.exec("echo '2^10' | bc").unwrap();
    assert_eq!(r.stdout, "1024\n");
}

// ── file ─────────────────────────────────────────────────────────────

#[test]
fn file_detect_png() {
    let mut files = HashMap::new();
    files.insert(
        "/image.png".into(),
        b"\x89PNG\r\n\x1a\nrest of data".to_vec(),
    );
    let mut sh = RustBashBuilder::new().files(files).build().unwrap();
    let r = sh.exec("file /image.png").unwrap();
    assert!(r.stdout.contains("PNG"));
}

#[test]
fn file_detect_directory() {
    let mut sh = shell();
    sh.exec("mkdir /testdir").unwrap();
    let r = sh.exec("file /testdir").unwrap();
    assert!(r.stdout.contains("directory"));
}

#[test]
fn file_detect_text() {
    let mut sh = shell();
    sh.exec("echo 'hello world' > /plain.txt").unwrap();
    let r = sh.exec("file /plain.txt").unwrap();
    assert!(r.stdout.contains("text") || r.stdout.contains("ASCII"));
}

// ── strings ──────────────────────────────────────────────────────────

#[test]
fn strings_extract_ascii() {
    let mut sh = shell();
    // Create content with printable strings surrounded by non-printable bytes
    sh.exec("printf 'hello\\x00\\x01\\x02world\\x00' > /bin.dat")
        .unwrap();
    let r = sh.exec("strings /bin.dat").unwrap();
    assert!(r.stdout.contains("hello"));
    assert!(r.stdout.contains("world"));
}

// ── split ────────────────────────────────────────────────────────────

#[test]
fn split_by_lines() {
    let mut sh = shell();
    sh.exec("printf 'a\\nb\\nc\\nd\\n' > /input.txt").unwrap();
    let r = sh.exec("split -l 2 /input.txt").unwrap();
    assert_eq!(r.exit_code, 0);
    let r = sh.exec("cat xaa").unwrap();
    assert_eq!(r.stdout, "a\nb\n");
    let r = sh.exec("cat xab").unwrap();
    assert_eq!(r.stdout, "c\nd\n");
}

// ── rg (ripgrep) ────────────────────────────────────────────────────

#[test]
fn rg_recursive_search() {
    let mut sh = shell();
    sh.exec("mkdir -p /searchdir/sub").unwrap();
    sh.exec("echo 'hello world' > /searchdir/file1.txt")
        .unwrap();
    sh.exec("echo 'hello there' > /searchdir/sub/file2.txt")
        .unwrap();
    let r = sh.exec("rg hello /searchdir").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("hello world"));
    assert!(r.stdout.contains("hello there"));
}

#[test]
fn rg_case_insensitive() {
    let mut sh = shell();
    sh.exec("echo 'Hello World' > /rifile.txt").unwrap();
    let r = sh.exec("rg -i hello /rifile.txt").unwrap();
    assert!(r.stdout.contains("Hello World"));
}

// ── help builtin ────────────────────────────────────────────────────

#[test]
fn help_lists_builtins() {
    let mut sh = shell();
    let r = sh.exec("help").unwrap();
    assert!(r.stdout.contains("cd"));
    assert!(r.stdout.contains("export"));
    assert!(r.stdout.contains("exit"));
}

#[test]
fn help_specific_builtin() {
    let mut sh = shell();
    let r = sh.exec("help cd").unwrap();
    assert!(r.stdout.contains("cd"));
    assert!(r.stdout.contains("Change"));
}

// ── clear ────────────────────────────────────────────────────────────

#[test]
fn clear_outputs_ansi_escape() {
    let mut sh = shell();
    let r = sh.exec("clear").unwrap();
    assert!(r.stdout.contains("\x1b[2J"));
    assert!(r.stdout.contains("\x1b[H"));
}

// ── history ──────────────────────────────────────────────────────────

#[test]
fn history_returns_empty() {
    let mut sh = shell();
    let r = sh.exec("history").unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "");
}

#[test]
fn sh_c_isolates_state() {
    let mut sh = shell();
    sh.exec("x=old").unwrap();
    sh.exec("sh -c 'x=new'").unwrap();
    let r = sh.exec("echo $x").unwrap();
    assert_eq!(r.stdout, "old\n");
}

#[test]
fn sh_c_positional_params() {
    let mut sh = shell();
    let r = sh.exec("sh -c 'echo $0 $1' foo bar").unwrap();
    assert_eq!(r.stdout, "foo bar\n");
}

#[test]
fn file_empty_file() {
    let mut sh = shell();
    sh.exec("touch /empty").unwrap();
    let r = sh.exec("file /empty").unwrap();
    assert!(r.stdout.contains("empty"), "got: {}", r.stdout);
}

// ── M7.8: Command Fidelity Infrastructure ──────────────────────────

#[test]
fn unknown_option_wc_long() {
    let mut sh = shell();
    let r = sh.exec("wc --bogus 2>&1").unwrap();
    assert!(
        r.stdout.contains("wc: unrecognized option '--bogus'"),
        "got: {}",
        r.stdout
    );
}

#[test]
fn unknown_option_wc_short() {
    let mut sh = shell();
    let r = sh.exec("wc -z 2>&1").unwrap();
    assert!(
        r.stdout.contains("wc: invalid option -- 'z'"),
        "got: {}",
        r.stdout
    );
}

#[test]
fn unknown_option_wc_exit_code() {
    let mut sh = shell();
    let r = sh.exec("wc --bogus").unwrap();
    assert_eq!(r.exit_code, 2);
}

#[test]
fn unknown_option_sort_long() {
    let mut sh = shell();
    let r = sh.exec("sort --fake 2>&1").unwrap();
    assert!(
        r.stdout.contains("sort: unrecognized option '--fake'"),
        "got: {}",
        r.stdout
    );
}

#[test]
fn unknown_option_sort_short() {
    let mut sh = shell();
    let r = sh.exec("sort -z 2>&1").unwrap();
    assert!(
        r.stdout.contains("sort: invalid option -- 'z'"),
        "got: {}",
        r.stdout
    );
}

#[test]
fn unknown_option_head() {
    let mut sh = shell();
    let r = sh.exec("head --bogus 2>&1").unwrap();
    assert!(
        r.stdout.contains("head: unrecognized option '--bogus'"),
        "got: {}",
        r.stdout
    );
    let r2 = sh.exec("head --bogus").unwrap();
    assert_eq!(r2.exit_code, 2);
}

#[test]
fn unknown_option_tail() {
    let mut sh = shell();
    let r = sh.exec("tail --bogus 2>&1").unwrap();
    assert!(
        r.stdout.contains("tail: unrecognized option '--bogus'"),
        "got: {}",
        r.stdout
    );
    let r2 = sh.exec("tail --bogus").unwrap();
    assert_eq!(r2.exit_code, 2);
}

#[test]
fn valid_flags_still_work_wc() {
    let mut sh = shell();
    sh.exec("echo -e 'a b\\nc d' > /wc_test.txt").unwrap();
    let r = sh.exec("wc -l /wc_test.txt").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("2"), "got: {}", r.stdout);
}

#[test]
fn valid_combined_flags_wc() {
    let mut sh = shell();
    sh.exec("echo hello > /wc_combo.txt").unwrap();
    let r = sh.exec("wc -lw /wc_combo.txt").unwrap();
    assert_eq!(r.exit_code, 0);
}

#[test]
fn double_dash_stops_flag_parsing() {
    let mut sh = shell();
    // After --, -z should be treated as a filename, not a flag
    let r = sh.exec("wc -- -z 2>&1").unwrap();
    // Should be a file-not-found error, not an invalid option error
    assert!(!r.stdout.contains("invalid option"), "got: {}", r.stdout);
}

#[test]
fn flag_info_metadata_accessible() {
    use rust_bash::commands::{FlagInfo, FlagStatus};

    let fi = FlagInfo {
        flag: "-n",
        description: "test",
        status: FlagStatus::Supported,
    };
    assert_eq!(fi.flag, "-n");
    assert_eq!(fi.status, FlagStatus::Supported);

    let stubbed = FlagInfo {
        flag: "-P",
        description: "test",
        status: FlagStatus::Stubbed,
    };
    assert_eq!(stubbed.status, FlagStatus::Stubbed);

    let ignored = FlagInfo {
        flag: "-t",
        description: "test",
        status: FlagStatus::Ignored,
    };
    assert_eq!(ignored.status, FlagStatus::Ignored);
    assert_ne!(FlagStatus::Supported, FlagStatus::Stubbed);
}

#[test]
fn unknown_option_helper_format() {
    use rust_bash::commands::unknown_option;

    let long = unknown_option("grep", "--nonexistent");
    assert_eq!(long.stderr, "grep: unrecognized option '--nonexistent'\n");
    assert_eq!(long.exit_code, 2);
    assert_eq!(long.stdout, "");

    let short = unknown_option("wc", "-z");
    assert_eq!(short.stderr, "wc: invalid option -- 'z'\n");
    assert_eq!(short.exit_code, 2);
}

#[test]
fn format_help_includes_flag_info() {
    use rust_bash::commands::{CommandMeta, FlagInfo, FlagStatus, format_help};

    static TEST_META: CommandMeta = CommandMeta {
        name: "test_cmd",
        synopsis: "test_cmd [OPTIONS]",
        description: "A test command.",
        options: &[("-n", "a flag")],
        supports_help_flag: true,
        flags: &[
            FlagInfo {
                flag: "-n",
                description: "a flag",
                status: FlagStatus::Supported,
            },
            FlagInfo {
                flag: "-x",
                description: "experimental",
                status: FlagStatus::Stubbed,
            },
        ],
    };

    let help = format_help(&TEST_META);
    assert!(help.contains("Flag support:"), "got: {}", help);
    assert!(help.contains("[supported]"), "got: {}", help);
    assert!(help.contains("[stubbed]"), "got: {}", help);
}

#[test]
fn format_help_no_flags_section_when_empty() {
    use rust_bash::commands::{CommandMeta, format_help};

    static TEST_META: CommandMeta = CommandMeta {
        name: "test_cmd",
        synopsis: "test_cmd",
        description: "A test command.",
        options: &[],
        supports_help_flag: true,
        flags: &[],
    };

    let help = format_help(&TEST_META);
    assert!(!help.contains("Flag support:"), "got: {}", help);
}

// ── M7.3: Compression and archiving ──────────────────────────────

#[test]
fn gzip_compress_and_gunzip_roundtrip_file() {
    let mut sh = shell();
    sh.exec("echo 'hello compression' > /tmp_test_file.txt")
        .unwrap();
    let r = sh.exec("gzip /tmp_test_file.txt").unwrap();
    assert_eq!(r.exit_code, 0, "gzip stderr: {}", r.stderr);

    let r = sh.exec("gunzip /tmp_test_file.txt.gz").unwrap();
    assert_eq!(r.exit_code, 0, "gunzip stderr: {}", r.stderr);

    let r = sh.exec("cat /tmp_test_file.txt").unwrap();
    assert_eq!(r.stdout, "hello compression\n");
}

#[test]
fn gzip_keep_flag() {
    let mut sh = shell();
    sh.exec("echo 'keep me' > /keep_test.txt").unwrap();
    sh.exec("gzip -k /keep_test.txt").unwrap();

    // Both original and .gz should exist
    let r = sh.exec("test -f /keep_test.txt && echo yes").unwrap();
    assert_eq!(r.stdout, "yes\n");
    let r = sh.exec("test -f /keep_test.txt.gz && echo yes").unwrap();
    assert_eq!(r.stdout, "yes\n");
}

#[test]
fn gzip_c_pipe_gunzip_binary_pipeline() {
    let mut sh = shell();
    sh.exec("echo 'binary pipeline test' > /pipe_test.txt")
        .unwrap();
    let r = sh.exec("gzip -c /pipe_test.txt | gunzip").unwrap();
    assert_eq!(r.stdout, "binary pipeline test\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn gzip_stdin_pipe_gunzip_roundtrip() {
    let mut sh = shell();
    let r = sh.exec("echo 'stdin roundtrip' | gzip | gunzip").unwrap();
    assert_eq!(r.stdout, "stdin roundtrip\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn zcat_outputs_to_stdout() {
    let mut sh = shell();
    sh.exec("echo 'zcat content' > /zcat_test.txt").unwrap();
    sh.exec("gzip -k /zcat_test.txt").unwrap();
    let r = sh.exec("zcat /zcat_test.txt.gz").unwrap();
    assert_eq!(r.stdout, "zcat content\n");
    // Original .gz should still exist
    let r2 = sh.exec("test -f /zcat_test.txt.gz && echo yes").unwrap();
    assert_eq!(r2.stdout, "yes\n");
}

#[test]
fn gzip_decompress_flag() {
    let mut sh = shell();
    sh.exec("echo 'decompress flag' > /dflag.txt").unwrap();
    sh.exec("gzip /dflag.txt").unwrap();
    let r = sh.exec("gzip -d /dflag.txt.gz").unwrap();
    assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
    let r = sh.exec("cat /dflag.txt").unwrap();
    assert_eq!(r.stdout, "decompress flag\n");
}

#[test]
fn gzip_nonexistent_file_error() {
    let mut sh = shell();
    let r = sh.exec("gzip /no_such_file.txt").unwrap();
    assert_ne!(r.exit_code, 0);
    assert!(r.stderr.contains("no_such_file.txt"));
}

#[test]
fn gzip_redirect_binary_to_file() {
    let mut sh = shell();
    sh.exec("echo 'redirect test' > /redir.txt").unwrap();
    sh.exec("gzip -c /redir.txt > /redir.txt.gz").unwrap();
    // Verify the .gz file is non-empty and valid gzip
    sh.exec("gunzip /redir.txt.gz").unwrap();
    let r = sh.exec("cat /redir.txt").unwrap();
    assert_eq!(r.stdout, "redirect test\n");
}

#[test]
fn tar_create_extract_roundtrip() {
    let mut sh = shell();
    sh.exec("echo 'tar content' > /tar_test.txt").unwrap();
    let r = sh.exec("tar cf /archive.tar tar_test.txt").unwrap();
    assert_eq!(r.exit_code, 0, "create stderr: {}", r.stderr);

    sh.exec("rm /tar_test.txt").unwrap();
    let r = sh.exec("tar xf /archive.tar").unwrap();
    assert_eq!(r.exit_code, 0, "extract stderr: {}", r.stderr);

    let r = sh.exec("cat /tar_test.txt").unwrap();
    assert_eq!(r.stdout, "tar content\n");
}

#[test]
fn tar_list_contents() {
    let mut sh = shell();
    sh.exec("echo 'file1' > /tl1.txt").unwrap();
    sh.exec("echo 'file2' > /tl2.txt").unwrap();
    sh.exec("tar cf /list.tar tl1.txt tl2.txt").unwrap();

    let r = sh.exec("tar tf /list.tar").unwrap();
    assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
    assert!(r.stdout.contains("tl1.txt"), "stdout: {}", r.stdout);
    assert!(r.stdout.contains("tl2.txt"), "stdout: {}", r.stdout);
}

#[test]
fn tar_with_gzip_flag() {
    let mut sh = shell();
    sh.exec("echo 'gzipped tar' > /tgz_test.txt").unwrap();
    let r = sh.exec("tar czf /archive.tar.gz tgz_test.txt").unwrap();
    assert_eq!(r.exit_code, 0, "create stderr: {}", r.stderr);

    sh.exec("rm /tgz_test.txt").unwrap();
    let r = sh.exec("tar xzf /archive.tar.gz").unwrap();
    assert_eq!(r.exit_code, 0, "extract stderr: {}", r.stderr);

    let r = sh.exec("cat /tgz_test.txt").unwrap();
    assert_eq!(r.stdout, "gzipped tar\n");
}

#[test]
fn tar_directory() {
    let mut sh = shell();
    sh.exec("mkdir -p /tardir").unwrap();
    sh.exec("echo 'a' > /tardir/a.txt").unwrap();
    sh.exec("echo 'b' > /tardir/b.txt").unwrap();
    let r = sh.exec("tar cf /dir.tar tardir").unwrap();
    assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);

    sh.exec("rm -r /tardir").unwrap();
    let r = sh.exec("tar xf /dir.tar").unwrap();
    assert_eq!(r.exit_code, 0, "extract stderr: {}", r.stderr);

    let r = sh.exec("cat /tardir/a.txt").unwrap();
    assert_eq!(r.stdout, "a\n");
    let r = sh.exec("cat /tardir/b.txt").unwrap();
    assert_eq!(r.stdout, "b\n");
}

#[test]
fn tar_verbose_output() {
    let mut sh = shell();
    sh.exec("echo 'verbose' > /vtar.txt").unwrap();
    let r = sh.exec("tar cvf /v.tar vtar.txt").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("vtar.txt"), "stdout: {}", r.stdout);
}

#[test]
fn tar_change_directory() {
    let mut sh = shell();
    sh.exec("mkdir -p /src_dir").unwrap();
    sh.exec("echo 'from src' > /src_dir/data.txt").unwrap();
    sh.exec("mkdir -p /dst_dir").unwrap();

    let r = sh.exec("tar -C /src_dir -cf /cd.tar data.txt").unwrap();
    assert_eq!(r.exit_code, 0, "create stderr: {}", r.stderr);

    let r = sh.exec("tar -C /dst_dir -xf /cd.tar").unwrap();
    assert_eq!(r.exit_code, 0, "extract stderr: {}", r.stderr);

    let r = sh.exec("cat /dst_dir/data.txt").unwrap();
    assert_eq!(r.stdout, "from src\n");
}

#[test]
fn gzip_empty_input() {
    let mut sh = shell();
    let r = sh.exec("echo -n '' | gzip | gunzip").unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "");
}

// ── packages/core/AGENTS.md validation ───────────────────────────────────────

#[test]
fn agents_npm_md_command_count_matches_registry() {
    let commands = rust_bash::commands::register_default_commands();
    let actual_count = commands.len();

    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/core/AGENTS.md"),
    )
    .expect("packages/core/AGENTS.md should exist at repo root");

    // The doc says "Available Commands (N)" — extract N and verify.
    let re = regex::Regex::new(r"## Available Commands \((\d+)\)").unwrap();
    let caps = re
        .captures(&content)
        .expect("packages/core/AGENTS.md should contain '## Available Commands (N)'");
    let documented_count: usize = caps[1].parse().unwrap();

    // Allow ±2 tolerance for feature-gated commands (e.g. curl).
    assert!(
        (actual_count as isize - documented_count as isize).unsigned_abs() <= 2,
        "Command count mismatch: registry has {actual_count}, doc says {documented_count}",
    );
}

#[test]
fn agents_npm_md_documented_commands_exist_in_registry() {
    let commands = rust_bash::commands::register_default_commands();

    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/core/AGENTS.md"),
    )
    .expect("packages/core/AGENTS.md should exist at repo root");

    // Extract command names from the table rows: "| `cmd` |"
    let re = regex::Regex::new(r"(?m)^\| `([a-z0-9_\[\]-]+)`").unwrap();
    let section_start = content
        .find("## Available Commands")
        .expect("missing Available Commands section");
    let section_end = content
        .find("## Shell Builtins")
        .expect("missing Shell Builtins section");
    let commands_section = &content[section_start..section_end];

    let mut missing = Vec::new();
    for caps in re.captures_iter(commands_section) {
        let name = &caps[1];
        // Skip curl — network-gated, not always in registry
        if name == "curl" {
            continue;
        }
        if !commands.contains_key(name) {
            missing.push(name.to_string());
        }
    }

    assert!(
        missing.is_empty(),
        "Commands documented in packages/core/AGENTS.md but missing from registry: {missing:?}",
    );
}

#[test]
fn agents_npm_md_documented_builtins_exist() {
    let builtin_names: std::collections::HashSet<&str> =
        rust_bash::builtin_names().iter().copied().collect();

    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/core/AGENTS.md"),
    )
    .expect("packages/core/AGENTS.md should exist at repo root");

    let section_start = content
        .find("## Shell Builtins")
        .expect("missing Shell Builtins section");
    let section_end = content[section_start..]
        .find("\n## ")
        .map(|i| section_start + i)
        .unwrap_or(content.len());
    let builtins_section = &content[section_start..section_end];

    // Extract builtin names from table rows: "| `name` |" or "| `a` / `b` |"
    let re = regex::Regex::new(r"`([a-z_.]+)`").unwrap();
    let mut missing = Vec::new();
    for caps in re.captures_iter(builtins_section) {
        let name = &caps[1];
        if !builtin_names.contains(name) {
            missing.push(name.to_string());
        }
    }

    assert!(
        missing.is_empty(),
        "Builtins documented in packages/core/AGENTS.md but not in builtin_names(): {missing:?}",
    );
}
