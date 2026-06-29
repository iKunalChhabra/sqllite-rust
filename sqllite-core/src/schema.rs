//! Database schema catalog.

use crate::constants::{ROOT_PAGE, SCHEMA_TABLE_NAME, SCHEMA_TABLE_NAME_LEGACY};
use crate::error::{Result, ResultCode, SqlliteError};
use crate::storage::btree::{btree_insert_index, btree_insert_row, Btree, BtreeFlags};
use crate::storage::pager::Pager;
use crate::record::encode_record;
use crate::types::{Affinity, Value};
use parking_lot::RwLock;
use sqllite_parser::ast::{ColumnConstraint, ColumnDef, ColumnType, CreateIndex, CreateTable, Statement};
use sqllite_parser::parse_one;
use std::collections::HashMap;
use std::sync::Arc;

/// Column metadata.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub affinity: Affinity,
    pub not_null: bool,
    pub primary_key: bool,
    pub autoincrement: bool,
    pub default_value: Option<Value>,
}

/// Index metadata.
#[derive(Debug, Clone)]
pub struct Index { pub name: String, pub table: String, pub columns: Vec<String>, pub root_page: u32, pub unique: bool }

/// Table metadata.
#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub root_page: u32,
    pub columns: Vec<Column>,
    pub rowid_alias: Option<String>,
}

impl Table {
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}

/// Schema catalog for a database.
#[derive(Default)]
pub struct Schema {
    tables: HashMap<String, Table>,
    indexes: HashMap<String, Index>,
    schema_cookie: u32,
    schema_btree: Option<Arc<Btree>>,
}

impl Schema {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schema_cookie(&self) -> u32 {
        self.schema_cookie
    }

