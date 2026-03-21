mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{FixtureFile, base_env, load_fixture, run_cases};

// ---------------------------------------------------------------------------
// Comparison test runner
// ---------------------------------------------------------------------------

fn run_comparison_file(path: &Path) -> datatest_stable::Result<()> {
    let fixture = load_fixture(path)?;

    if std::env::var("RECORD_FIXTURES").is_ok_and(|v| v == "1") {
        record_fixture(path, &fixture)?;
        return Ok(());
    }

    run_cases(path, &fixture)
}

// ---------------------------------------------------------------------------
// Recording mode
// ---------------------------------------------------------------------------

const RECORD_TIMEOUT: Duration = Duration::from_secs(10);

fn record_fixture(path: &Path, fixture: &FixtureFile) -> datatest_stable::Result<()> {
    use std::process::Command;
    use toml_edit::DocumentMut;

    let content = std::fs::read_to_string(path)?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("toml_edit parse error for {}: {e}", path.display()))?;

    let cases_array = doc["cases"]
        .as_array_of_tables_mut()
        .ok_or_else(|| format!("No [[cases]] table in {}", path.display()))?;

    eprintln!(
        "RECORDING MODE: updating {} cases in {}",
        fixture.cases.len(),
        path.display()
    );

    for (i, case) in fixture.cases.iter().enumerate() {
        if case.skip.is_some()
            || case.status == common::CaseStatus::Xfail
            || case.status == common::CaseStatus::Skip
        {
            let reason = case
                .skip
                .as_deref()
                .or(case.reason.as_deref())
                .unwrap_or("xfail");
            eprintln!("SKIP recording for {}: {reason}", case.name);
            continue;
        }

        let tmp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;

        // Stage VFS files into a real temp directory for bash to access.
        for (vfs_path, file_content) in &case.files {
            let rel_path = vfs_path.strip_prefix('/').unwrap_or(vfs_path);
            let full_path = tmp.path().join(rel_path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            std::fs::write(&full_path, file_content)
                .map_err(|e| format!("write {}: {e}", full_path.display()))?;
        }

        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c").arg(&case.script);
        cmd.current_dir(tmp.path());
        cmd.env_clear();
        for (k, v) in base_env() {
            cmd.env(&k, &v);
        }
        for (k, v) in &case.env {
            cmd.env(k, v);
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = if let Some(stdin_data) = &case.stdin {
            cmd.stdin(std::process::Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| format!("spawn bash: {e}"))?;
            if let Some(mut child_stdin) = child.stdin.take() {
                use std::io::Write;
                child_stdin
                    .write_all(stdin_data.as_bytes())
                    .map_err(|e| format!("write stdin: {e}"))?;
            }
            wait_with_timeout(child, &case.name)?
        } else {
            let child = cmd.spawn().map_err(|e| format!("spawn bash: {e}"))?;
            wait_with_timeout(child, &case.name)?
        };

        update_case_in_doc(
            cases_array,
            i,
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
            output.status.code().unwrap_or(-1),
        );

        eprintln!("RECORDED {}", case.name);
    }

    std::fs::write(path, doc.to_string()).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

fn wait_with_timeout(
    mut child: std::process::Child,
    case_name: &str,
) -> datatest_stable::Result<std::process::Output> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return child.wait_with_output().map_err(|e| e.to_string().into()),
            Ok(None) if start.elapsed() > RECORD_TIMEOUT => {
                let _ = child.kill();
                return Err(
                    format!("[{case_name}] recording timed out after {RECORD_TIMEOUT:?}").into(),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("[{case_name}] wait error: {e}").into()),
        }
    }
}

fn update_case_in_doc(
    cases: &mut toml_edit::ArrayOfTables,
    index: usize,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) {
    let table = cases
        .get_mut(index)
        .expect("case index out of bounds in TOML document");
    table["stdout"] = toml_edit::value(stdout);
    table["stderr"] = toml_edit::value(stderr);
    table["exit_code"] = toml_edit::value(i64::from(exit_code));
}

// ---------------------------------------------------------------------------
// Harness registration
// ---------------------------------------------------------------------------

datatest_stable::harness! {
    { test = run_comparison_file, root = "tests/fixtures/comparison", pattern = r".*\.toml$" },
}
