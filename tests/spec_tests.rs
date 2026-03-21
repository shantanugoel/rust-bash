mod common;

use std::path::Path;

use common::{load_fixture, run_cases};

// ---------------------------------------------------------------------------
// Spec test runner
//
// Structurally identical to the comparison runner but:
//   - reads from tests/fixtures/spec/
//   - does NOT support recording mode
// ---------------------------------------------------------------------------

fn run_spec_file(path: &Path) -> datatest_stable::Result<()> {
    let fixture = load_fixture(path)?;
    run_cases(path, &fixture)
}

// ---------------------------------------------------------------------------
// Harness registration
// ---------------------------------------------------------------------------

datatest_stable::harness! {
    { test = run_spec_file, root = "tests/fixtures/spec", pattern = r".*\.toml$" },
}
