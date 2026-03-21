// Shared test module items are used by different test binaries (comparison, spec_tests,
// oils_spec) — not every binary uses every item, so dead_code warnings are false positives.
#![allow(dead_code)]

pub mod oils_format;

use std::collections::HashMap;
use std::path::Path;

use rust_bash::{ExecutionLimits, RustBashBuilder};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Fixture data model
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct FixtureFile {
    pub cases: Vec<TestCase>,
}

/// Test case status: pass (default), xfail, or skip.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CaseStatus {
    #[default]
    Pass,
    Xfail,
    Skip,
}

#[derive(Deserialize)]
pub struct TestCase {
    pub name: String,
    pub script: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default)]
    pub stderr_contains: Option<String>,
    #[serde(default)]
    pub stderr_ignore: bool,
    #[serde(default)]
    pub stdin: Option<String>,
    #[serde(default)]
    pub expect_error: bool,
    #[serde(default)]
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub skip: Option<String>,
    #[serde(default)]
    pub status: CaseStatus,
    #[serde(default)]
    pub milestone: Option<String>,
    #[serde(default)]
    pub feature: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Controlled environment shared by all runners
// ---------------------------------------------------------------------------

pub fn base_env() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("HOME".into(), "/root".into());
    m.insert("USER".into(), "testuser".into());
    m.insert("TZ".into(), "UTC".into());
    m.insert("LC_ALL".into(), "C".into());
    m.insert("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into());
    m
}

// ---------------------------------------------------------------------------
// Shared test execution logic
// ---------------------------------------------------------------------------

pub fn load_fixture(path: &Path) -> datatest_stable::Result<FixtureFile> {
    let content = std::fs::read_to_string(path)?;
    let fixture: FixtureFile =
        toml::from_str(&content).map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
    Ok(fixture)
}

/// Outcome of running a single test case.
#[derive(Debug)]
enum CaseOutcome {
    Pass {
        milestone: Option<String>,
    },
    ExpectedFail {
        milestone: Option<String>,
    },
    UnexpectedPass {
        name: String,
        milestone: Option<String>,
    },
    Fail {
        message: String,
    },
    Skip {
        milestone: Option<String>,
    },
}

