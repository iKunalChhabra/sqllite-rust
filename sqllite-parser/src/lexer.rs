//! SQL lexer using logos.

use logos::Logos;

/// SQL token types.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n\f]+")]
#[logos(skip r"--[^\n]*")]
#[logos(skip r"/\*([^*]|\*[^/])*\*/")]
pub enum Token {
    #[regex("(?i)SELECT")]
    Select,
    #[regex("(?i)FROM")]
    From,
    #[regex("(?i)WHERE")]
    Where,
    #[regex("(?i)INSERT")]
    Insert,
    #[regex("(?i)INTO")]
    Into,
    #[regex("(?i)VALUES")]
    Values,
    #[regex("(?i)UPDATE")]
    Update,
    #[regex("(?i)SET")]
    Set,
    #[regex("(?i)DELETE")]
    Delete,
    #[regex("(?i)CREATE")]
    Create,
    #[regex("(?i)TABLE")]
    Table,
    #[regex("(?i)INDEX")]
    Index,
    #[regex("(?i)DROP")]
    Drop,
    #[regex("(?i)IF")]
    If,
    #[regex("(?i)NOT")]
    Not,
    #[regex("(?i)EXISTS")]
    Exists,
    #[regex("(?i)PRIMARY")]
    Primary,
    #[regex("(?i)KEY")]
    Key,
    #[regex("(?i)AUTOINCREMENT")]
    Autoincrement,
    #[regex("(?i)NULL")]
    Null,
    #[regex("(?i)INTEGER")]
    Integer,
    #[regex("(?i)REAL")]
    Real,
    #[regex("(?i)TEXT")]
    Text,
    #[regex("(?i)BLOB")]
    Blob,
    #[regex("(?i)AND")]
    And,
    #[regex("(?i)OR")]
    Or,
    #[regex("(?i)ORDER")]
    Order,
    #[regex("(?i)BY")]
    By,
    #[regex("(?i)ASC")]
    Asc,
    #[regex("(?i)DESC")]
    Desc,
    #[regex("(?i)LIMIT")]
    Limit,
    #[regex("(?i)BEGIN")]
    Begin,
    #[regex("(?i)COMMIT")]
    Commit,
    #[regex("(?i)ROLLBACK")]
    Rollback,
    #[regex("(?i)TRANSACTION")]
    Transaction,
    #[regex("(?i)PRAGMA")]
    Pragma,
    #[regex("(?i)UNIQUE")]
    Unique,
    #[regex("(?i)DEFAULT")]
    Default,
    #[regex("(?i)AS")]
    As,
    #[regex("(?i)ON")]
    On,
    #[regex("(?i)JOIN")]
    Join,
    #[regex("(?i)LEFT")]
    Left,
    #[regex("(?i)INNER")]
    Inner,
    #[regex("(?i)CROSS")]
    Cross,
    #[regex("(?i)GROUP")]
    Group,
    #[regex("(?i)HAVING")]
    Having,
    #[regex("(?i)DISTINCT")]
    Distinct,
    #[regex("(?i)COUNT")]
    Count,
    #[regex("(?i)LIKE")]
    Like,
    #[regex("(?i)IS")]
    Is,
    #[regex("(?i)IN")]
    In,
    #[regex("(?i)BETWEEN")]
    Between,
    #[regex("(?i)CASE")]
    Case,
    #[regex("(?i)WHEN")]
    When,
    #[regex("(?i)THEN")]
    Then,
    #[regex("(?i)ELSE")]
    Else,
    #[regex("(?i)END")]
    End,
    #[regex("(?i)COLLATE")]
    Collate,
    #[regex("(?i)CONSTRAINT")]
    Constraint,
    #[regex("(?i)FOREIGN")]
    Foreign,
    #[regex("(?i)REFERENCES")]
    References,
    #[regex("(?i)CHECK")]
    Check,
    #[regex("(?i)VIEW")]
    View,
    #[regex("(?i)TRIGGER")]
    Trigger,
    #[regex("(?i)TEMP")]
    Temp,
    #[regex("(?i)TEMPORARY")]
    Temporary,
    #[regex("(?i)WITHOUT")]
    Without,
    #[regex("(?i)ROWID")]
    Rowid,
    #[regex("(?i)REPLACE")]
    Replace,
    #[regex("(?i)IGNORE")]
    Ignore,
    #[regex("(?i)FAIL")]
    Fail,
    #[regex("(?i)ABORT")]
    AbortKw,
    #[regex("(?i)WITH")]
    With,
    #[regex("(?i)UNION")]
    Union,
    #[regex("(?i)ALL")]
    All,
    #[regex("(?i)EXCEPT")]
    Except,
    #[regex("(?i)INTERSECT")]
    Intersect,
    #[regex("(?i)OFFSET")]
    Offset,

