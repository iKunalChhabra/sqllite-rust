//! Integration test: run the full ported SQLite regression suite.

use sqllite_tests::run_tests;
use std::path::Path;

#[test]
fn run_all_sqllite_tests() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let tests_dir = Path::new(manifest_dir).join("../tests");
    let results = run_tests(&tests_dir, None, false).expect("test runner failed");
    eprintln!(
        "SQLite tests: {} passed, {} failed, {} skipped",
        results.passed, results.failed, results.skipped
    );
    assert_eq!(
        results.failed, 0,
        "{} SQLite-compatible test(s) failed",
        results.failed
    );
}
