# sqllite-rust

A pure Rust rewrite of SQLite — no dependency on the SQLite C library.

## Overview

sqllite-rust is an in-progress, from-scratch implementation of a SQLite-compatible embedded SQL database engine written entirely in Rust. The goal is full feature parity with SQLite and passing the complete SQLite test suite.

See [PLAN.md](PLAN.md) for the detailed architecture map and implementation phases.

## Project Structure

```
sqllite-core/     # Database engine (VFS, pager, B-tree, VDBE, schema)
sqllite-parser/   # SQL lexer and parser
sqllite-cli/      # Command-line shell (sqllite3)
sqllite-tests/    # SQLite-compatible .test file runner
tests/            # Ported SQLite regression tests
```

## Building

```bash
cargo build --release
```

## Running Tests

```bash
# Unit tests
cargo test

# Ported SQLite regression tests
cargo run -p sqllite-tests -- tests -v

# Or via Makefile
make test-sqlite
```

## CLI Usage

```bash
# Interactive shell
cargo run -p sqllite-cli

# Execute SQL
cargo run -p sqllite-cli -- -c "CREATE TABLE t(x int); INSERT INTO t VALUES(42); SELECT * FROM t;"
```

## Current Status

Implemented (Phase 1–4 foundation):

- SQLite file format (page header, varints, record encoding)
- Virtual file system (Unix + in-memory)
- Pager with page cache and transactions
- B-tree storage (leaf table pages, insert, scan, delete)
- SQL lexer and parser (DDL/DML/DQL subset)
- VDBE virtual machine (~50 opcodes)
- Schema catalog and CREATE/DROP TABLE
- INSERT, SELECT, DELETE with WHERE
- `:memory:` and file-backed databases

Not yet implemented: WAL, JOINs, aggregates, indexes, triggers, views, PRAGMA, full opcode set, remaining 1,180+ SQLite tests.

## License

MIT
