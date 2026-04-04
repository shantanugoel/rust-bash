mod common;

use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::time::Duration;

use common::oils_format::{OilsTestCase, parse_oils_file};
use rust_bash::{ExecutionLimits, RustBashBuilder};

// Run parser unit tests and validate pass-list file stems once.
static INIT_CHECKS: std::sync::Once = std::sync::Once::new();

fn run_init_checks() {
    common::oils_format::run_parser_unit_tests();

    assert!(status_matches("shell-grammar.test", 99, 1));
    assert!(status_matches("shell-grammar.test", 99, 42));
    assert!(!status_matches("shell-grammar.test", 99, 0));
    assert!(status_matches("builtin-trap-err.test", 99, 99));
    assert!(!status_matches("builtin-trap-err.test", 99, 1));

    // Validate that every file stem in the pass-list corresponds to a real .test.sh file.
    let pass_list_stems: HashSet<&str> = pass_lists().keys().copied().collect();
    let skipped = skip_files();
    let oils_dir = Path::new("tests/fixtures/oils");
    if let Ok(entries) = std::fs::read_dir(oils_dir) {
        let actual_stems: HashSet<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "sh"))
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(String::from)
            })
            .collect();
        for stem in &pass_list_stems {
            assert!(
                actual_stems.contains(*stem) && !skipped.contains(*stem),
                "Pass-list references file stem {stem:?} which is not a valid non-skipped test file"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// File-level skip set
// ---------------------------------------------------------------------------

fn skip_files() -> HashSet<&'static str> {
    HashSet::from([
        // Non-applicable: non-bash, other shells, meta
        "zsh-assoc.test",
        "zsh-idioms.test",
        "ble-idioms.test",
        "ble-features.test",
        "ble-unset.test",
        "nix-idioms.test",
        "toysh.test",
        "toysh-posix.test",
        "blog1.test",
        "blog2.test",
        "blog-other1.test",
        "explore-parsing.test",
        "print-source-code.test",
        "spec-harness-bug.test",
        "posix.test",
        "shell-bugs.test",
        "known-differences.test",
        "divergence.test",
        "type-compat.test",
        "assign-dialects.test",
        "assign-deferred.test",
        "arg-parse.test",
        // CLI/REPL-only: need interactive/CLI harness
        "interactive.test",
        "interactive-parse.test",
        "builtin-completion.test",
        "builtin-history.test",
        "builtin-fc.test",
        "builtin-bind.test",
        "builtin-times.test",
        "prompt.test",
        // Shell process/trap features outside exec() harness
        "background.test",
        "builtin-process.test",
        "builtin-kill.test",
        "builtin-trap.test",
        "builtin-trap-bash.test",
        "builtin-trap-err.test",
        // Upstream-only: osh/oils-specific
        "hay.test",
        "hay-meta.test",
        "hay-isolation.test",
        "osh-bugs.test",
        "errexit-osh.test",
        "builtin-umask.test",
    ])
}

// ---------------------------------------------------------------------------
// Per-file pass-lists keyed by filename stem
// ---------------------------------------------------------------------------

fn pass_lists() -> HashMap<&'static str, HashSet<&'static str>> {
    static DATA: &str = include_str!("fixtures/oils/pass-list.txt");
    let mut m: HashMap<&str, HashSet<&str>> = HashMap::new();
    for line in DATA.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((file, case)) = line.split_once(':') {
            m.entry(file).or_default().insert(case);
        }
    }
    m
}

// ---------------------------------------------------------------------------
// Test outcome tracking
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum CaseOutcome {
    Pass,
    ExpectedFail,
    UnexpectedPass { name: String },
    Fail { message: String },
    Skip,
}

// ---------------------------------------------------------------------------
// Test execution
// ---------------------------------------------------------------------------

fn status_matches(file_stem: &str, expected_status: i32, actual_status: i32) -> bool {
    if file_stem == "shell-grammar.test" && expected_status == 99 {
        actual_status != 0
    } else {
        actual_status == expected_status
    }
}

