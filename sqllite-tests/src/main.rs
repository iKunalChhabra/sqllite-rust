//! SQLite-compatible test runner for sqllite-rust.
//!
//! Parses a subset of SQLite's TCL test format and runs tests against our engine.

use anyhow::{bail, Result};
use clap::Parser;
use sqllite_core::Connection;
use std::fs;
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

fn main() -> Result<()> {
    let args = Args::parse();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    if args.path.is_dir() {
        let mut files: Vec<_> = fs::read_dir(&args.path)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "test").unwrap_or(false))
            .collect();
        files.sort();
        for file in files {
            if let Some(ref pat) = args.pattern {
                if !file.to_string_lossy().contains(pat) {
                    continue;
                }
            }
            match run_test_file(&file, args.verbose) {
                Ok((p, f, s)) => {
                    passed += p;
                    failed += f;
                    skipped += s;
                }
                Err(e) => {
                    eprintln!("Error running {}: {e}", file.display());
                    failed += 1;
                }
            }
        }
    } else {
        let (p, f, s) = run_test_file(&args.path, args.verbose)?;
        passed = p;
        failed = f;
        skipped = s;
    }

    println!("Tests: {passed} passed, {failed} failed, {skipped} skipped");
    if failed > 0 {
        bail!("{failed} test(s) failed");
    }
    Ok(())
}

fn run_test_file(path: &PathBuf, verbose: bool) -> Result<(usize, usize, usize)> {
    let content = fs::read_to_string(path)?;
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    // SQLite tests share one database connection across sequential do_test blocks.
    let mut conn = Connection::open_in_memory()?;

    let mut i = 0;
    let lines: Vec<&str> = content.lines().collect();
    while i < lines.len() {
        let line = lines[i].trim();
        if line.starts_with("do_test ") {
            if let Some((name, body_start, body_end, expected_start, expected_end)) =
                parse_do_test(&lines, i)
            {
                let expected = extract_expected(&lines[expected_start..=expected_end]);
                let sql_commands = extract_execsql(&lines[body_start..=body_end]);
                match run_test_case(&mut conn, &name, &sql_commands, &expected) {
                    Ok(true) => {
                        if verbose {
                            println!("  PASS: {name}");
                        }
                        passed += 1;
                    }
                    Ok(false) => {
                        println!("  FAIL: {name}");
                        failed += 1;
                    }
                    Err(e) => {
                        println!("  ERROR: {name}: {e}");
                        failed += 1;
                    }
                }
                i = expected_end + 1;
                continue;
            }
        }
        i += 1;
    }

    if passed + failed == 0 {
        skipped += 1;
    }
    Ok((passed, failed, skipped))
}

fn parse_do_test(lines: &[&str], start: usize) -> Option<(String, usize, usize, usize, usize)> {
    let line = lines[start].trim();
    if !line.starts_with("do_test ") {
        return None;
    }
    let rest = line["do_test ".len()..].trim();
    let name_end = rest.find(' ')?;
    let name = rest[..name_end].to_string();

    let mut brace_depth = 0;
    let mut body_start = 0;
    let mut body_end = 0;
    let mut expected_start = 0;
    let mut expected_end = 0;
    let mut block = 0u8;

    for (i, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            if ch == '{' {
                if brace_depth == 0 {
                    if block == 0 {
                        body_start = i;
                    } else if block == 1 {
                        expected_start = i;
                    }
                }
                brace_depth += 1;
            } else if ch == '}' {
                brace_depth -= 1;
                if brace_depth == 0 {
                    if block == 0 {
                        body_end = i;
                        block = 1;
                    } else if block == 1 {
                        expected_end = i;
                        return Some((name, body_start, body_end, expected_start, expected_end));
                    }
                }
            }
        }
    }
    None
}

fn extract_execsql(lines: &[&str]) -> Vec<String> {
    let text: String = lines
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("\n");
    let mut commands = Vec::new();

    // Match execsql {SQL} including inside catch blocks
    let mut search = text.as_str();
    while let Some(idx) = search.find("execsql") {
        let after = &search[idx + "execsql".len()..];
        if let Some(sql) = extract_braced(after) {
            commands.push(sql.trim().to_string());
        }
        search = &search[idx + 1..];
    }
    commands
}

fn extract_expected(lines: &[&str]) -> String {
    let text = lines.join("\n");
    let after_body = if let Some(pos) = text.rfind("} {") {
        text[pos + 3..].trim()
    } else {
        text.trim()
    };
    extract_braced(after_body)
        .unwrap_or_else(|| after_body.trim().trim_matches('{').trim_matches('}').to_string())
        .trim()
        .to_string()
}

fn extract_braced(s: &str) -> Option<String> {
    let s = s.trim_start();
    if !s.starts_with('{') {
        return None;
    }
    let mut depth = 0;
    let mut result = String::new();
    for ch in s.chars() {
        if ch == '{' {
            depth += 1;
            if depth > 1 {
                result.push(ch);
            }
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(result);
            }
            result.push(ch);
        } else {
            result.push(ch);
        }
    }
    None
}

fn run_test_case(
    conn: &mut Connection,
    _name: &str,
    sql_commands: &[String],
    expected: &str,
) -> Result<bool> {
    let mut last_result = Vec::new();
    let mut last_error: Option<String> = None;

    for sql in sql_commands {
        let sql = sql.trim();
        if sql.is_empty() {
            continue;
        }
        match conn.execute(sql) {
            Ok(rows) => {
                last_result = rows;
                last_error = None;
            }
            Err(e) => {
                last_error = Some(e.message());
                last_result = vec![format!("1 {{{}}}", e.message())];
            }
        }
    }

    let norm_expected = normalize(expected);
    if norm_expected.starts_with("1 {") {
        let actual = if let Some(ref err) = last_error {
            format!("1 {{{err}}}")
        } else {
            format_result(&last_result)
        };
        return Ok(error_result_match(&normalize(&actual), &norm_expected));
    }

    let actual = format_result(&last_result);
    let ok = normalize(&actual) == normalize(expected);
    if !ok && std::env::var("SQLLITE_TEST_DEBUG").is_ok() {
        eprintln!("  expected: {expected:?}");
        eprintln!("  actual:   {actual:?}");
    }
    Ok(ok)
}

fn normalize(s: &str) -> String {
    s.replace('\n', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_result(rows: &[String]) -> String {
    rows.join(" ")
}

/// Match SQLite TCL error list format: `1 {error message}`
fn error_result_match(actual: &str, expected: &str) -> bool {
    let actual_msg = actual
        .strip_prefix("1 {")
        .and_then(|s| s.strip_suffix('}'))
        .or_else(|| actual.strip_prefix("1 {"));
    let expected_msg = expected
        .strip_prefix("1 {")
        .and_then(|s| s.strip_suffix('}'))
        .or_else(|| expected.strip_prefix("1 {"));
    match (actual_msg, expected_msg) {
        (Some(a), Some(e)) => a == e,
        _ => actual == expected,
    }
}
