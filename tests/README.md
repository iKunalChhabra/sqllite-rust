# SQLite-Compatible Regression Tests

This directory contains a curated subset of [SQLite's TCL test suite](https://www.sqlite.org/testing.html), ported into a simplified self-contained `.test` format for **sqllite-rust**.

## Running Tests

```bash
# Run all ported tests (verbose)
cargo run -p sqllite-tests -- tests -v

# Run a category
cargo run -p sqllite-tests -- tests/dml -v

# Run via Makefile
make test-sqlite

# Run as part of `cargo test` (integration test)
cargo test -p sqllite-tests
```

## Directory Layout

| Directory | Focus | Source inspiration |
|-----------|-------|-------------------|
| `ddl/` | CREATE TABLE, DROP TABLE | `insert.test`, `alter.test` |
| `dml/` | INSERT, SELECT, DELETE, UPDATE errors | `insert.test`, `select1.test`, `delete.test`, `update.test` |
| `types/` | Integer, text, real storage | `affinity2.test` |
| `expr/` | Literal arithmetic and comparisons | `expr.test` |
| `basic.test` | Original smoke tests | `insert.test` |

## Supported Test Directives

The `sqllite-test` runner understands a subset of SQLite's TCL test format:

| Directive | Description |
|-----------|-------------|
| `do_test NAME { body } { expected }` | Run TCL body, compare result |
| `do_execsql_test NAME { sql } { expected }` | Run SQL (multi-statement), compare last result |
| `do_catchsql_test NAME { sql } { expected }` | Run SQL expecting error (`1 {message}`) or success (`0 {}`) |
| `execsql { sql }` | Standalone setup SQL between tests (shared connection) |
| `reset_db` | Reset to a fresh in-memory database |

### TCL patterns recognized inside `do_test` bodies

- `execsql { SQL }` — execute SQL
- `catchsql { SQL }` — catch errors (returns `0 {}` or `1 {message}`)
- `set v [catch {execsql { SQL }} msg]` + `lappend v $msg` — classic error test pattern

### SQL splitting

SQL blocks are split on `;` or on newlines (SQLite `execsql { ... }` style).

## Expected Result Format

Results use SQLite TCL whitespace-separated tokens. Empty/NULL values are written as `{}`:

```
{1 2 3}        → three integer columns
{1 {} 3}       → middle column is NULL
{1 {no such table: t}}  → error result
```

## Not Supported (omitted from ported tests)

- `ifcapable`, `finish_test`, `source`, Tcl loops/procs
- `db eval`, `sqlite3` handle APIs, prepared statement introspection
- `integrity_check`, `explain`, `db changes`
- Attached databases, temp tables, PRAGMA (most)
- JOINs, aggregates, indexes, triggers, views, ALTER TABLE
- `SELECT` without `FROM` (except where engine adds support)
- Full SQLite error message exact-match for syntax errors

## Adding Tests

1. Find a test in `/tmp/sqlite-ref/test/` (or upstream SQLite).
2. Simplify to use only supported directives and engine features.
3. Place in the appropriate category subdirectory.
4. Run `cargo run -p sqllite-tests -- path/to/new.test -v`.

Tests within a file share one database connection (like SQLite's `tester.tcl`). Use `reset_db` when a file needs a clean slate mid-file.