    pub fn bump_cookie(&mut self) {
        self.schema_cookie += 1;
    }

    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.get(&name.to_lowercase())
    }

    pub fn table_mut(&mut self, name: &str) -> Option<&mut Table> {
        self.tables.get_mut(&name.to_lowercase())
    }

    pub fn index(&self, name: &str) -> Option<&Index> { self.indexes.get(&name.to_lowercase()) }

    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values()
    }

    pub fn has_table(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        if lower == SCHEMA_TABLE_NAME || lower == SCHEMA_TABLE_NAME_LEGACY {
            return true;
        }
        self.tables.contains_key(&lower)
    }

    pub fn create_table(
        &mut self,
        pager: Arc<Pager>,
        create: &CreateTable,
    ) -> Result<()> {
        let name_lower = create.name.to_lowercase();
        if self.tables.contains_key(&name_lower) {
            if create.if_not_exists {
                return Ok(());
            }
            return Err(SqlliteError::sql(
                ResultCode::Error,
                format!("table {} already exists", create.name),
            ));
        }

        let (btree, root_page) = Btree::create_table(pager.clone())?;
        let columns = create
            .columns
            .iter()
            .map(column_def_to_column)
            .collect::<Vec<_>>();

        let table = Table {
            name: create.name.clone(),
            root_page,
            columns,
            rowid_alias: None,
        };

        // Insert into sqlite_schema
        self.insert_schema_row(pager, &create.name, "table", root_page, &create_sql(create))?;

        self.tables.insert(name_lower, table);
        self.bump_cookie();
        Ok(())
    }

    
    pub fn create_index(&mut self, pager: Arc<Pager>, create: &CreateIndex) -> Result<()> {
        let name_lower = create.name.to_lowercase();
        if self.indexes.contains_key(&name_lower) {
            if create.if_not_exists { return Ok(()); }
            return Err(SqlliteError::sql(ResultCode::Error, format!("index {} already exists", create.name)));
        }
        let table = self.table(&create.table).ok_or_else(|| SqlliteError::sql(ResultCode::Error, format!("no such table: {}", create.table)))?;
        for col in &create.columns {
            if table.column_index(col).is_none() {
                return Err(SqlliteError::sql(ResultCode::Error, format!("table {} has no column named {col}", create.table)));
            }
        }
        let (index_btree, root_page) = Btree::create_index(pager.clone())?;
        let table_btree = Btree::new(pager.clone(), table.root_page, BtreeFlags { intkey: true, blobkey: false });
        let mut cursor = table_btree.cursor();
        if cursor.first()? {
            loop {
                if let Some(rowid) = cursor.key() {
                    let values = cursor.values()?;
                    let key_values: Vec<Value> = create.columns.iter().map(|col| {
                        let idx = table.column_index(col).unwrap();
                        values.get(idx).cloned().unwrap_or(Value::Null)
                    }).collect();
                    btree_insert_index(&index_btree, &key_values, rowid)?;
                }
                if !cursor.next()? { break; }
            }
        }
        self.insert_schema_row(pager, &create.name, "index", root_page, &create_index_sql(create))?;
        self.indexes.insert(name_lower, Index { name: create.name.clone(), table: create.table.clone(), columns: create.columns.clone(), root_page, unique: create.unique });
        self.bump_cookie(); Ok(())
    }

    pub fn drop_table(&mut self, name: &str, if_exists: bool) -> Result<()> {
        let name_lower = name.to_lowercase();
        if !self.tables.contains_key(&name_lower) {
            if if_exists {
                return Ok(());
            }
            return Err(SqlliteError::sql(
                ResultCode::Error,
                format!("no such table: {name}"),
            ));
        }
        self.tables.remove(&name_lower);
        self.bump_cookie();
        Ok(())
    }

    pub fn set_table_root_page(
        &mut self,
        name: &str,
        root_page: u32,
    ) -> Result<()> {
        let name_lower = name.to_lowercase();
        let Some(table) = self.tables.get(&name_lower) else {
            return Ok(());
        };
        if table.root_page == root_page {
            return Ok(());
        }
        self.tables.get_mut(&name_lower).unwrap().root_page = root_page;

        let btree = self
            .schema_btree
            .as_ref()
            .ok_or_else(|| SqlliteError::sql(ResultCode::Internal, "schema btree not initialized"))?;
        let mut cursor = btree.cursor();
        if cursor.first()? {
            loop {
                let values = cursor.values()?;
                if values.len() >= 5 {
                    let obj_type = values[0].as_text().unwrap_or("");
                    let obj_name = values[1].as_text().unwrap_or("");
                    if obj_type == "table" && obj_name.eq_ignore_ascii_case(name) {
                        if let Some(rowid) = cursor.key() {
                            let mut updated = values;
                            updated[3] = Value::Integer(root_page as i64);
                            btree.replace(rowid, &encode_record(&updated))?;
                        }
                        break;
                    }
                }
                if !cursor.next()? {
                    break;
                }
            }
        }
        Ok(())
    }

    fn insert_schema_row(
        &mut self,
        _pager: Arc<Pager>,
        name: &str,
        obj_type: &str,
        root_page: u32,
        sql: &str,
    ) -> Result<()> {
        let btree = self
            .schema_btree
            .as_ref()
            .ok_or_else(|| SqlliteError::sql(ResultCode::Internal, "schema btree not initialized"))?;
        let rowid = self.next_schema_rowid(btree)?;
        btree_insert_row(
            btree,
            rowid,
            &[
                Value::Text(obj_type.into()),
                Value::Text(name.into()),
                Value::Text(name.into()),
                Value::Integer(root_page as i64),
                Value::Text(sql.into()),
            ],
        )?;
        Ok(())
    }

    fn next_schema_rowid(&self, btree: &Btree) -> Result<i64> {
        let mut cursor = btree.cursor();
        let mut max_id = 0i64;
        if cursor.first()? {
            loop {
                if let Some(id) = cursor.key() {
                    max_id = max_id.max(id);
                }
                if !cursor.next()? {
                    break;
                }
            }
        }
        Ok(max_id + 1)
    }

    pub fn init_schema_table(&mut self, pager: Arc<Pager>) -> Result<()> {
        let btree = Btree::new(
            pager.clone(),
            ROOT_PAGE,
            BtreeFlags {
                intkey: true,
                blobkey: false,
            },
        );
        self.schema_btree = Some(Arc::new(btree));

        let schema_table = Table {
            name: SCHEMA_TABLE_NAME.into(),
            root_page: ROOT_PAGE,
            columns: vec![
                Column {
                    name: "type".into(),
                    affinity: Affinity::Text,
                    not_null: false,
                    primary_key: false,
                    autoincrement: false,
                    default_value: None,
                },
                Column {
                    name: "name".into(),
                    affinity: Affinity::Text,
                    not_null: false,
                    primary_key: false,
                    autoincrement: false,
                    default_value: None,
                },
                Column {
                    name: "tbl_name".into(),
                    affinity: Affinity::Text,
                    not_null: false,
                    primary_key: false,
                    autoincrement: false,
                    default_value: None,
                },
                Column {
                    name: "rootpage".into(),
                    affinity: Affinity::Integer,
                    not_null: false,
                    primary_key: false,
                    autoincrement: false,
                    default_value: None,
                },
                Column {
                    name: "sql".into(),
                    affinity: Affinity::Text,
                    not_null: false,
                    primary_key: false,
                    autoincrement: false,
                    default_value: None,
                },
            ],
            rowid_alias: Some("rowid".into()),
        };
        self.tables
            .insert(SCHEMA_TABLE_NAME.to_lowercase(), schema_table);

        self.load_schema_from_btree()?;
        Ok(())
    }

    fn load_schema_from_btree(&mut self) -> Result<()> {
        let btree = self.schema_btree.as_ref().unwrap().clone();
        let mut cursor = btree.cursor();
        if !cursor.first()? {
            return Ok(());
        }
        loop {
            let values = cursor.values()?;
            if values.len() >= 5 {
                let obj_type = values[0].as_text().unwrap_or("");
                let name = values[1].as_text().unwrap_or("");
                let root_page = values[3].as_integer().unwrap_or(0) as u32;
                let sql = values[4].as_text().unwrap_or("");

                if obj_type == "table" {
                    let name_lower = name.to_lowercase();
                    if name_lower != SCHEMA_TABLE_NAME && name_lower != SCHEMA_TABLE_NAME_LEGACY {
                        if !sql.is_empty() {
                            if let Ok(Statement::CreateTable(create)) = parse_one(sql) {
                                self.register_table_from_create(create, root_page);
                            }
                        }
                    }
                } else if obj_type == "index" {
                    let name_lower = name.to_lowercase();
                    if !sql.is_empty() {
                        if let Ok(Statement::CreateIndex(create)) = parse_one(sql) {
                            self.indexes.insert(
                                name_lower,
                                Index {
                                    name: create.name.clone(),
                                    table: create.table.clone(),
                                    columns: create.columns.clone(),
                                    root_page,
                                    unique: create.unique,
                                },
                            );
                        }
                    }
                }
            }
            if !cursor.next()? {
                break;
            }
        }
        Ok(())
    }

    fn register_table_from_create(&mut self, create: CreateTable, root_page: u32) {
        let name_lower = create.name.to_lowercase();
        let columns = create
            .columns
            .iter()
            .map(column_def_to_column)
            .collect::<Vec<_>>();
        self.tables.insert(
            name_lower,
            Table {
                name: create.name.clone(),
                root_page,
                columns,
                rowid_alias: None,
            },
        );
    }
}

