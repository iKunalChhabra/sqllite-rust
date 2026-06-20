//! Database connection and statement execution.

use crate::compile::compile;
use crate::error::{Result, ResultCode, SqlliteError};
use crate::io::{OpenFlags, UnixVfs, Vfs};
use crate::schema::{Schema, SharedSchema};
use crate::storage::pager::{Pager, PagerFlags};
use crate::types::Value;
use crate::vdbe::Vdbe;
use parking_lot::RwLock;
use sqllite_parser::ast::Statement;
use sqllite_parser::parse_one;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A database connection.
pub struct Connection {
    pager: Arc<Pager>,
    schema: SharedSchema,
    path: Option<PathBuf>,
    last_error: String,
    changes: i64,
    last_rowid: i64,
    in_transaction: bool,
}

impl Connection {
    /// Open a database connection.
    pub fn open(path: &str) -> Result<Self> {
        let vfs = UnixVfs;
        let (pager_path, memory) = if path == ":memory:" {
            (None, true)
        } else {
            (Some(PathBuf::from(path)), false)
        };
        let flags = PagerFlags {
            omit_journal: false,
            memory,
            read_only: false,
        };
        let pager = Arc::new(Pager::open(
            &vfs,
            pager_path.as_deref(),
            flags,
        )?);
        let schema = Arc::new(RwLock::new(Schema::new()));
        schema.write().init_schema_table(pager.clone())?;

        Ok(Self {
            pager,
            schema,
            path: pager_path,
            last_error: String::new(),
            changes: 0,
            last_rowid: 0,
            in_transaction: false,
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Execute a SQL statement without returning results.
    pub fn exec(&mut self, sql: &str) -> Result<()> {
        let mut stmt = self.prepare(sql)?;
        loop {
            match stmt.step()? {
                ResultCode::Row => continue,
                ResultCode::Done => break,
                code if code.is_ok() => break,
                code => {
                    return Err(SqlliteError::sql(code, self.last_error.clone()));
                }
            }
        }
        self.changes = stmt.changes();
        self.last_rowid = stmt.last_insert_rowid();
        Ok(())
    }

    /// Prepare a SQL statement.
    pub fn prepare(&self, sql: &str) -> Result<StatementHandle> {
        let program = compile(sql, &self.schema.read())?;
        let vdbe = Vdbe::new(program, self.pager.clone(), self.schema.clone());
        Ok(StatementHandle {
            vdbe,
            sql: sql.to_string(),
            connection_schema: self.schema.clone(),
            connection_pager: self.pager.clone(),
        })
    }

    /// Execute SQL and collect all rows as strings (for testing).
    pub fn exec_sql(&mut self, sql: &str) -> Result<Vec<String>> {
        let mut results = Vec::new();
        let mut stmt = self.prepare(sql)?;
        loop {
            match stmt.step()? {
                ResultCode::Row => {
                    for i in 0..stmt.column_count() {
                        if let Some(v) = stmt.column_value(i) {
                            results.push(value_to_string(v));
                        }
                    }
                }
                ResultCode::Done => break,
                code => {
                    return Err(SqlliteError::sql(code, stmt.error_message()));
                }
            }
        }
        self.changes = stmt.changes();
        self.last_rowid = stmt.last_insert_rowid();
        Ok(results)
    }

    pub fn changes(&self) -> i64 {
        self.changes
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.last_rowid
    }

    pub fn commit(&mut self) -> Result<()> {
        // Handle DDL that needs schema updates before commit
        self.pager.commit()?;
        self.in_transaction = false;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<()> {
        self.pager.rollback()?;
        self.in_transaction = false;
        Ok(())
    }

    pub fn pager(&self) -> &Pager {
        &self.pager
    }

    pub fn schema(&self) -> &SharedSchema {
        &self.schema
    }

    /// Execute a statement, handling DDL directly.
    pub fn execute(&mut self, sql: &str) -> Result<Vec<String>> {
        let stmt = parse_one(sql).map_err(|e| SqlliteError::Parse(e.to_string()))?;

        match &stmt {
            Statement::CreateTable(create) => {
                self.pager.begin()?;
                self.schema
                    .write()
                    .create_table(self.pager.clone(), create)?;
                self.pager.commit()?;
                Ok(vec![])
            }
            Statement::DropTable(drop) => {
                self.pager.begin()?;
                self.schema.write().drop_table(&drop.name, drop.if_exists)?;
                self.pager.commit()?;
                Ok(vec![])
            }
            Statement::Begin => {
                self.pager.begin()?;
                self.in_transaction = true;
                Ok(vec![])
            }
            Statement::Commit => {
                self.commit()?;
                Ok(vec![])
            }
            Statement::Rollback => {
                self.rollback()?;
                Ok(vec![])
            }
            _ => self.exec_sql(sql),
        }
    }
}

/// A prepared statement.
pub struct StatementHandle {
    vdbe: Vdbe,
    sql: String,
    connection_schema: SharedSchema,
    connection_pager: Arc<Pager>,
}

impl StatementHandle {
    pub fn step(&mut self) -> Result<ResultCode> {
        let rc = self.vdbe.step()?;
        if rc == ResultCode::Done {
            // Commit if not in explicit transaction
            if !self.connection_pager.in_transaction() {
                let _ = self.connection_pager.commit();
            }
        }
        Ok(rc)
    }

    pub fn reset(&mut self) {
        self.vdbe.reset();
    }

    pub fn column_count(&self) -> usize {
        self.vdbe.column_count()
    }

    pub fn column_value(&self, idx: usize) -> Option<&Value> {
        self.vdbe.column_value(idx)
    }

    pub fn column_name(&self, idx: usize) -> &str {
        self.vdbe.column_name(idx)
    }

    pub fn changes(&self) -> i64 {
        self.vdbe.changes()
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.vdbe.last_insert_rowid()
    }

    pub fn error_message(&self) -> String {
        String::new()
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => {
            if r.fract() == 0.0 && r.abs() < 9_007_199_254_740_992.0 {
                format!("{:.0}", r)
            } else {
                r.to_string()
            }
        }
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("X'{}'", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_insert_select() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test1(one int, two int, three int)")
            .unwrap();
        conn.execute("INSERT INTO test1 VALUES(1,2,3)").unwrap();
        let rows = conn.execute("SELECT * FROM test1").unwrap();
        assert_eq!(rows, vec!["1", "2", "3"]);
    }

    #[test]
    fn insert_error_no_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        let err = conn.execute("INSERT INTO test1 VALUES(1,2,3)").unwrap_err();
        assert!(err.message().contains("no such table"));
    }

    #[test]
    fn insert_wrong_column_count() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test1(one int, two int, three int)")
            .unwrap();
        let err = conn.execute("INSERT INTO test1 VALUES(1,2)").unwrap_err();
        assert!(err.message().contains("columns"));
    }
}