    #[token("||")]
    Concat,
    #[token("==")]
    EqEq,
    #[token("!=")]
    #[token("<>")]
    NotEq,
    #[token("<=")]
    Le,
    #[token(">=")]
    Ge,
    #[token("<<")]
    Shl,
    #[token(">>")]
    Shr,
    #[token("=")]
    Eq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("~")]
    Tilde,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token(",")]
    Comma,
    #[token(";")]
    Semi,
    #[token(".")]
    Dot,

    #[regex(r"-?[0-9]+", |lex| lex.slice().parse().ok())]
    IntegerLit(i64),
    #[regex(r"-?[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?", parse_float)]
    RealLit(f64),
    #[regex(r"'([^'\\]|\\.|'')*'", parse_string)]
    StringLit(String),
    #[regex(r#""([^"\\]|\\.|"")*""#, parse_ident_string)]
    QuotedIdent(String),
    #[regex(r"`([^`\\]|\\.|``)*`", parse_backtick_ident)]
    BacktickIdent(String),
    #[regex(r"[xX]'[0-9a-fA-F]*'", parse_blob)]
    BlobLit(Vec<u8>),
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),
    #[token("?")]
    BindParam,
    #[regex(r":[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice()[1..].to_string())]
    NamedParam(String),
    #[regex(r"@[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice()[1..].to_string())]
    AtParam(String),
    #[regex(r"\$[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice()[1..].to_string())]
    DollarParam(String),
}

fn parse_float(lex: &mut logos::Lexer<'_, Token>) -> Option<f64> {
    lex.slice().parse().ok()
}

fn parse_string(lex: &mut logos::Lexer<'_, Token>) -> Option<String> {
    let s = lex.slice();
    let inner = &s[1..s.len() - 1];
    Some(unescape_sql_string(inner))
}

fn parse_ident_string(lex: &mut logos::Lexer<'_, Token>) -> Option<String> {
    let s = lex.slice();
    let inner = &s[1..s.len() - 1];
    Some(inner.replace("\"\"", "\""))
}

fn parse_backtick_ident(lex: &mut logos::Lexer<'_, Token>) -> Option<String> {
    let s = lex.slice();
    let inner = &s[1..s.len() - 1];
    Some(inner.replace("``", "`"))
}

fn parse_blob(lex: &mut logos::Lexer<'_, Token>) -> Option<Vec<u8>> {
    let s = lex.slice();
    let hex = &s[2..s.len() - 1];
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).ok()?;
        bytes.push(byte);
    }
    Some(bytes)
}

fn unescape_sql_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                result.push('\'');
            } else {
                result.push('\'');
            }
        } else if c == '\\' {
            if let Some(next) = chars.next() {
                result.push(next);
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// A token with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub start: usize,
    pub end: usize,
}

/// Tokenize SQL source into a list of tokens.
pub fn tokenize(sql: &str) -> Vec<SpannedToken> {
    let mut lexer = Token::lexer(sql);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next() {
        if let Ok(t) = token {
            let start = lexer.span().start;
            let end = lexer.span().end;
            tokens.push(SpannedToken {
                token: t,
                start,
                end,
            });
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_select() {
        let tokens = tokenize("SELECT * FROM t WHERE x = 1");
        assert!(tokens.iter().any(|t| t.token == Token::Select));
        assert!(tokens.iter().any(|t| t.token == Token::Star));
        assert!(tokens.iter().any(|t| matches!(t.token, Token::IntegerLit(1))));
    }
}