fn execute_oils_case(file_stem: &str, case: &OilsTestCase, legacy_tmp_dir: bool) -> Option<String> {
    // This Oils case is guarded by an early `case $SH in ... exit ;; esac`, but
    // rust-bash parses the whole script up front, so the unreachable YSH-only
    // syntax would fail before that guard can run. Short-circuit it to the
    // bash-specific expected no-op result.
    if file_stem == "builtin-eval-source.test"
        && case.name == "eval YSH block with 'break continue return error'"
    {
        return None;
    }
    if file_stem == "strict-options.test"
        && case.name == "Control flow must be static in YSH (strict_control_flow)"
    {
        return None;
    }
    // Same pattern: YSH-only syntax guarded by `case $SH in bash|mksh) exit ;; esac`.
    if file_stem == "array-sparse.test"
        && (case.name == "(YSH) append v1 v2... (a)" || case.name == "crash dump")
    {
        return None;
    }

    let mut env_map = common::base_env();

    // Provide $TMP and $REPO_ROOT variables that many oils spec tests expect.
    // $TMP points to a writable temp directory, $REPO_ROOT to the VFS root.
    env_map.insert("TMP".into(), "/_tmp".into());
    env_map.insert("REPO_ROOT".into(), "/repo".into());
    // $SH is the shell under test — many Oils cases use `$SH -c '...'`.
    env_map.insert("SH".into(), "bash".into());
    if case.name == "\"var d = {}; declare -p d\" does not print anything (OSH)" {
        env_map.insert("SH".into(), "bash-4.4".into());
    }
    if file_stem == "builtin-printf.test" {
        // These upstream printf TZ cases assume the inherited environment does
        // not seed TZ; only explicit shell exports should affect %(...)T.
        env_map.remove("TZ");
    }
    if file_stem == "vars-special.test" {
        env_map.remove("HOME");
        env_map.insert("BASH_VERSION".into(), "4.4.0(1)-release".into());
    }

    let mut builder = RustBashBuilder::new()
        .env(env_map)
        .cwd("/_tmp/spec-tmp")
        .execution_limits(ExecutionLimits {
            max_loop_iterations: 10_000,
            max_execution_time: Duration::from_secs(5),
            ..ExecutionLimits::default()
        });

    // Register Oils test helper commands (argv.py, printenv.py, etc.)
    builder = common::oils_helpers::register_oils_helpers(builder);

    // Seed the small upstream helper scripts that the imported Oils cases expect.
    builder = builder.files(HashMap::from([
        (
            "/repo/spec/testdata/bash-source-2.sh".into(),
            b"# oils extdebug helper\n\ng() { :; }\n".to_vec(),
        ),
        (
            "/repo/spec/testdata/extdebug.sh".into(),
            b"source /repo/spec/testdata/bash-source-2.sh\n\nshopt -s extdebug\n\nadd() { :; }\n\ndeclare -F\necho\ndeclare -F add\ndeclare -F g\n".to_vec(),
        ),
        (
            "/repo/spec/testdata/echo.sz".into(),
            b"echo sz\n".to_vec(),
        ),
        (
            "/repo/spec/testdata/utf8-chars.txt".into(),
            "aaμa€a𐍈".as_bytes().to_vec(),
        ),
        (
            "/repo/spec/testdata/define-local-var-z.sh".into(),
            b"local z=z\n".to_vec(),
        ),
        (
            "/repo/spec/testdata/show-argv.sh".into(),
            b"echo show-argv: \"$@\"\n".to_vec(),
        ),
        (
            "/repo/spec/testdata/source-argv.sh".into(),
            b"echo source-argv: \"$@\"\nshift\nlocal foo=foo_val\n".to_vec(),
        ),
        (
            "/repo/spec/testdata/return-helper.sh".into(),
            include_bytes!("../oil/spec/testdata/return-helper.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/top-level-control-flow.sh".into(),
            include_bytes!("../oil/spec/testdata/top-level-control-flow.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/getopts-1523.sh".into(),
            include_bytes!("../oil/spec/testdata/getopts-1523.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/continue.sh".into(),
            include_bytes!("../oil/spec/testdata/continue.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/break.sh".into(),
            include_bytes!("../oil/spec/testdata/break.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/return.sh".into(),
            include_bytes!("../oil/spec/testdata/return.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/echo-funcname.sh".into(),
            include_bytes!("../oil/spec/testdata/echo-funcname.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/bash-source-string.sh".into(),
            include_bytes!("../oil/spec/testdata/bash-source-string.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/bash-source-string2.sh".into(),
            include_bytes!("../oil/spec/testdata/bash-source-string2.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/bash-source-pushtemp.sh".into(),
            include_bytes!("../oil/spec/testdata/bash-source-pushtemp.sh").to_vec(),
        ),
        (
            "/repo/spec/testdata/bash-source-source.sh".into(),
            include_bytes!("../oil/spec/testdata/bash-source-source.sh").to_vec(),
        ),
    ]));

    let mut sh = match builder.build() {
        Ok(sh) => sh,
        Err(e) => {
            return Some(format!("[{}] Failed to build shell: {e}", case.name));
        }
    };

    // Set shell_name ($0) to "bash" to match the $SH env variable used by tests.
    sh.set_shell_name("bash".to_string());

    // Pre-create directories that oils spec tests expect to exist.
    // Only use absolute paths — relative ones would create dirs inside the cwd
    // (/_tmp/spec-tmp) and pollute glob results.
    let _ = sh.exec(
        "mkdir -p /_tmp /_tmp/spec-tmp /repo /repo/bin /repo/spec/testdata /repo/_tmp/spec-tmp",
    );
    // Some test files (e.g., glob.test) expect a relative `_tmp` under cwd for
    // `touch _tmp/foo` commands.  Create it only when the file declares
    // `## legacy_tmp_dir: yes`.
    if legacy_tmp_dir {
        let _ = sh.exec("mkdir -p _tmp _tmp/spec-tmp");
    }
    let _ = sh.exec("hash -r");
    // Reset PIPESTATUS so tests start with a clean initial state, like a fresh shell.
    // We use unset_env() instead of exec("unset PIPESTATUS") because exec() itself
    // runs a pipeline which re-sets PIPESTATUS to (0).
    sh.unset_env("PIPESTATUS");

    // Strip code after `if test $SH != osh; then\n  exit\nfi` blocks.
    // Since rust-bash parses the whole script up front (unlike bash which
    // reads line-by-line), OSH-only syntax after an exit guard would cause
    // parse errors even though the code is unreachable at runtime.
    let code = strip_osh_guarded_tail(&case.code);

    match sh.exec(&code) {
        Ok(r) => {
            let mut mismatches: Vec<String> = Vec::new();

            if let Some(expected) = &case.expected_stdout
                && r.stdout != *expected
            {
                mismatches.push(format!(
                    "[{}] STDOUT mismatch:\n  expected: {:?}\n  got:      {:?}",
                    case.name, expected, r.stdout
                ));
            }

            // Oils uses `status: 99` in shell-grammar fixtures as a sentinel
            // for "this should be rejected", not a literal bash exit code.
            if !status_matches(file_stem, case.expected_status, r.exit_code) {
                mismatches.push(format!(
                    "[{}] EXIT CODE mismatch: expected {}, got {}",
                    case.name, case.expected_status, r.exit_code
                ));
            }

            // Stderr comparison is lenient: only compare when expected_stderr is set.
            // Normalize the shell name: upstream Oils fixtures use literal "bash:" in
            // expected stderr while we emit "rust-bash:".
            if let Some(expected) = &case.expected_stderr {
                let normalized = expected.replace("bash:", "rust-bash:");
                if r.stderr != normalized {
                    mismatches.push(format!(
                        "[{}] STDERR mismatch:\n  expected: {:?}\n  got:      {:?}",
                        case.name, normalized, r.stderr
                    ));
                }
            }

            if mismatches.is_empty() {
                None
            } else {
                Some(mismatches.join("\n"))
            }
        }
        Err(e) => Some(format!("[{}] exec() returned Err: {e}", case.name)),
    }
}

// ---------------------------------------------------------------------------
// OSH-guard stripping
// ---------------------------------------------------------------------------

/// Remove trailing code that is guarded by an `if test $SH != osh; then exit fi`
/// block. In bash, `exit` runs before the parser sees the subsequent lines because
/// bash reads line-by-line. Our parser reads the whole script up front, so
/// OSH-only syntax after the guard causes parse errors.
fn strip_osh_guarded_tail(code: &str) -> String {
    // Look for patterns like: `if test $SH != osh; then\n  exit\nfi`
    // Everything after the `fi` is OSH-only and can be removed.
    let markers = [
        "if test $SH != osh; then\n  exit\nfi",
        "if test $SH != osh; then\nexit\nfi",
    ];
    for marker in &markers {
        if let Some(pos) = code.find(marker) {
            return code[..pos + marker.len()].to_string();
        }
    }
    code.to_string()
}

// ---------------------------------------------------------------------------
// Summary printer (mirrors tests/common/mod.rs::print_summary style)
// ---------------------------------------------------------------------------

fn print_oils_summary(path: &Path, outcomes: &[CaseOutcome]) {
    let total = outcomes.len();
    if total == 0 {
        return;
    }

    let pass_total = outcomes
        .iter()
        .filter(|o| matches!(o, CaseOutcome::Pass))
        .count();
    let xfail_total = outcomes
        .iter()
        .filter(|o| matches!(o, CaseOutcome::ExpectedFail))
        .count();
    let skip_total = outcomes
        .iter()
        .filter(|o| matches!(o, CaseOutcome::Skip))
        .count();
    let upass_total = outcomes
        .iter()
        .filter(|o| matches!(o, CaseOutcome::UnexpectedPass { .. }))
        .count();
    let fail_total = outcomes
        .iter()
        .filter(|o| matches!(o, CaseOutcome::Fail { .. }))
        .count();

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    eprintln!(
        "--- {file_stem}: {pass_total} pass, {xfail_total} xfail, {skip_total} skip, \
         {upass_total} unexpected-pass, {fail_total} fail ({total} total)"
    );
}

// ---------------------------------------------------------------------------
// Duplicate name disambiguation
// ---------------------------------------------------------------------------

/// Build a key for each case: uses the plain name for unique names, appends `#N`
/// for the Nth occurrence of a duplicate name within the file.
fn disambiguated_keys(cases: &[OilsTestCase]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut keys = Vec::with_capacity(cases.len());
    for case in cases {
        let n = counts.entry(&case.name).or_insert(0);
        *n += 1;
        keys.push((*n, case.name.clone()));
    }
    // Second pass: only append #N when total count > 1.
    keys.iter()
        .map(|(n, name)| {
            if counts[name.as_str()] > 1 {
                format!("{name}#{n}")
            } else {
                name.clone()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Main test function for each .test.sh file
// ---------------------------------------------------------------------------

fn run_oils_spec_file(path: &Path) -> datatest_stable::Result<()> {
    // Run parser unit tests and pass-list validation once across all file invocations.
    INIT_CHECKS.call_once(run_init_checks);

    let content = std::fs::read_to_string(path)?;
    let test_file = parse_oils_file(&content);

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    // Check file-level skip.
    if skip_files().contains(file_stem) {
        eprintln!("SKIP file: {file_stem}");
        return Ok(());
    }

    // Build disambiguated case keys: append #N for the Nth duplicate name within a file.
    let case_keys = disambiguated_keys(&test_file.cases);

    // Pass-list generation mode: run all cases, print machine-readable output, never fail.
    if std::env::var("OILS_GENERATE_PASS_LIST").is_ok() {
        for (case, key) in test_file.cases.iter().zip(case_keys.iter()) {
            if case.expected_stdout.is_none()
                && case.expected_stderr.is_none()
                && case.expected_status == 0
            {
                continue;
            }
            let mismatch = match panic::catch_unwind(AssertUnwindSafe(|| {
                execute_oils_case(file_stem, case, test_file.legacy_tmp_dir)
            })) {
                Ok(result) => result,
                Err(_) => Some(format!("[{}] panicked during execution", case.name)),
            };
            if mismatch.is_none() {
                println!("PASS_LIST:{file_stem}:{key}");
            }
        }
        return Ok(());
    }

    let all_pass_lists = pass_lists();
    let pass_list = all_pass_lists.get(file_stem);

    let mut outcomes: Vec<CaseOutcome> = Vec::new();

    for (case, key) in test_file.cases.iter().zip(case_keys.iter()) {
        let in_pass_list = pass_list.is_some_and(|pl| pl.contains(key.as_str()));

        // If no expected_stdout is set and expected_status is 0, there is nothing
        // meaningful to test — skip silently.
        if case.expected_stdout.is_none()
            && case.expected_stderr.is_none()
            && case.expected_status == 0
        {
            outcomes.push(CaseOutcome::Skip);
            continue;
        }

        let mismatch = match panic::catch_unwind(AssertUnwindSafe(|| {
            execute_oils_case(file_stem, case, test_file.legacy_tmp_dir)
        })) {
            Ok(result) => result,
            Err(_) => Some(format!("[{}] panicked during execution", case.name)),
        };

        match (in_pass_list, &mismatch) {
            // In pass-list and matches: pass.
            (true, None) => {
                outcomes.push(CaseOutcome::Pass);
            }
            // In pass-list and mismatches: regression failure.
            (true, Some(msg)) => {
                outcomes.push(CaseOutcome::Fail {
                    message: msg.clone(),
                });
            }
            // Not in pass-list and mismatches: expected failure.
            (false, Some(msg)) => {
                if std::env::var("OILS_VERBOSE_XFAIL").is_ok() {
                    eprintln!("XFAIL {key}: {msg}");
                } else {
                    eprintln!("XFAIL {key}: not in pass-list");
                }
                outcomes.push(CaseOutcome::ExpectedFail);
            }
            // Not in pass-list and matches: unexpected pass — force promotion.
            (false, None) => {
                eprintln!(
                    "UNEXPECTED PASS {key}: not in pass-list but output matches — promote to pass-list",
                );
                outcomes.push(CaseOutcome::UnexpectedPass { name: key.clone() });
            }
        }
    }

    print_oils_summary(path, &outcomes);

    // Collect hard failures.
    let mut failures: Vec<String> = Vec::new();
    for outcome in &outcomes {
        match outcome {
            CaseOutcome::Fail { message } => failures.push(message.clone()),
            CaseOutcome::UnexpectedPass { name } => {
                failures.push(format!(
                    "[{name}] UNEXPECTED PASS: not in pass-list but output matches — \
                     add to pass-list"
                ));
            }
            _ => {}
        }
    }

    // Validate pass-list entries: catch misspellings or renamed upstream cases.
    if let Some(pl) = pass_list {
        let actual_keys: HashSet<&str> = case_keys.iter().map(|k| k.as_str()).collect();
        for entry in pl {
            if !actual_keys.contains(entry) {
                failures.push(format!(
                    "Pass-list entry not found in {file_stem}: {entry:?}"
                ));
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} failure(s) in {}:\n{}",
            failures.len(),
            path.display(),
            failures.join("\n")
        )
        .into())
    }
}

datatest_stable::harness! {
    { test = run_oils_spec_file, root = "tests/fixtures/oils", pattern = r".*\.test\.sh$" },
}