pub fn run_cases(path: &Path, fixture: &FixtureFile) -> datatest_stable::Result<()> {
    let mut outcomes: Vec<CaseOutcome> = Vec::new();

    for case in &fixture.cases {
        // Handle legacy skip field
        if let Some(reason) = &case.skip {
            eprintln!("SKIP {}: {reason}", case.name);
            outcomes.push(CaseOutcome::Skip {
                milestone: case.milestone.clone(),
            });
            continue;
        }

        // Handle status = "skip"
        if case.status == CaseStatus::Skip {
            let reason = case.reason.as_deref().unwrap_or("no reason given");
            eprintln!("SKIP {}: {reason}", case.name);
            outcomes.push(CaseOutcome::Skip {
                milestone: case.milestone.clone(),
            });
            continue;
        }

        let mut validation_failures: Vec<String> = Vec::new();
        if !validate_case(case, &mut validation_failures) {
            for msg in validation_failures {
                outcomes.push(CaseOutcome::Fail { message: msg });
            }
            continue;
        }

        let mismatch = execute_and_compare(case);

        match (&case.status, &mismatch) {
            // pass + match => pass
            (CaseStatus::Pass, None) => {
                outcomes.push(CaseOutcome::Pass {
                    milestone: case.milestone.clone(),
                });
            }
            // pass + mismatch => fail
            (CaseStatus::Pass, Some(msg)) => {
                outcomes.push(CaseOutcome::Fail {
                    message: msg.clone(),
                });
            }
            // xfail + mismatch => expected failure (ok)
            (CaseStatus::Xfail, Some(_)) => {
                let reason = case.reason.as_deref().unwrap_or("expected failure");
                let feat = case.feature.as_deref().unwrap_or("unknown");
                eprintln!("XFAIL {} [{}]: {reason}", case.name, feat);
                outcomes.push(CaseOutcome::ExpectedFail {
                    milestone: case.milestone.clone(),
                });
            }
            // xfail + match => unexpected pass (fail to force promotion)
            (CaseStatus::Xfail, None) => {
                eprintln!(
                    "UNEXPECTED PASS {}: marked xfail but matches bash — promote to pass",
                    case.name
                );
                outcomes.push(CaseOutcome::UnexpectedPass {
                    name: case.name.clone(),
                    milestone: case.milestone.clone(),
                });
            }
            // skip handled above
            (CaseStatus::Skip, _) => unreachable!(),
        }
    }

    print_summary(path, &outcomes);

    // Collect hard failures
    let mut failures: Vec<String> = Vec::new();
    for outcome in &outcomes {
        match outcome {
            CaseOutcome::Fail { message } => failures.push(message.clone()),
            CaseOutcome::UnexpectedPass { name, .. } => {
                failures.push(format!(
                    "[{name}] UNEXPECTED PASS: marked xfail but output matches bash — promote to status = \"pass\""
                ));
            }
            _ => {}
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

/// Execute a test case and return None if it matches expected, or Some(error_message) on mismatch.
fn execute_and_compare(case: &TestCase) -> Option<String> {
    let mut env_map = base_env();
    env_map.extend(case.env.clone());

    let file_map: HashMap<String, Vec<u8>> = case
        .files
        .iter()
        .map(|(k, v)| (k.clone(), v.as_bytes().to_vec()))
        .collect();

    let mut builder = RustBashBuilder::new()
        .env(env_map)
        .execution_limits(ExecutionLimits {
            max_loop_iterations: 10_000,
            max_execution_time: std::time::Duration::from_secs(5),
            ..ExecutionLimits::default()
        });

    if !file_map.is_empty() {
        builder = builder.files(file_map);
    }

    let mut sh = match builder.build() {
        Ok(sh) => sh,
        Err(e) => {
            return if case.expect_error {
                None
            } else {
                Some(format!("[{}] Failed to build shell: {e}", case.name))
            };
        }
    };

    let result = if let Some(stdin_content) = &case.stdin {
        sh.exec_with_overrides(&case.script, None, None, Some(stdin_content))
    } else {
        sh.exec(&case.script)
    };

    match result {
        Ok(r) => {
            if case.expect_error {
                return Some(format!(
                    "[{}] expected exec() to return Err, but got Ok (exit_code={})",
                    case.name, r.exit_code
                ));
            }
            let mut mismatches: Vec<String> = Vec::new();
            if r.stdout != case.stdout {
                mismatches.push(format!(
                    "[{}] STDOUT mismatch:\n  expected: {:?}\n  got:      {:?}",
                    case.name, case.stdout, r.stdout
                ));
            }
            if r.exit_code != case.exit_code {
                mismatches.push(format!(
                    "[{}] EXIT CODE mismatch: expected {}, got {}",
                    case.name, case.exit_code, r.exit_code
                ));
            }
            if let Some(msg) = check_stderr_mismatch(&case.name, &r.stderr, case) {
                mismatches.push(msg);
            }
            if mismatches.is_empty() {
                None
            } else {
                Some(mismatches.join("\n"))
            }
        }
        Err(e) => {
            if case.expect_error {
                None
            } else {
                Some(format!("[{}] exec() returned Err: {e}", case.name))
            }
        }
    }
}

fn validate_case(case: &TestCase, failures: &mut Vec<String>) -> bool {
    if case.stderr_ignore && case.stderr_contains.is_some() {
        failures.push(format!(
            "[{}] fixture error: cannot set both stderr_ignore and stderr_contains",
            case.name
        ));
        return false;
    }
    true
}

fn check_stderr_mismatch(name: &str, actual: &str, case: &TestCase) -> Option<String> {
    if case.stderr_ignore {
        return None;
    }
    if let Some(substring) = &case.stderr_contains {
        if !actual.contains(substring.as_str()) {
            return Some(format!(
                "[{name}] STDERR does not contain {substring:?}, got {actual:?}"
            ));
        }
    } else if actual != case.stderr {
        return Some(format!(
            "[{name}] STDERR mismatch:\n  expected: {:?}\n  got:      {actual:?}",
            case.stderr
        ));
    }
    None
}

/// Print a per-milestone summary at the end of each fixture file.
fn print_summary(path: &Path, outcomes: &[CaseOutcome]) {
    use std::collections::BTreeMap;

    let total = outcomes.len();
    if total == 0 {
        return;
    }

    let mut by_milestone: BTreeMap<String, (usize, usize, usize, usize)> = BTreeMap::new();
    let (mut pass_total, mut xfail_total, mut skip_total, mut upass_total) = (0, 0, 0, 0);

    for outcome in outcomes {
        let ms = match outcome {
            CaseOutcome::Pass { milestone } => {
                pass_total += 1;
                milestone.clone()
            }
            CaseOutcome::ExpectedFail { milestone } => {
                xfail_total += 1;
                milestone.clone()
            }
            CaseOutcome::Skip { milestone } => {
                skip_total += 1;
                milestone.clone()
            }
            CaseOutcome::UnexpectedPass { milestone, .. } => {
                upass_total += 1;
                milestone.clone()
            }
            CaseOutcome::Fail { .. } => {
                // Failures are printed separately
                None
            }
        };
        if let Some(ms) = ms {
            let entry = by_milestone.entry(ms).or_insert((0, 0, 0, 0));
            match outcome {
                CaseOutcome::Pass { .. } => entry.0 += 1,
                CaseOutcome::ExpectedFail { .. } => entry.1 += 1,
                CaseOutcome::Skip { .. } => entry.2 += 1,
                CaseOutcome::UnexpectedPass { .. } => entry.3 += 1,
                _ => {}
            }
        }
    }

    let fail_total = outcomes
        .iter()
        .filter(|o| matches!(o, CaseOutcome::Fail { .. }))
        .count();

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    eprintln!(
        "--- {file_stem}: {pass_total} pass, {xfail_total} xfail, {skip_total} skip, {upass_total} unexpected-pass, {fail_total} fail ({total} total)"
    );

    for (ms, (p, x, s, u)) in &by_milestone {
        eprintln!("    {ms}: {p} pass, {x} xfail, {s} skip, {u} unexpected-pass");
    }
}
