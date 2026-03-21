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

pub fn run_cases(path: &Path, fixture: &FixtureFile) -> datatest_stable::Result<()> {
    let mut failures: Vec<String> = Vec::new();

    for case in &fixture.cases {
        if let Some(reason) = &case.skip {
            eprintln!("SKIP {}: {reason}", case.name);
            continue;
        }

        if !validate_case(case, &mut failures) {
            continue;
        }

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

        let mut sh = builder
            .build()
            .map_err(|e| format!("[{}] Failed to build shell: {e}", case.name))?;

        let result = if let Some(stdin_content) = &case.stdin {
            sh.exec_with_overrides(&case.script, None, None, Some(stdin_content))
        } else {
            sh.exec(&case.script)
        };

        match result {
            Ok(r) => {
                if case.expect_error {
                    failures.push(format!(
                        "[{}] expected exec() to return Err, but got Ok (exit_code={})",
                        case.name, r.exit_code
                    ));
                    continue;
                }
                if r.stdout != case.stdout {
                    failures.push(format!(
                        "[{}] STDOUT mismatch:\n  expected: {:?}\n  got:      {:?}",
                        case.name, case.stdout, r.stdout
                    ));
                }
                if r.exit_code != case.exit_code {
                    failures.push(format!(
                        "[{}] EXIT CODE mismatch: expected {}, got {}",
                        case.name, case.exit_code, r.exit_code
                    ));
                }
                check_stderr(&case.name, &r.stderr, case, &mut failures);
            }
            Err(e) => {
                if !case.expect_error {
                    failures.push(format!("[{}] exec() returned Err: {e}", case.name));
                }
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

fn check_stderr(name: &str, actual: &str, case: &TestCase, failures: &mut Vec<String>) {
    if case.stderr_ignore {
        return;
    }
    if let Some(substring) = &case.stderr_contains {
        if !actual.contains(substring.as_str()) {
            failures.push(format!(
                "[{name}] STDERR does not contain {substring:?}, got {actual:?}"
            ));
        }
    } else if actual != case.stderr {
        failures.push(format!(
            "[{name}] STDERR mismatch:\n  expected: {:?}\n  got:      {actual:?}",
            case.stderr
        ));
    }
}
