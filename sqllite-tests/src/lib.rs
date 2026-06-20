//! SQLite-compatible test runner library for sqllite-rust.

use anyhow::Result;
use sqllite_core::Connection;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy)]
pub struct TestResults {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl TestResults {
    pub fn total(&self) -> usize {
        self.passed + self.failed + self.skipped
    }

    pub fn merge(&mut self, other: TestResults) {
        self.passed += other.passed;
        self.failed += other.failed;
        self.skipped += other.skipped;
    }
}

/// Run all `.test` files under `path` (file or directory).
pub fn run_tests(path: &Path, pattern: Option<&str>, verbose: bool) -> Result<TestResults> {
    let mut results = TestResults::default();

    if path.is_dir() {
        for file in collect_test_files(path)? {
            if let Some(pat) = pattern {
                if !file.to_string_lossy().contains(pat) {
                    continue;
                }
            }
            match run_test_file(&file, verbose) {
                Ok(r) => results.merge(r),
                Err(e) => {
                    eprintln!("Error running {}: {e}", file.display());
                    results.failed += 1;
                }
            }
        }
    } else {
        results = run_test_file(path, verbose)?;
    }

    Ok(results)
}

fn collect_test_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_test_files(&path)?);
        } else if path.extension().is_some_and(|e| e == "test") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn run_test_file(path: &Path, verbose: bool) -> Result<TestResults> {
    let content = fs::read_to_string(path)?;
    let lines: Vec<&str> = content.lines().collect();
    let mut conn = Connection::open_in_memory()?;
    let mut results = TestResults::default();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }

        if line == "reset_db" || line.starts_with("reset_db ") {
            conn = Connection::open_in_memory()?;
            i += 1;
            continue;
        }

        if line.starts_with("execsql ") {
            if let Some((sql, end)) = parse_inline_execsql(&lines, i) {
                for stmt in split_sql(&sql) {
                    conn.execute(&stmt)?;
                }
                i = end + 1;
                continue;
            }
        }

        if let Some(parsed) = parse_test_directive(&lines, i) {
            match parsed.kind {
                TestKind::ExecSql { sql, expected, catch } => {
                    let ok = run_sql_test(&mut conn, &sql, &expected, catch)?;
                    record_result(&mut results, &parsed.name, ok, verbose);
                }
                TestKind::DoTest { body } => {
                    let (sql_commands, catch_sql, catch) = extract_body_commands(&body);
                    let expected = parsed.expected.unwrap_or_default();
                    let ok = run_commands_test(
                        &mut conn,
                        &sql_commands,
                        catch_sql.as_deref(),
                        &expected,
                        catch,
                    )?;
                    record_result(&mut results, &parsed.name, ok, verbose);
                }
            }
            i = parsed.end_line + 1;
            continue;
        }

        i += 1;
    }

    if results.passed + results.failed == 0 {
        results.skipped += 1;
    }
    Ok(results)
}

fn record_result(results: &mut TestResults, name: &str, ok: bool, verbose: bool) {
    if ok {
        if verbose {
            println!("  PASS: {name}");
        }
        results.passed += 1;
    } else {
        println!("  FAIL: {name}");
        results.failed += 1;
    }
}

struct ParsedDirective {
    name: String,
    kind: TestKind,
    expected: Option<String>,
    end_line: usize,
}

enum TestKind {
    ExecSql {
        sql: String,
        expected: String,
        catch: bool,
    },
    DoTest {
        body: String,
    },
}

fn parse_test_directive(lines: &[&str], start: usize) -> Option<ParsedDirective> {
    let line = lines[start].trim();
    if line.starts_with("do_execsql_test ") {
        let rest = &line["do_execsql_test ".len()..];
        let (name, blocks, end_line) = parse_name_and_blocks(lines, start, rest)?;
        let sql = blocks.first().cloned().unwrap_or_default();
        let expected = blocks.get(1).cloned().unwrap_or_default();
        return Some(ParsedDirective {
            name,
            kind: TestKind::ExecSql {
                sql,
                expected,
                catch: false,
            },
            expected: None,
            end_line,
        });
    }

    if line.starts_with("do_catchsql_test ") {
        let rest = &line["do_catchsql_test ".len()..];
        let (name, blocks, end_line) = parse_name_and_blocks(lines, start, rest)?;
        let sql = blocks.first().cloned().unwrap_or_default();
        let expected = blocks.get(1).cloned().unwrap_or_default();
        return Some(ParsedDirective {
            name,
            kind: TestKind::ExecSql {
                sql,
                expected,
                catch: true,
            },
            expected: None,
            end_line,
        });
    }

    if line.starts_with("do_test ") {
        let rest = &line["do_test ".len()..];
        let (name, blocks, end_line) = parse_name_and_blocks(lines, start, rest)?;
        let body = blocks.first().cloned().unwrap_or_default();
        let expected = blocks.get(1).cloned();
        return Some(ParsedDirective {
            name,
            kind: TestKind::DoTest { body },
            expected,
            end_line,
        });
    }

    None
}

