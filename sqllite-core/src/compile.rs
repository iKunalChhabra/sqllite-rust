//! SQL to VDBE code generation.

#[path = "compile_select.rs"]
mod compile_select;

use crate::error::{Result, ResultCode, SqlliteError};
use crate::schema::Schema;
use crate::vdbe::program::{Insn, InsnP4, Opcode, Program};
use sqllite_parser::ast::*;
use sqllite_parser::parse_one;

use compile_select::compile_select;

/// Compile a SQL statement into a VDBE program.
pub fn compile(sql: &str, schema: &Schema) -> Result<Program> {
    let stmt = parse_one(sql).map_err(|e| SqlliteError::Parse(e.to_string()))?;
    match stmt {
        Statement::Select(s) => compile_select(&s, schema),
        Statement::Insert(i) => compile_insert(&i, schema),
        Statement::Update(u) => compile_update(&u, schema),
        Statement::Delete(d) => compile_delete(&d, schema),
        Statement::CreateTable(c) => compile_create_table(&c),
        Statement::CreateIndex(_) => compile_create_index(),
        Statement::DropTable(d) => compile_drop_table(&d),
        Statement::Pragma(p) => compile_pragma(&p),
        Statement::Begin => compile_begin(),
        Statement::Commit => compile_commit(),
        Statement::Rollback => compile_rollback(),
    }
}