fn column_def_to_column(def: &ColumnDef) -> Column {
    let affinity = match def.col_type {
        ColumnType::Integer => Affinity::Integer,
        ColumnType::Real => Affinity::Real,
        ColumnType::Text => Affinity::Text,
        ColumnType::Blob => Affinity::Blob,
        _ => Affinity::Numeric,
    };
    let mut col = Column {
        name: def.name.clone(),
        affinity,
        not_null: false,
        primary_key: false,
        autoincrement: false,
        default_value: None,
    };
    for c in &def.constraints {
        match c {
            ColumnConstraint::PrimaryKey { autoincrement } => {
                col.primary_key = true;
                col.autoincrement = *autoincrement;
            }
            ColumnConstraint::NotNull => col.not_null = true,
            ColumnConstraint::Unique => {}
            ColumnConstraint::Default(expr) => {
                col.default_value = Some(expr_to_value(expr));
            }
        }
    }
    col
}

fn expr_to_value(expr: &sqllite_parser::ast::Expr) -> Value {
    use sqllite_parser::ast::Expr;
    match expr {
        Expr::Null => Value::Null,
        Expr::Integer(i) => Value::Integer(*i),
        Expr::Real(r) => Value::Real(*r),
        Expr::String(s) => Value::Text(s.clone()),
        Expr::Blob(b) => Value::Blob(b.clone()),
        _ => Value::Null,
    }
}

fn create_sql(create: &CreateTable) -> String {
    let mut sql = String::from("CREATE TABLE ");
    if create.if_not_exists {
        sql.push_str("IF NOT EXISTS ");
    }
    sql.push_str(&create.name);
    sql.push('(');
    for (i, col) in create.columns.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push_str(&col.name);
        sql.push(' ');
        sql.push_str(match col.col_type {
            ColumnType::Integer => "INTEGER",
            ColumnType::Real => "REAL",
            ColumnType::Text => "TEXT",
            ColumnType::Blob => "BLOB",
            ColumnType::Null => "NULL",
            ColumnType::Numeric => "NUMERIC",
        });
    }
    sql.push(')');
    sql
}

fn create_index_sql(create: &CreateIndex) -> String {
    let mut sql = String::from("CREATE "); if create.unique { sql.push_str("UNIQUE "); }
    sql.push_str("INDEX "); if create.if_not_exists { sql.push_str("IF NOT EXISTS "); }
    sql.push_str(&create.name); sql.push_str(" ON "); sql.push_str(&create.table); sql.push('(');
    for (i, col) in create.columns.iter().enumerate() { if i > 0 { sql.push_str(", "); } sql.push_str(col); }
    sql.push(')'); sql
}

/// Thread-safe schema wrapper.
pub type SharedSchema = Arc<RwLock<Schema>>;