fn parse_name_and_blocks(
    lines: &[&str],
    start: usize,
    rest: &str,
) -> Option<(String, Vec<String>, usize)> {
    let name = first_token(rest)?;
    let combined = lines[start..].join("\n");
    let name_end = combined.find(name)? + name.len();
    let mut pos = name_end;
    let mut blocks = Vec::new();

    for _ in 0..3 {
        let slice = combined.get(pos..)?.trim_start();
        let trim_skip = combined[pos..].len() - slice.len();
        if let Some((block, consumed, _)) = extract_braced_from_str(slice) {
            blocks.push(block);
            pos += trim_skip + consumed;
        } else {
            break;
        }
    }

    if blocks.is_empty() {
        return None;
    }

    let end_line = start + combined[..pos].chars().filter(|&c| c == '\n').count();
    Some((name.to_string(), blocks, end_line))
}

fn first_token(s: &str) -> Option<&str> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    if s.starts_with('{') {
        return None;
    }
    let end = s.find(|c: char| c.is_whitespace() || c == '{').unwrap_or(s.len());
    Some(s[..end].trim())
}

fn parse_inline_execsql(lines: &[&str], start: usize) -> Option<(String, usize)> {
    let combined = lines[start..].join("\n");
    let exec_pos = combined.find("execsql")? + "execsql".len();
    let after_exec = combined[exec_pos..].trim_start();
    let trim_skip = combined[exec_pos..].len() - after_exec.len();
    let (sql, consumed, _) = extract_braced_from_str(after_exec)?;
    let end_pos = exec_pos + trim_skip + consumed;
    let end_line = start + combined[..end_pos].chars().filter(|&c| c == '\n').count();
    Some((sql, end_line))
}

/// Extract a TCL braced string from `s`, returning content, total bytes consumed, and line offset.
fn extract_braced_from_str(s: &str) -> Option<(String, usize, usize)> {
    let trimmed = s.trim_start();
    let skip = s.len() - trimmed.len();
    if !trimmed.starts_with('{') {
        return None;
    }

    let mut depth = 0;
    let mut content = String::new();
    let mut consumed = 0usize;

    for ch in trimmed.chars() {
        consumed += ch.len_utf8();
        if ch == '{' {
            depth += 1;
            if depth > 1 {
                content.push(ch);
            }
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                let lines_used = trimmed[..consumed].chars().filter(|&c| c == '\n').count();
                return Some((content, skip + consumed, lines_used));
            }
            content.push(ch);
        } else {
            content.push(ch);
        }
    }
    None
}

/// Extract a TCL braced string spanning multiple lines (first line remainder + following lines).
fn extract_braced_block_multiline(
    lines: &[&str],
    start_within_first: &str,
) -> Option<(String, usize, usize)> {
    let first = lines.first()?;
    let start_idx = first.len().saturating_sub(start_within_first.len());
    let combined: String = std::iter::once(&first[start_idx..])
        .chain(lines.iter().skip(1).copied())
        .collect::<Vec<_>>()
        .join("\n");
    extract_braced_from_str(&combined)
}

fn extract_body_commands(body: &str) -> (Vec<String>, Option<String>, bool) {
    if let Some(sql) = extract_directive_call(body, "catchsql") {
        return (split_sql(&sql), None, true);
    }

    let catch_sql = extract_catch_execsql(body);
    let mut setup = Vec::new();
    let mut search = body;
    while let Some(idx) = search.find("execsql") {
        let after = &search[idx + "execsql".len()..].trim_start();
        if let Some((sql_block, consumed, _)) = extract_braced_block_from_str(after) {
            let is_caught = catch_sql
                .as_ref()
                .is_some_and(|c| split_sql(c) == split_sql(&sql_block));
            if !is_caught {
                setup.extend(split_sql(&sql_block));
            }
            search = &after[consumed..];
        } else {
            search = &search[idx + 1..];
        }
    }

    if let Some(sql) = catch_sql {
        return (setup, Some(sql), true);
    }
    (setup, None, false)
}

fn extract_braced_block_from_str(s: &str) -> Option<(String, usize, usize)> {
    extract_braced_from_str(s)
}

fn extract_directive_call(body: &str, directive: &str) -> Option<String> {
    let idx = body.find(directive)?;
    let after = &body[idx + directive.len()..].trim_start();
    extract_braced_block_from_str(after).map(|(s, _, _)| s)
}

