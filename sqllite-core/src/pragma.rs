//! PRAGMA statement execution.

use crate::constants::{MAX_PAGE_SIZE, MIN_PAGE_SIZE};
use crate::error::{Result, ResultCode, SqlliteError};
use crate::schema::Schema;
use crate::storage::pager::{JournalMode, Pager};
use crate::types::Value;

pub type PragmaRow = Vec<Value>;

pub fn execute_pragma(
    name: &str,
    value: Option<&str>,
    pager: &Pager,
    schema: &Schema,
) -> Result<Vec<PragmaRow>> {
    match name.to_ascii_lowercase().as_str() {
        "journal_mode" => execute_journal_mode(value, pager),
        "page_size" => execute_page_size(value, pager),
        "table_info" => execute_table_info(value, schema),
        other => Err(SqlliteError::sql(
            ResultCode::Error,
            format!("unknown or unsupported pragma: {other}"),
        )),
    }
}

fn journal_mode_name(mode: JournalMode) -> &'static str {
    match mode {
        JournalMode::Delete => "delete",
        JournalMode::Persist => "persist",
        JournalMode::Off => "off",
        JournalMode::Truncate => "truncate",
        JournalMode::Memory => "memory",
        JournalMode::Wal => "wal",
    }
}

fn parse_journal_mode(s: &str) -> Result<JournalMode> {
    match s.to_ascii_lowercase().as_str() {
        "delete" => Ok(JournalMode::Delete),
        "persist" => Ok(JournalMode::Persist),
        "off" => Ok(JournalMode::Off),
        "truncate" => Ok(JournalMode::Truncate),
        "memory" => Ok(JournalMode::Memory),
        "wal" => Ok(JournalMode::Wal),
        _ => Err(SqlliteError::sql(
            ResultCode::Error,
            format!("unknown journal mode: {s}"),
        )),
    }
}

fn execute_journal_mode(value: Option<&str>, pager: &Pager) -> Result<Vec<PragmaRow>> {
    if let Some(mode_str) = value {
        let mode = parse_journal_mode(mode_str)?;
        let applied = pager.set_journal_mode(mode)?;
        Ok(vec![vec![Value::Text(journal_mode_name(applied).to_string())]])
    } else {
        Ok(vec![vec![Value::Text(
            journal_mode_name(pager.journal_mode()).to_string(),
        )]])
    }
}

fn execute_page_size(value: Option<&str>, pager: &Pager) -> Result<Vec<PragmaRow>> {
    if let Some(size_str) = value {
        let size = size_str
            .parse::<u32>()
            .map_err(|_| SqlliteError::sql(ResultCode::Error, "invalid page_size value"))?;
        if size < MIN_PAGE_SIZE || size > MAX_PAGE_SIZE || !size.is_power_of_two() {
            return Err(SqlliteError::sql(
                ResultCode::Error,
                "page_size must be a power of two between 512 and 65536",
            ));
        }
        let applied = pager.set_page_size(size)?;
        Ok(vec![vec![Value::Integer(applied as i64)]])
    } else {
        Ok(vec![vec![Value::Integer(pager.page_size() as i64)]])
    }
}

fn execute_table_info(value: Option<&str>, schema: &Schema) -> Result<Vec<PragmaRow>> {
    let name = value
        .ok_or_else(|| SqlliteError::sql(ResultCode::Error, "table_info requires a table name"))?;
    let table = schema.table(name).ok_or_else(|| {
        SqlliteError::sql(ResultCode::Error, format!("no such table: {name}"))
    })?;
    let mut rows = Vec::new();
    for (cid, col) in table.columns.iter().enumerate() {
        let type_name = col.affinity.as_char().to_string().to_uppercase();
        rows.push(vec![
            Value::Integer(cid as i64),
            Value::Text(col.name.clone()),
            Value::Text(type_name),
            Value::Integer(if col.not_null { 1 } else { 0 }),
            col.default_value.clone().unwrap_or(Value::Null),
            Value::Integer(if col.primary_key { 1 } else { 0 }),
        ]);
    }
    Ok(rows)
}

pub fn pragma_value_from_expr(expr: &sqllite_parser::ast::Expr) -> String {
    use sqllite_parser::ast::Expr;
    match expr {
        Expr::String(s) => s.clone(),
        Expr::Ident(s) => s.clone(),
        Expr::Integer(i) => i.to_string(),
        Expr::Real(r) => r.to_string(),
        _ => String::new(),
    }
}

pub fn pragma_rows_to_strings(rows: &[PragmaRow]) -> Vec<String> {
    rows.iter()
        .flat_map(|row| row.iter().map(value_to_string))
        .collect()
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
        Value::Blob(b) => format!(
            "X'{}'",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Connection;

    #[test]
    fn pragma_journal_mode_query() {
        let conn = Connection::open_in_memory().unwrap();
        let rows = execute_pragma("journal_mode", None, conn.pager(), &conn.schema().read())
            .unwrap();
        assert_eq!(rows[0][0], Value::Text("delete".to_string()));
    }

    #[test]
    fn pragma_page_size_query() {
        let conn = Connection::open_in_memory().unwrap();
        let rows = execute_pragma("page_size", None, conn.pager(), &conn.schema().read())
            .unwrap();
        assert_eq!(rows[0][0], Value::Integer(4096));
    }
}
