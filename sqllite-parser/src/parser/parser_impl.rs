//! Parser implementation.

use crate::ast::*;
use crate::lexer::{SpannedToken, Token};
use crate::parser::ParseError;

pub struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<SpannedToken>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    pub fn skip_semicolons(&mut self) {
        while self.check(&Token::Semi) {
            self.advance();
        }
    }

    pub fn parse_statement(&mut self) -> Result<Statement, ParseError> {
        if self.match_token(&Token::Select) {
            return Ok(Statement::Select(self.parse_select()?));
        }
        if self.match_token(&Token::Insert) {
            return Ok(Statement::Insert(self.parse_insert()?));
        }
        if self.match_token(&Token::Update) {
            return Ok(Statement::Update(self.parse_update()?));
        }
        if self.match_token(&Token::Delete) {
            return Ok(Statement::Delete(self.parse_delete()?));
        }
        if self.match_token(&Token::Create) {
            let unique = self.match_token(&Token::Unique);
            if self.match_token(&Token::Index) {
                return Ok(Statement::CreateIndex(self.parse_create_index(unique)?));
            }
            self.expect(&Token::Table)?;
            return Ok(Statement::CreateTable(self.parse_create_table()?));
        }
        if self.match_token(&Token::Drop) {
            self.expect(&Token::Table)?;
            return Ok(Statement::DropTable(self.parse_drop_table()?));
        }
        if self.match_token(&Token::Pragma) {
            return Ok(Statement::Pragma(self.parse_pragma()?));
        }
        if self.match_token(&Token::Begin) {
            return Ok(Statement::Begin);
        }
        if self.match_token(&Token::Commit) {
            return Ok(Statement::Commit);
        }
        if self.match_token(&Token::Rollback) {
            return Ok(Statement::Rollback);
        }
        self.error("syntax error")
    }

    fn parse_select(&mut self) -> Result<Select, ParseError> {
        let distinct = self.match_token(&Token::Distinct);
        let columns = self.parse_result_columns()?;
        let from = if self.match_token(&Token::From) {
            Some(self.parse_table_ref()?)
        } else {
            None
        };
        let where_clause = if self.match_token(&Token::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let mut group_by = Vec::new();
        if self.match_token(&Token::Group) {
            self.expect(&Token::By)?;
            loop {
                group_by.push(self.parse_expr()?);
                if !self.match_token(&Token::Comma) {
                    break;
                }
            }
        }
        let having = if self.match_token(&Token::Having) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let mut order_by = Vec::new();
        if self.match_token(&Token::Order) {
            self.expect(&Token::By)?;
            loop {
                let expr = self.parse_expr()?;
                let desc = self.match_token(&Token::Desc);
                if !desc {
                    let _ = self.match_token(&Token::Asc);
                }
                order_by.push(OrderTerm { expr, desc });
                if !self.match_token(&Token::Comma) {
                    break;
                }
            }
        }
        let limit = if self.match_token(&Token::Limit) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let offset = if self.match_token(&Token::Comma) || self.match_token(&Token::Offset) {
            if !self.match_token(&Token::Offset) {
                // LIMIT offset, count form - swap
            }
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Select {
            distinct,
            columns,
            from,
            where_clause,
            group_by,
            having,
            order_by,
            limit,
            offset,
        })
    }

    fn parse_result_columns(&mut self) -> Result<Vec<ResultColumn>, ParseError> {
        let mut cols = Vec::new();
        if self.match_token(&Token::Star) {
            cols.push(ResultColumn {
                expr: Expr::Star,
                alias: None,
            });
            return Ok(cols);
        }
        loop {
            let expr = self.parse_expr()?;
            let alias = if self.match_token(&Token::As) {
                Some(self.parse_ident()?)
            } else if let Expr::Ident(ref name) = expr {
                // might be alias without AS
                None
            } else {
                None
            };
            cols.push(ResultColumn { expr, alias });
            if !self.match_token(&Token::Comma) {
                break;
            }
        }
        Ok(cols)
    }

    fn parse_table_ref(&mut self) -> Result<TableRef, ParseError> {
        let name = self.parse_ident()?;
        let alias = if self.check_ident() && !self.is_join_keyword() {
            Some(self.parse_ident()?)
        } else {
            None
        };
        let mut left = TableRef::Table { name, alias };
        while self.at_join() {
            let join_type = self.parse_join_type();
            self.expect(&Token::Join)?;
            let right_name = self.parse_ident()?;
            let right_alias = if self.check_ident() && !self.is_join_keyword() {
                Some(self.parse_ident()?)
            } else {
                None
            };
            let on = if self.match_token(&Token::On) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            left = TableRef::Join {
                left: Box::new(left),
                right: Box::new(TableRef::Table {
                    name: right_name,
                    alias: right_alias,
                }),
                join_type,
                on,
            };
        }
        Ok(left)
    }

    fn parse_insert(&mut self) -> Result<Insert, ParseError> {
        let or_conflict = self.parse_or_conflict();
        self.expect(&Token::Into)?;
        let table = self.parse_ident()?;
        let mut columns = Vec::new();
        if self.match_token(&Token::LParen) {
            loop {
                columns.push(self.parse_ident()?);
                if !self.match_token(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::RParen)?;
        }
        self.expect(&Token::Values)?;
        let mut values = Vec::new();
        loop {
            self.expect(&Token::LParen)?;
            let mut row = Vec::new();
            loop {
                row.push(self.parse_expr()?);
                if !self.match_token(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::RParen)?;
            values.push(row);
            if !self.match_token(&Token::Comma) {
                break;
            }
        }
        Ok(Insert {
            or_conflict,
            table,
            columns,
            values,
            select: None,
        })
    }

    fn parse_update(&mut self) -> Result<Update, ParseError> {
        let table = self.parse_ident()?;
        self.expect(&Token::Set)?;
        let mut assignments = Vec::new();
        loop {
            let col = self.parse_ident()?;
            self.expect(&Token::Eq)?;
            let val = self.parse_expr()?;
            assignments.push((col, val));
            if !self.match_token(&Token::Comma) {
                break;
            }
        }
        let where_clause = if self.match_token(&Token::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Update {
            table,
            assignments,
            where_clause,
        })
    }

    fn parse_delete(&mut self) -> Result<Delete, ParseError> {
        self.expect(&Token::From)?;
        let table = self.parse_ident()?;
        let where_clause = if self.match_token(&Token::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Delete {
            table,
            where_clause,
        })
    }

    fn parse_create_index(&mut self, unique: bool) -> Result<CreateIndex, ParseError> {
        let if_not_exists = self.match_token(&Token::If) && { self.expect(&Token::Not)?; self.expect(&Token::Exists)?; true };
        let name = self.parse_ident()?; self.expect(&Token::On)?; let table = self.parse_ident()?;
        self.expect(&Token::LParen)?; let mut columns = Vec::new();
        loop { columns.push(self.parse_ident()?); if !self.match_token(&Token::Comma) { break; } }
        self.expect(&Token::RParen)?;
        Ok(CreateIndex { if_not_exists, unique, name, table, columns })
    }

    fn parse_create_table(&mut self) -> Result<CreateTable, ParseError> {
        let if_not_exists = self.match_token(&Token::If) && {
            self.expect(&Token::Not)?;
            self.expect(&Token::Exists)?;
            true
        };
        let name = self.parse_ident()?;
        self.expect(&Token::LParen)?;
        let mut columns = Vec::new();
        loop {
            columns.push(self.parse_column_def()?);
            if !self.match_token(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        Ok(CreateTable {
            if_not_exists,
            name,
            columns,
            temp: false,
        })
    }

    fn parse_column_def(&mut self) -> Result<ColumnDef, ParseError> {
        let name = self.parse_ident()?;
        let col_type = self.parse_column_type();
        let mut constraints = Vec::new();
        while self.match_token(&Token::Primary) {
            self.expect(&Token::Key)?;
            let autoincrement = self.match_token(&Token::Autoincrement);
            constraints.push(ColumnConstraint::PrimaryKey { autoincrement });
        }
        if self.match_token(&Token::Not) {
            self.expect(&Token::Null)?;
            constraints.push(ColumnConstraint::NotNull);
        }
        if self.match_token(&Token::Unique) {
            constraints.push(ColumnConstraint::Unique);
        }
        if self.match_token(&Token::Default) {
            constraints.push(ColumnConstraint::Default(self.parse_expr()?));
        }
        Ok(ColumnDef {
            name,
            col_type,
            constraints,
        })
    }

    fn parse_column_type(&mut self) -> ColumnType {
        if self.match_token(&Token::Integer) {
            ColumnType::Integer
        } else if self.match_token(&Token::Real) {
            ColumnType::Real
        } else if self.match_token(&Token::Text) {
            ColumnType::Text
        } else if self.match_token(&Token::Blob) {
            ColumnType::Blob
        } else if self.match_token(&Token::Null) {
            ColumnType::Null
        } else if let Some(t) = self.current_token() {
            if let Token::Ident(name) = &t.token {
                let name = name.to_ascii_lowercase();
                self.advance();
                match name.as_str() {
                    "int" | "integer" => ColumnType::Integer,
                    "float" | "double" | "real" => ColumnType::Real,
                    "varchar" | "char" | "character" | "text" | "clob" => ColumnType::Text,
                    "blob" => ColumnType::Blob,
                    "boolean" | "bool" => ColumnType::Integer,
                    "numeric" | "decimal" => ColumnType::Numeric,
                    _ => ColumnType::Numeric,
                }
            } else {
                ColumnType::Numeric
            }
        } else {
            ColumnType::Numeric
        }
    }

    fn parse_drop_table(&mut self) -> Result<DropTable, ParseError> {
        let if_exists = self.match_token(&Token::If) && {
            self.expect(&Token::Exists)?;
            true
        };
        let name = self.parse_ident()?;
        Ok(DropTable { if_exists, name })
    }

    fn parse_pragma(&mut self) -> Result<Pragma, ParseError> {
        let name = self.parse_ident()?;
        let value = if self.match_token(&Token::Eq) {
            Some(self.parse_expr()?)
        } else if self.match_token(&Token::LParen) {
            let arg = self.parse_expr()?;
            self.expect(&Token::RParen)?;
            Some(arg)
        } else {
            None
        };
        Ok(Pragma { name, value })
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and_expr()?;
        while self.match_token(&Token::Or) {
            let right = self.parse_and_expr()?;
            left = Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not_expr()?;
        while self.match_token(&Token::And) {
            let right = self.parse_not_expr()?;
            left = Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_not_expr(&mut self) -> Result<Expr, ParseError> {
        if self.match_token(&Token::Not) {
            let expr = self.parse_not_expr()?;
            return Ok(Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            });
        }
        self.parse_comparison_expr()
    }

    fn parse_comparison_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_additive_expr()?;
        loop {
            let op = if self.match_token(&Token::Eq) || self.match_token(&Token::EqEq) {
                BinaryOp::Eq
            } else if self.match_token(&Token::NotEq) {
                BinaryOp::NotEq
            } else if self.match_token(&Token::Lt) {
                BinaryOp::Lt
            } else if self.match_token(&Token::Le) {
                BinaryOp::Le
            } else if self.match_token(&Token::Gt) {
                BinaryOp::Gt
            } else if self.match_token(&Token::Ge) {
                BinaryOp::Ge
            } else if self.match_token(&Token::Like) {
                BinaryOp::Like
            } else if self.match_token(&Token::Is) {
                if self.match_token(&Token::Not) {
                    BinaryOp::IsNot
                } else {
                    BinaryOp::Is
                }
            } else {
                break;
            };
            let right = self.parse_additive_expr()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_additive_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplicative_expr()?;
        loop {
            let op = if self.match_token(&Token::Plus) {
                BinaryOp::Add
            } else if self.match_token(&Token::Minus) {
                BinaryOp::Sub
            } else if self.match_token(&Token::Concat) {
                BinaryOp::Concat
            } else {
                break;
            };
            let right = self.parse_multiplicative_expr()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_multiplicative_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary_expr()?;
        loop {
            let op = if self.match_token(&Token::Star) {
                BinaryOp::Mul
            } else if self.match_token(&Token::Slash) {
                BinaryOp::Div
            } else if self.match_token(&Token::Percent) {
                BinaryOp::Mod
            } else {
                break;
            };
            let right = self.parse_unary_expr()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self) -> Result<Expr, ParseError> {
        if self.match_token(&Token::Minus) {
            let expr = self.parse_unary_expr()?;
            return Ok(Expr::UnaryOp {
                op: UnaryOp::Minus,
                expr: Box::new(expr),
            });
        }
        if self.match_token(&Token::Plus) {
            return self.parse_unary_expr();
        }
        self.parse_primary_expr()
    }

    fn parse_primary_expr(&mut self) -> Result<Expr, ParseError> {
        if self.match_token(&Token::Null) {
            return Ok(Expr::Null);
        }
        let current = self.current_token().map(|t| t.token.clone());
        if let Some(token) = current {
            match token {
                Token::IntegerLit(n) => {
                    self.advance();
                    return Ok(Expr::Integer(n));
                }
                Token::RealLit(r) => {
                    self.advance();
                    return Ok(Expr::Real(r));
                }
                Token::StringLit(s) => {
                    self.advance();
                    return Ok(Expr::String(s));
                }
                Token::BlobLit(b) => {
                    self.advance();
                    return Ok(Expr::Blob(b));
                }
                Token::Star => {
                    self.advance();
                    return Ok(Expr::Star);
                }
                Token::LParen => {
                    self.advance();
                    if self.match_token(&Token::Select) {
                        let select = self.parse_select()?;
                        self.expect(&Token::RParen)?;
                        return Ok(Expr::Ident(format!("({select:?})")));
                    }
                    let expr = self.parse_expr()?;
                    self.expect(&Token::RParen)?;
                    return Ok(expr);
                }
                Token::Count => {
                    self.advance();
                    self.expect(&Token::LParen)?;
                    let distinct = self.match_token(&Token::Distinct);
                    let args = if self.match_token(&Token::Star) {
                        vec![Expr::Star]
                    } else {
                        let mut a = vec![self.parse_expr()?];
                        while self.match_token(&Token::Comma) {
                            a.push(self.parse_expr()?);
                        }
                        a
                    };
                    self.expect(&Token::RParen)?;
                    return Ok(Expr::Function {
                        name: "count".into(),
                        distinct,
                        args,
                    });
                }
                Token::Ident(name) | Token::QuotedIdent(name) | Token::BacktickIdent(name) => {
                    self.advance();
                    if self.match_token(&Token::LParen) {
                        let mut args = Vec::new();
                        if !self.check(&Token::RParen) {
                            loop {
                                args.push(self.parse_expr()?);
                                if !self.match_token(&Token::Comma) {
                                    break;
                                }
                            }
                        }
                        self.expect(&Token::RParen)?;
                        return Ok(Expr::Function {
                            name,
                            distinct: false,
                            args,
                        });
                    }
                    if self.match_token(&Token::Dot) {
                        let col = self.parse_ident()?;
                        return Ok(Expr::QualifiedIdent {
                            table: name,
                            column: col,
                        });
                    }
                    return Ok(Expr::Ident(name));
                }
                _ => {}
            }
        }
        self.error("expected expression")
    }

    fn parse_ident(&mut self) -> Result<String, ParseError> {
        if let Some(t) = self.current_token() {
            match &t.token {
                Token::Ident(s) | Token::QuotedIdent(s) | Token::BacktickIdent(s) => {
                    let s = s.clone();
                    self.advance();
                    return Ok(s);
                }
                _ => {}
            }
        }
        self.error("expected identifier")
    }

    fn parse_or_conflict(&mut self) -> Option<ConflictAction> {
        if !self.match_token(&Token::Or) {
            return None;
        }
        if self.match_token(&Token::Rollback) {
            Some(ConflictAction::Rollback)
        } else if self.match_token(&Token::AbortKw) {
            Some(ConflictAction::Abort)
        } else if self.match_token(&Token::Fail) {
            Some(ConflictAction::Fail)
        } else if self.match_token(&Token::Ignore) {
            Some(ConflictAction::Ignore)
        } else if self.match_token(&Token::Replace) {
            Some(ConflictAction::Replace)
        } else {
            None
        }
    }

    // Token helpers
    fn current_token(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&SpannedToken> {
        if !self.is_at_end() {
            self.pos += 1;
        }
        self.tokens.get(self.pos - 1)
    }

    fn check(&self, token: &Token) -> bool {
        self.current_token()
            .map(|t| std::mem::discriminant(&t.token) == std::mem::discriminant(token))
            .unwrap_or(false)
    }

    fn check_ident(&self) -> bool {
        matches!(
            self.current_token().map(|t| &t.token),
            Some(Token::Ident(_)) | Some(Token::QuotedIdent(_)) | Some(Token::BacktickIdent(_))
        )
    }

    fn match_token(&mut self, token: &Token) -> bool {
        if self.check(token) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &Token) -> Result<(), ParseError> {
        if self.match_token(token) {
            Ok(())
        } else {
            self.error(&format!("expected {:?}", std::mem::discriminant(token)))
        }
    }

    fn at_join(&self) -> bool {
        matches!(
            self.current_token().map(|t| &t.token),
            Some(Token::Join) | Some(Token::Inner) | Some(Token::Left) | Some(Token::Cross)
        )
    }

    fn parse_join_type(&mut self) -> JoinType {
        if self.match_token(&Token::Cross) {
            JoinType::Cross
        } else if self.match_token(&Token::Left) {
            JoinType::Left
        } else if self.match_token(&Token::Inner) {
            JoinType::Inner
        } else {
            JoinType::Inner
        }
    }

    fn match_join_type(&mut self) -> bool {
        self.match_token(&Token::Inner)
            || self.match_token(&Token::Left)
            || self.match_token(&Token::Cross)
    }

    fn current_join_type(&self) -> JoinType {
        JoinType::Inner
    }

    fn is_join_keyword(&self) -> bool {
        matches!(
            self.current_token().map(|t| &t.token),
            Some(Token::Join) | Some(Token::On) | Some(Token::Where) | Some(Token::Order)
                | Some(Token::Group) | Some(Token::Limit) | Some(Token::Comma)
        )
    }

    fn error<T>(&self, message: &str) -> Result<T, ParseError> {
        let found = self
            .current_token()
            .map(|t| format!("{:?}", t.token))
            .unwrap_or_else(|| "end of input".into());
        Err(ParseError::Syntax {
            found,
            message: message.into(),
        })
    }
}
