//! sqllite-core: Pure Rust SQLite-compatible database engine.

pub mod compile;
pub mod connection;
pub mod constants;
pub mod error;
pub mod io;
pub mod pragma;
pub mod record;
pub mod schema;
pub mod storage;
pub mod types;
pub mod varint;
pub mod vdbe;

pub use connection::{Connection, StatementHandle};
pub use error::{Result, ResultCode, SqlliteError};
pub use types::{Affinity, Value};
pub use vdbe::{Insn, Opcode, Program, Vdbe};
