//! SQLite-compatible test runner for sqllite-rust.

use anyhow::bail;
use clap::Parser;
use sqllite_tests::{run_tests, TestResults};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sqllite-test", about = "Run SQLite-compatible tests")]
struct Args {
    /// Test file or directory to run
    #[arg(default_value = "tests")]
    path: PathBuf,

    /// Only run tests matching this pattern
    #[arg(short, long)]
    pattern: Option<String>,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let results = run_tests(
        &args.path,
        args.pattern.as_deref(),
        args.verbose,
    )?;
    print_summary(&results);
    if results.failed > 0 {
        bail!("{} test(s) failed", results.failed);
    }
    Ok(())
}

fn print_summary(results: &TestResults) {
    println!(
        "Tests: {} passed, {} failed, {} skipped",
        results.passed, results.failed, results.skipped
    );
}
