//! Abstract syntax tree for SQL statements.

/// SQL data type in column definition.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnType {
    Null,
    Integer,
    Real,
    Text,
    Blob,
    Numeric,
}

/// Column constraint.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnConstraint {
    PrimaryKey { autoincrement: bool },
    NotNull,
    Unique,
    Default(Expr),
}

/// Column definition in CREATE TABLE.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: ColumnType,
    pub constraints: Vec<ColumnConstraint>,
}

/// Expression AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Null,
    Integer(i64),
    Real(f64),
    String(String),
    Blob(Vec<u8>),
    Ident(String),
    QualifiedIdent { table: String, column: String },
    Star,
    QualifiedStar(String),
    UnaryOp { op: UnaryOp, expr: Box<Expr> },
    BinaryOp {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Function {
        name: String,
        distinct: bool,
        args: Vec<Expr>,
    },
    IsNull(Box<Expr>),
    IsNotNull(Box<Expr>),
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
    },
    InList {
        expr: Box<Expr>,
        values: Vec<Expr>,
    },
    Case {
        base: Option<Box<Expr>>,
        when_then: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },
    Cast { expr: Box<Expr>, to_type: ColumnType },
    Collate { expr: Box<Expr>, collation: String },
    Bind(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Minus,
    Plus,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    And,
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
    Like,
    NotLike,
    BitAnd,
    BitOr,
    Shl,
    Shr,
    Is,
    IsNot,
}

/// ORDER BY term.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderTerm {
    pub expr: Expr,
    pub desc: bool,
}

/// SELECT result column.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultColumn {
    pub expr: Expr,
    pub alias: Option<String>,
}

/// Table reference in FROM clause.
#[derive(Debug, Clone, PartialEq)]
pub enum TableRef {
    Table {
        name: String,
        alias: Option<String>,
    },
    Subquery {
        select: Box<Select>,
        alias: String,
    },
    Join {
        left: Box<TableRef>,
        right: Box<TableRef>,
        join_type: JoinType,
        on: Option<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Cross,
    Inner,
    Left,
}

/// SELECT statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    pub distinct: bool,
    pub columns: Vec<ResultColumn>,
    pub from: Option<TableRef>,
    pub where_clause: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub order_by: Vec<OrderTerm>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
}

/// INSERT statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    pub or_conflict: Option<ConflictAction>,
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<Vec<Expr>>,
    pub select: Option<Select>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    Rollback,
    Abort,
    Fail,
    Ignore,
    Replace,
}

/// UPDATE statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub table: String,
    pub assignments: Vec<(String, Expr)>,
    pub where_clause: Option<Expr>,
}

/// DELETE statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Delete {
    pub table: String,
    pub where_clause: Option<Expr>,
}

/// CREATE TABLE statement.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable {
    pub if_not_exists: bool,
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub temp: bool,
}

/// DROP TABLE statement.
#[derive(Debug, Clone, PartialEq)]
pub struct DropTable {
    pub if_exists: bool,
    pub name: String,
}

/// PRAGMA statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Pragma {
    pub name: String,
    pub value: Option<Expr>,
}

/// Top-level SQL statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Select(Select),
    Insert(Insert),
    Update(Update),
    Delete(Delete),
    CreateTable(CreateTable),
    DropTable(DropTable),
    Pragma(Pragma),
    Begin,
    Commit,
    Rollback,
}

/// Parse result.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseResult {
    pub statements: Vec<Statement>,
}
