//! Recursive descent SQL parser.

mod parser_impl;
pub use parser_impl::Parser;

use crate::ast::{ParseResult, Statement};
use crate::lexer::{tokenize, SpannedToken};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("parse error near \"{found}\": {message}")]
    Syntax { found: String, message: String },
    #[error("unexpected end of input")]
    Eof,
}

pub type Result<T> = std::result::Result<T, ParseError>;

/// Parse SQL source into statements.
pub fn parse(sql: &str) -> Result<ParseResult> {
    let tokens = tokenize(sql);
    let mut parser = Parser::new(tokens);
    let mut statements = Vec::new();
    while !parser.is_at_end() {
        statements.push(parser.parse_statement()?);
        parser.skip_semicolons();
    }
    Ok(ParseResult { statements })
}

/// Parse a single SQL statement.
pub fn parse_one(sql: &str) -> Result<Statement> {
    let tokens = tokenize(sql);
    let mut parser = Parser::new(tokens);
    let stmt = parser.parse_statement()?;
    Ok(stmt)
}
