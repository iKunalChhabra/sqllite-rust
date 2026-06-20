# sqllite-rust: Complete SQLite Rewrite Plan

This document maps the SQLite C source (~207k LOC in `src/`, 1,190 TCL test files) to a pure Rust implementation with zero dependency on the SQLite C library.

## Source Analysis Summary

### SQLite Architecture (bottom-up)

```
┌─────────────────────────────────────────────────────────────┐
│  Public API (main.c, prepare.c, vdbeapi.c)                  │
│  sqlite3_open, sqlite3_prepare_v2, sqlite3_step, etc.     │
├─────────────────────────────────────────────────────────────┤
│  SQL Compiler                                               │
│  tokenize.c → parse.y → resolve.c → select.c/insert.c/... │
│  → where.c (optimizer) → vdbe.c (code generator)          │
├─────────────────────────────────────────────────────────────┤
│  VDBE Virtual Machine (vdbe.c, vdbeaux.c, vdbemem.c)       │
│  ~190 opcodes, register-based execution                   │
├─────────────────────────────────────────────────────────────┤
│  Schema & Catalog (build.c, table.c, alter.c, fkey.c)      │
├─────────────────────────────────────────────────────────────┤
│  B-Tree Storage (btree.c - 11,600 LOC)                      │
│  Table & index btrees, cursors, cell format                 │
├─────────────────────────────────────────────────────────────┤
│  Pager / Transactions (pager.c - 7,826 LOC, wal.c - 4,645) │
│  Page cache, journal, WAL, locking                          │
├─────────────────────────────────────────────────────────────┤
│  VFS / I/O (os_unix.c, os_win.c, memdb.c)                  │
│  Pluggable virtual file system                              │
├─────────────────────────────────────────────────────────────┤
│  Infrastructure (malloc.c, mutex.c, hash.c, util.c)         │
└─────────────────────────────────────────────────────────────┘
```

### Module Mapping: C → Rust

| SQLite C Module | Lines | Rust Module | Priority |
|-----------------|-------|-------------|----------|
| `main.c` | 5,201 | `sqllite-core/src/api/` | P1 |
| `tokenize.c` | ~900 | `sqllite-parser/src/lexer.rs` | P1 |
| `parse.y` → `parse.c` | ~25k | `sqllite-parser/src/parser.rs` | P1 |
| `vdbe.c` | 9,437 | `sqllite-core/src/vdbe/` | P1 |
| `vdbeaux.c` | 5,812 | `sqllite-core/src/vdbe/aux.rs` | P1 |
| `vdbemem.c` | 2,257 | `sqllite-core/src/vdbe/mem.rs` | P1 |
| `btree.c` | 11,600 | `sqllite-core/src/storage/btree/` | P1 |
| `pager.c` | 7,826 | `sqllite-core/src/storage/pager.rs` | P1 |
| `wal.c` | 4,645 | `sqllite-core/src/storage/wal.rs` | P2 |
| `pcache.c` | ~1,500 | `sqllite-core/src/storage/pcache.rs` | P1 |
| `os_unix.c` | 8,606 | `sqllite-core/src/io/unix.rs` | P1 |
| `build.c` | 5,830 | `sqllite-core/src/schema/build.rs` | P1 |
| `select.c` | 8,976 | `sqllite-core/src/translate/select.rs` | P1 |
| `insert.c` | 3,463 | `sqllite-core/src/translate/insert.rs` | P1 |
| `update.c` | 1,328 | `sqllite-core/src/translate/update.rs` | P2 |
| `delete.c` | ~800 | `sqllite-core/src/translate/delete.rs` | P2 |
| `where.c` | 7,886 | `sqllite-core/src/translate/where/` | P2 |
| `expr.c` | 7,727 | `sqllite-core/src/expr/` | P1 |
| `resolve.c` | 2,356 | `sqllite-core/src/resolve.rs` | P1 |
| `pragma.c` | 3,105 | `sqllite-core/src/pragma.rs` | P2 |
| `func.c` | 3,501 | `sqllite-core/src/functions/` | P2 |
| `trigger.c` | 1,575 | `sqllite-core/src/trigger.rs` | P3 |
| `fkey.c` | 1,488 | `sqllite-core/src/fkey.rs` | P3 |
| `analyze.c` | 2,012 | `sqllite-core/src/analyze.rs` | P3 |
| `alter.c` | 3,067 | `sqllite-core/src/alter.rs` | P3 |
| `attach.c` | ~1,000 | `sqllite-core/src/attach.rs` | P3 |
| `backup.c` | ~800 | `sqllite-core/src/backup.rs` | P3 |
| `json.c` | 5,739 | `sqllite-core/src/json/` | P3 |
| `date.c` | 1,828 | `sqllite-core/src/date.rs` | P2 |
| `window.c` | 3,112 | `sqllite-core/src/window.rs` | P3 |
| `vtab.c` | 1,380 | `sqllite-core/src/vtab.rs` | P3 |

### Extensions (`ext/`)

