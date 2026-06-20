//! SQLite-compatible result and error codes.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, SqlliteError>;

/// SQLite result codes matching the C API.
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ResultCode {
    Ok = 0,
    Error = 1,
    Internal = 2,
    Perm = 3,
    Abort = 4,
    Busy = 5,
    Locked = 6,
    NoMem = 7,
    ReadOnly = 8,
    Interrupt = 9,
    IoErr = 10,
    Corrupt = 11,
    NotFound = 12,
    Full = 13,
    CantOpen = 14,
    Protocol = 15,
    Empty = 16,
    Schema = 17,
    TooBig = 18,
    Constraint = 19,
    Mismatch = 20,
    Misuse = 21,
    NoLfs = 22,
    Auth = 23,
    Format = 24,
    Range = 25,
    NotADb = 26,
    Notice = 27,
    Warning = 28,
    Row = 100,
    Done = 101,
}

impl ResultCode {
    pub fn is_ok(self) -> bool {
        matches!(self, ResultCode::Ok | ResultCode::Row | ResultCode::Done)
    }

    pub fn message(self) -> &'static str {
        match self {
            ResultCode::Ok => "not an error",
            ResultCode::Error => "SQL logic error",
            ResultCode::Internal => "internal logic error",
            ResultCode::Perm => "access permission denied",
            ResultCode::Abort => "callback requested query abort",
            ResultCode::Busy => "database is locked",
            ResultCode::Locked => "database table is locked",
            ResultCode::NoMem => "out of memory",
            ResultCode::ReadOnly => "attempt to write a readonly database",
            ResultCode::Interrupt => "operation interrupted",
            ResultCode::IoErr => "disk I/O error",
            ResultCode::Corrupt => "database disk image is malformed",
            ResultCode::NotFound => "unknown operation",
            ResultCode::Full => "database or disk is full",
            ResultCode::CantOpen => "unable to open database file",
            ResultCode::Protocol => "locking protocol",
            ResultCode::Empty => "table contains no data",
            ResultCode::Schema => "database schema has changed",
            ResultCode::TooBig => "string or blob too big",
            ResultCode::Constraint => "constraint failed",
            ResultCode::Mismatch => "datatype mismatch",
            ResultCode::Misuse => "library routine called out of sequence",
            ResultCode::NoLfs => "large file support is disabled",
            ResultCode::Auth => "authorization denied",
            ResultCode::Format => "auxiliary database format error",
            ResultCode::Range => "bind or column index out of range",
            ResultCode::NotADb => "file is not a database",
            ResultCode::Notice => "notification message",
            ResultCode::Warning => "warning message",
            ResultCode::Row => "another row available",
            ResultCode::Done => "execution finished",
        }
    }
}

impl fmt::Display for ResultCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

#[derive(Debug, Error)]
pub enum SqlliteError {
    #[error("{code}: {message}")]
    Sql { code: ResultCode, message: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parse error: {0}")]
    Parse(String),
}

impl SqlliteError {
    pub fn sql(code: ResultCode, message: impl Into<String>) -> Self {
        Self::Sql {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> ResultCode {
        match self {
            SqlliteError::Sql { code, .. } => *code,
            SqlliteError::Io(_) => ResultCode::IoErr,
            SqlliteError::Parse(_) => ResultCode::Error,
        }
    }

    pub fn message(&self) -> String {
        match self {
            SqlliteError::Sql { message, .. } => message.clone(),
            SqlliteError::Io(e) => e.to_string(),
            SqlliteError::Parse(m) => m.clone(),
        }
    }
}