fn extract_catch_execsql(body: &str) -> Option<String> {
    // set v [catch {execsql {SQL}} msg]
    let idx = body.find("catch")?;
    let after = &body[idx..];
    let exec_idx = after.find("execsql")?;
    let after_exec = &after[exec_idx + "execsql".len()..].trim_start();
    extract_braced_block_from_str(after_exec).map(|(s, _, _)| s)
}

fn split_sql(sql: &str) -> Vec<String> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if trimmed.contains(';') {
        return trimmed
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }

    trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn run_sql_test(
    conn: &mut Connection,
    sql: &str,
    expected: &str,
    catch: bool,
) -> Result<bool> {
    if catch {
        return run_commands_test(conn, &[], Some(sql), expected, true);
    }
    let commands = split_sql(sql);
    run_commands_test(conn, &commands, None, expected, false)
}

fn run_commands_test(
    conn: &mut Connection,
    sql_commands: &[String],
    catch_sql: Option<&str>,
    expected: &str,
    catch: bool,
) -> Result<bool> {
    if catch {
        for sql in sql_commands {
            let sql = sql.trim();
            if sql.is_empty() {
                continue;
            }
            conn.execute(sql)?;
        }
        let sql = catch_sql.unwrap_or("");
        return Ok(run_catchsql(conn, sql, expected));
    }

    let mut last_result = Vec::new();
    for sql in sql_commands {
        let sql = sql.trim();
        if sql.is_empty() {
            continue;
        }
        match conn.execute(sql) {
            Ok(rows) => last_result = rows,
            Err(e) => {
                let actual = format!("1 {{{}}}", e.message());
                return Ok(results_match(&actual, expected));
            }
        }
    }

    let norm_expected = normalize(expected);
    if norm_expected.starts_with("1 {") {
        return Ok(results_match(&format_result(&last_result), expected));
    }

    let actual = format_result(&last_result);
    let ok = results_match(&actual, expected);
    if !ok && std::env::var("SQLLITE_TEST_DEBUG").is_ok() {
        eprintln!("  expected: {expected:?}");
        eprintln!("  actual:   {actual:?}");
    }
    Ok(ok)
}

fn run_catchsql(conn: &mut Connection, sql: &str, expected: &str) -> bool {
    let sql = sql.trim();
    let actual = match conn.execute(sql) {
        Ok(_) => "0 {}".to_string(),
        Err(e) => format!("1 {{{}}}", e.message()),
    };
    results_match(&actual, expected)
}

/// Parse SQLite TCL result format into tokens (`{}` denotes empty/NULL).
fn parse_result_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '{' {
            chars.next();
            let mut depth = 1;
            let mut token = String::new();
            while let Some(c) = chars.next() {
                if c == '{' {
                    depth += 1;
                    token.push(c);
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    token.push(c);
                } else {
                    token.push(c);
                }
            }
            tokens.push(token);
        } else {
            let mut token = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '{' {
                    break;
                }
                token.push(c);
                chars.next();
            }
            if !token.is_empty() {
                tokens.push(token);
            }
        }
    }
    tokens
}

fn results_match(actual: &str, expected: &str) -> bool {
    let actual_tokens = parse_result_tokens(actual);
    let expected_tokens = parse_result_tokens(expected);

    if expected_tokens.first().is_some_and(|t| t == "1")
        && actual_tokens.first().is_some_and(|t| t == "1")
    {
        return actual_tokens.get(1) == expected_tokens.get(1);
    }

    actual_tokens == expected_tokens
}

fn normalize(s: &str) -> String {
    s.replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_result(rows: &[String]) -> String {
    rows.iter()
        .map(|v| {
            if v.is_empty() {
                "{}".to_string()
            } else {
                v.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_braced_sql() {
        let lines = ["do_execsql_test t1 {", "  SELECT 1;", "  SELECT 2;", "}"];
        let parsed = parse_test_directive(&lines, 0).unwrap();
        match parsed.kind {
            TestKind::ExecSql { sql, .. } => {
                assert!(sql.contains("SELECT 1"));
                assert!(sql.contains("SELECT 2"));
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn split_sql_semicolons_and_newlines() {
        assert_eq!(split_sql("A; B"), vec!["A", "B"]);
        assert_eq!(
            split_sql("CREATE TABLE t(x int)\nINSERT INTO t VALUES(1)"),
            vec!["CREATE TABLE t(x int)", "INSERT INTO t VALUES(1)"]
        );
    }

    #[test]
    fn parse_result_tokens_handles_null_and_errors() {
        assert_eq!(parse_result_tokens("1 2 3"), vec!["1", "2", "3"]);
        assert_eq!(parse_result_tokens("1 {} 3"), vec!["1", "", "3"]);
        assert_eq!(
            parse_result_tokens("1 {no such table: t}"),
            vec!["1", "no such table: t"]
        );
    }
}