fn compile_insert(insert: &Insert, schema: &Schema) -> Result<Program> {
    let table = schema.table(&insert.table).ok_or_else(|| {
        SqlliteError::sql(ResultCode::Error, format!("no such table: {}", insert.table))
    })?;

    for col in &insert.columns {
        if table.column_index(col).is_none() {
            return Err(SqlliteError::sql(
                ResultCode::Error,
                format!("table {} has no column named {col}", insert.table),
            ));
        }
    }

    let mut prog = Program::new();
    prog.n_reg = 16;

    prog.emit(Insn {
        opcode: Opcode::Init,
        p1: 0,
        p2: 1,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    prog.emit(Insn {
        opcode: Opcode::Transaction,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    let cursor = 0;
    prog.emit(Insn {
        opcode: Opcode::OpenWrite,
        p1: cursor,
        p2: 0,
        p3: 0,
        p4: InsnP4::String(insert.table.clone()),
        p5: 0,
    });

    for row in &insert.values {
        let n_cols = if insert.columns.is_empty() {
            table.column_count()
        } else {
            insert.columns.len()
        };

        if row.len() != n_cols {
            return Err(SqlliteError::sql(
                ResultCode::Error,
                format!(
                    "table {} has {} columns but {} values were supplied",
                    insert.table,
                    n_cols,
                    row.len()
                ),
            ));
        }

        for (i, expr) in row.iter().enumerate() {
            compile_expr(&mut prog, expr, i as i32 + 2)?;
        }

        prog.emit(Insn {
            opcode: Opcode::NewRowid,
            p1: 1,
            p2: cursor,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });

        prog.emit(Insn {
            opcode: Opcode::MakeRecord,
            p1: n_cols as i32,
            p2: 2,
            p3: 10,
            p4: InsnP4::None,
            p5: 0,
        });

        prog.emit(Insn {
            opcode: Opcode::Insert,
            p1: 10,
            p2: cursor,
            p3: 1,
            p4: InsnP4::None,
            p5: 0,
        });
    }

    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    Ok(prog)
}

fn compile_delete(delete: &Delete, schema: &Schema) -> Result<Program> {
    if schema.table(&delete.table).is_none() {
        return Err(SqlliteError::sql(
            ResultCode::Error,
            format!("no such table: {}", delete.table),
        ));
    }

    let mut prog = Program::new();
    prog.emit(Insn {
        opcode: Opcode::Init,
        p1: 0,
        p2: 1,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::Transaction,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    let cursor = 0;
    prog.emit(Insn {
        opcode: Opcode::OpenWrite,
        p1: cursor,
        p2: 0,
        p3: 0,
        p4: InsnP4::String(delete.table.clone()),
        p5: 0,
    });
    let rewind = prog.emit(Insn {
        opcode: Opcode::Rewind,
        p1: cursor,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    let where_skip = if let Some(ref w) = delete.where_clause {
        compile_where_expr(&mut prog, w, schema.table(&delete.table).unwrap(), cursor, 0)?;
        let addr = prog.emit(Insn {
            opcode: Opcode::IfNot,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
        Some(addr)
    } else {
        None
    };

    prog.emit(Insn {
        opcode: Opcode::Delete,
        p1: 0,
        p2: cursor,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    let next_addr = prog.insns.len();
    if let Some(addr) = where_skip {
        prog.patch_p2(addr, next_addr as i32);
    }

    let loop_top = (rewind + 1) as i32;
    prog.emit(Insn {
        opcode: Opcode::Last,
        p1: cursor,
        p2: loop_top,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    let halt_addr = prog.insns.len();
    prog.patch_p2(rewind, halt_addr as i32);

    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    Ok(prog)
}

fn compile_update(update: &Update, schema: &Schema) -> Result<Program> {
    let table = schema.table(&update.table).ok_or_else(|| SqlliteError::sql(ResultCode::Error, format!("no such table: {}", update.table)))?;
    for (col, _) in &update.assignments {
        if table.column_index(col).is_none() {
            return Err(SqlliteError::sql(ResultCode::Error, format!("table {} has no column named {col}", update.table)));
        }
    }
    let col_count = table.column_count(); let mut prog = Program::new(); prog.n_reg = col_count + 16;
    prog.emit(Insn { opcode: Opcode::Init, p1: 0, p2: 1, p3: 0, p4: InsnP4::None, p5: 0 });
    prog.emit(Insn { opcode: Opcode::Transaction, p1: 0, p2: 0, p3: 0, p4: InsnP4::None, p5: 0 });
    let cursor = 0;
    prog.emit(Insn { opcode: Opcode::OpenWrite, p1: cursor, p2: 0, p3: 0, p4: InsnP4::String(update.table.clone()), p5: 0 });
    let rewind = prog.emit(Insn { opcode: Opcode::Rewind, p1: cursor, p2: 0, p3: 0, p4: InsnP4::None, p5: 0 });
    let where_skip = if let Some(ref w) = update.where_clause {
        compile_where_expr(&mut prog, w, table, cursor, 0)?;
        Some(prog.emit(Insn { opcode: Opcode::IfNot, p1: 0, p2: 0, p3: 0, p4: InsnP4::None, p5: 0 }))
    } else { None };
    for j in 0..col_count { prog.emit(Insn { opcode: Opcode::Column, p1: (j + 2) as i32, p2: cursor, p3: j as i32, p4: InsnP4::None, p5: 0 }); }
    for (col_name, expr) in &update.assignments {
        let col_idx = table.column_index(col_name).unwrap();
        compile_expr_with_columns(&mut prog, expr, Some(table), cursor, (col_idx + 2) as i32)?;
    }
    prog.emit(Insn { opcode: Opcode::Rowid, p1: 1, p2: cursor, p3: 0, p4: InsnP4::None, p5: 0 });
    prog.emit(Insn { opcode: Opcode::MakeRecord, p1: col_count as i32, p2: 2, p3: 10, p4: InsnP4::None, p5: 0 });
    prog.emit(Insn { opcode: Opcode::Insert, p1: 10, p2: cursor, p3: 1, p4: InsnP4::None, p5: 1 });
    let next_addr = prog.insns.len(); if let Some(addr) = where_skip { prog.patch_p2(addr, next_addr as i32); }
    prog.emit(Insn { opcode: Opcode::Last, p1: cursor, p2: (rewind + 1) as i32, p3: 0, p4: InsnP4::None, p5: 0 });
    prog.patch_p2(rewind, prog.insns.len() as i32);
    prog.emit(Insn { opcode: Opcode::Halt, p1: 0, p2: 0, p3: 0, p4: InsnP4::None, p5: 0 });
    Ok(prog)
}

fn compile_create_index() -> Result<Program> { Ok(Program { insns: vec![Insn { opcode: Opcode::Halt, p1: 0, p2: 0, p3: 0, p4: InsnP4::None, p5: 0 }], n_reg: 0, read_only: false }) }

fn compile_create_table(create: &CreateTable) -> Result<Program> {
    let mut prog = Program::new();
    prog.emit(Insn {
        opcode: Opcode::Init,
        p1: 0,
        p2: 1,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::Transaction,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::CreateBtree,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::String(create.name.clone()),
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    Ok(prog)
}

fn compile_drop_table(drop: &DropTable) -> Result<Program> {
    let mut prog = Program::new();
    prog.emit(Insn {
        opcode: Opcode::DropTable,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::String(drop.name.clone()),
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    Ok(prog)
}

fn compile_pragma(pragma: &Pragma) -> Result<Program> {
    let mut prog = Program::new();
    prog.read_only = true;
    prog.n_reg = 8;
    let value = pragma.value.as_ref().map(expr_to_pragma_value);
    prog.emit(Insn {
        opcode: Opcode::Pragma,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::Pragma {
            name: pragma.name.clone(),
            value,
        },
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    Ok(prog)
}

fn expr_to_pragma_value(expr: &Expr) -> String {
    match expr {
        Expr::String(s) => s.clone(),
        Expr::Ident(s) => s.clone(),
        Expr::Integer(i) => i.to_string(),
        Expr::Real(r) => r.to_string(),
        _ => String::new(),
    }
}

fn compile_begin() -> Result<Program> {
    let mut prog = Program::new();
    prog.emit(Insn {
        opcode: Opcode::Transaction,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    Ok(prog)
}

fn compile_commit() -> Result<Program> {
    let mut prog = Program::new();
    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    Ok(prog)
}

fn compile_rollback() -> Result<Program> {
    let mut prog = Program::new();
    prog.emit(Insn {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    Ok(prog)
}

fn compile_expr_with_columns(
    prog: &mut Program,
    expr: &Expr,
    table: Option<&crate::schema::Table>,
    cursor: i32,
    dest_reg: i32,
) -> Result<()> {
    if let Some(table) = table {
        if let Expr::Ident(name) = expr {
            if let Some(idx) = table.column_index(name) {
                prog.emit(Insn {
                    opcode: Opcode::Column,
                    p1: dest_reg,
                    p2: cursor,
                    p3: idx as i32,
                    p4: InsnP4::None,
                    p5: 0,
                });
                return Ok(());
            }
        }
    }
    compile_expr(prog, expr, dest_reg)
}

fn compile_where_expr(
    prog: &mut Program,
    expr: &Expr,
    table: &crate::schema::Table,
    cursor: i32,
    dest_reg: i32,
) -> Result<()> {
    match expr {
        Expr::BinaryOp { op, left, right } => {
            compile_expr_with_columns(prog, left, Some(table), cursor, dest_reg + 1)?;
            compile_expr(prog, right, dest_reg + 2)?;
            let opcode = match op {
                BinaryOp::Eq => Opcode::Eq,
                BinaryOp::NotEq => Opcode::Ne,
                BinaryOp::Lt => Opcode::Lt,
                BinaryOp::Le => Opcode::Le,
                BinaryOp::Gt => Opcode::Gt,
                BinaryOp::Ge => Opcode::Ge,
                _ => Opcode::Eq,
            };
            prog.emit(Insn {
                opcode,
                p1: dest_reg,
                p2: dest_reg + 1,
                p3: dest_reg + 2,
                p4: InsnP4::None,
                p5: 0,
            });
        }
        _ => compile_expr_with_columns(prog, expr, Some(table), cursor, dest_reg)?,
    }
    Ok(())
}

pub(crate) fn compile_expr(prog: &mut Program, expr: &Expr, reg: i32) -> Result<()> {
    match expr {
        Expr::Null => {
            prog.emit(Insn {
                opcode: Opcode::Null,
                p1: reg,
                p2: 0,
                p3: 0,
                p4: InsnP4::None,
                p5: 0,
            });
        }
        Expr::Integer(n) => {
            prog.emit(Insn {
                opcode: Opcode::Integer,
                p1: reg,
                p2: *n as i32,
                p3: 0,
                p4: InsnP4::None,
                p5: 0,
            });
        }
        Expr::Real(r) => {
            prog.emit(Insn {
                opcode: Opcode::Real,
                p1: reg,
                p2: 0,
                p3: 0,
                p4: InsnP4::Real(*r),
                p5: 0,
            });
        }
        Expr::String(s) => {
            prog.emit(Insn {
                opcode: Opcode::String,
                p1: reg,
                p2: 0,
                p3: 0,
                p4: InsnP4::String(s.clone()),
                p5: 0,
            });
        }
        Expr::BinaryOp { op, left, right } => {
            compile_expr(prog, left, reg + 1)?;
            compile_expr(prog, right, reg + 2)?;
            let opcode = match op {
                BinaryOp::Add => Opcode::Add,
                BinaryOp::Sub => Opcode::Subtract,
                BinaryOp::Mul => Opcode::Multiply,
                BinaryOp::Div => Opcode::Divide,
                BinaryOp::Mod => Opcode::Remainder,
                BinaryOp::Concat => Opcode::Concat,
                BinaryOp::And => Opcode::And,
                BinaryOp::Or => Opcode::Or,
                BinaryOp::Eq => Opcode::Eq,
                BinaryOp::NotEq => Opcode::Ne,
                BinaryOp::Lt => Opcode::Lt,
                BinaryOp::Le => Opcode::Le,
                BinaryOp::Gt => Opcode::Gt,
                BinaryOp::Ge => Opcode::Ge,
                _ => Opcode::Eq,
            };
            prog.emit(Insn {
                opcode,
                p1: reg,
                p2: reg + 1,
                p3: reg + 2,
                p4: InsnP4::None,
                p5: 0,
            });
        }
        Expr::UnaryOp { op, expr } => {
            compile_expr(prog, expr, reg + 1)?;
            let opcode = match op {
                UnaryOp::Not => Opcode::Not,
                UnaryOp::Minus => Opcode::Subtract,
                _ => Opcode::Not,
            };
            prog.emit(Insn {
                opcode,
                p1: reg,
                p2: reg + 1,
                p3: 0,
                p4: InsnP4::None,
                p5: 0,
            });
        }
        _ => {
            prog.emit(Insn {
                opcode: Opcode::Null,
                p1: reg,
                p2: 0,
                p3: 0,
                p4: InsnP4::None,
                p5: 0,
            });
        }
    }
    Ok(())
}