| Extension | Rust Module | Priority |
|-----------|-------------|----------|
| FTS3/FTS5 | `sqllite-ext/fts/` | P4 |
| RTREE | `sqllite-ext/rtree/` | P4 |
| RBU | `sqllite-ext/rbu/` | P4 |
| Session | `sqllite-ext/session/` | P4 |

## Implementation Phases

### Phase 1: Foundation (Week 1 equivalent)
- [x] Project workspace setup
- [ ] Error types matching SQLite result codes
- [ ] `Value` type (NULL, INTEGER, REAL, TEXT, BLOB)
- [ ] Page format constants and header parsing
- [ ] Varint encoding/decoding
- [ ] Record format (serial types, payload)

### Phase 2: Storage Layer
- [ ] VFS trait + Unix implementation
- [ ] Page cache (LRU)
- [ ] Pager (read/write pages, dirty tracking)
- [ ] Rollback journal
- [ ] WAL mode
- [ ] B-tree page types (leaf table, interior table, leaf index, interior index)
- [ ] B-tree cursors (seek, next, prev, insert, delete)
- [ ] Database file header (100 bytes)

### Phase 3: SQL Front-End
- [ ] Lexer (tokenize.c equivalent)
- [ ] Parser (parse.y grammar - all DDL/DML/DQL)
- [ ] AST types
- [ ] Name resolution (resolve.c)
- [ ] Expression evaluation

### Phase 4: VDBE
- [ ] All ~190 opcodes
- [ ] Register file
- [ ] Program compilation from AST
- [ ] Statement execution loop

### Phase 5: Schema & DDL
- [ ] sqlite_master table
- [ ] CREATE TABLE / INDEX / VIEW / TRIGGER
- [ ] DROP TABLE / INDEX
- [ ] ALTER TABLE
- [ ] PRAGMA statements

### Phase 6: DML & Queries
- [ ] INSERT (including INSERT OR ...)
- [ ] UPDATE / DELETE
- [ ] SELECT (simple → complex with JOINs, subqueries, aggregates)
- [ ] ORDER BY, GROUP BY, HAVING, LIMIT
- [ ] Query optimizer (where.c)
- [ ] Transactions (BEGIN/COMMIT/ROLLBACK/SAVEPOINT)

### Phase 7: Public API
- [ ] `sqlite3_open` / `sqlite3_close`
- [ ] `sqlite3_prepare_v2` / `sqlite3_step` / `sqlite3_finalize`
- [ ] `sqlite3_exec` with callback
- [ ] `sqlite3_bind_*` / `sqlite3_column_*`
- [ ] `sqlite3_errmsg` / `sqlite3_errcode`
- [ ] CLI shell (`sqllite` binary)

### Phase 8: Advanced Features
- [ ] Foreign keys
- [ ] Triggers
- [ ] Views
- [ ] ATTACH/DETACH
- [ ] Backup API
- [ ] User-defined functions
- [ ] Virtual tables
- [ ] JSON functions
- [ ] Date/time functions
- [ ] Window functions
- [ ] ANALYZE / query planner stats
- [ ] Auto-vacuum / incremental vacuum
- [ ] Encryption (if needed)

### Phase 9: Testing
- [ ] Rust test runner for SQLite `.test` TCL format
- [ ] Port `testfixture` test protocol
- [ ] Run all 1,190 test files
- [ ] SQL logic test compatibility
- [ ] Fuzz testing

## File Format Compatibility

Must read/write standard SQLite 3 database files:
- Magic: `SQLite format 3\0` (16 bytes)
- Page size: 512-65536, default 4096
- B-tree page types: 0x02, 0x05, 0x0a, 0x0d
- Record format with serial types 0-9
- WAL format with frame headers
- Journal format with page records

## Test Strategy

SQLite tests are TCL scripts in `test/*.test` run by `testfixture`. Our approach:

1. **Rust test runner** (`sqllite-tests/`) parses `.test` files
2. Implements `execsql`, `do_test`, `catchsql` primitives
3. Runs against our engine via the public API
4. Compares output with expected results
5. Incrementally enable test files as features are implemented

Test categories (by count):
- Basic DDL/DML: createtab, insert, select, delete, update
- Schema: alter, index, fkey, trigger, view
- Transactions: trans, wal, savepoint
- Types: affinity, cast, blob
- Functions: func, date, json
- Pragmas: pragma
- Edge cases: corrupt, fault, malloc

## Crate Structure

```
sqllite-rust/
├── Cargo.toml              # Workspace
├── PLAN.md                 # This file
├── sqllite-core/           # Core engine
├── sqllite-parser/         # SQL lexer + parser
├── sqllite-cli/            # Command-line shell
└── sqllite-tests/          # Test runner
```

## Dependencies (Rust crates only, no SQLite)

- `logos` - lexer
- `thiserror` - error types
- `memchr` - fast byte search
- `parking_lot` - mutexes
- `tempfile` - test temp files

No `libsqlite3`, `rusqlite`, or any SQLite C bindings.
