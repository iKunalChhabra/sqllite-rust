//! SELECT statement compilation (JOIN, GROUP BY, ORDER BY, aggregates).

use crate::error::{Result, ResultCode, SqlliteError};
use crate::schema::Schema;
use crate::vdbe::program::{AggFunc, AggSpec, GroupBySpec, Insn, InsnP4, Opcode, Program, SortKey};
use sqllite_parser::ast::*;

struct TableBinding {
    name: String,
    alias: String,
    cursor: i32,
}

struct JoinInfo {
    join_type: JoinType,
    on: Option<Expr>,
}

struct ColumnRef {
    table_idx: usize,
    col_idx: usize,
}

struct SelectPlan {
    row_sources: Vec<ColumnRef>,
    row_width: usize,
    output_width: usize,
    has_aggregates: bool,
    group_spec: Option<GroupBySpec>,
    sort_keys: Vec<SortKey>,
}

pub fn compile_select(select: &Select, schema: &Schema) -> Result<Program> {
    let from = select.from.as_ref().ok_or_else(|| {
        SqlliteError::sql(ResultCode::Error, "SELECT requires a FROM clause")
    })?;

    if let Some(cursor) = simple_count_star(select, from) {
        return compile_simple_count(cursor, schema, from);
    }

    let mut tables = Vec::new();
    let mut join = None;
    collect_from(from, schema, &mut tables, &mut join, 0)?;

    let has_join = tables.len() > 1;
    if has_join {
        let info = join.as_ref().ok_or_else(|| {
            SqlliteError::sql(ResultCode::Error, "JOIN requires an ON clause")
        })?;
        if info.join_type != JoinType::Inner {
            return Err(SqlliteError::sql(
                ResultCode::Error,
                "only INNER JOIN is supported",
            ));
        }
        if info.on.is_none() {
            return Err(SqlliteError::sql(ResultCode::Error, "JOIN requires an ON clause"));
        }
    }

    let plan = build_select_plan(select, schema, &tables)?;
    let needs_buffer =
        !select.group_by.is_empty() || !select.order_by.is_empty() || plan.has_aggregates;

    let mut prog = Program::new();
    prog.read_only = true;
    prog.n_reg = 32;

    prog.emit(Insn {
        opcode: Opcode::Init,
        p1: 0,
        p2: 1,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    for table in &tables {
        prog.emit(Insn {
            opcode: Opcode::OpenRead,
            p1: table.cursor,
            p2: 0,
            p3: 0,
            p4: InsnP4::String(table.name.clone()),
            p5: 0,
        });
    }

    let outer_cursor = tables[0].cursor;
    let outer_rewind = prog.emit(Insn {
        opcode: Opcode::Rewind,
        p1: outer_cursor,
        p2: 0,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });

    if has_join {
        let inner_cursor = tables[1].cursor;
        let inner_rewind = prog.emit(Insn {
            opcode: Opcode::Rewind,
            p1: inner_cursor,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });

        compile_pred(
            &mut prog,
            join.as_ref().unwrap().on.as_ref().unwrap(),
            schema,
            &tables,
            1,
        )?;
        let on_skip = prog.emit(Insn {
            opcode: Opcode::IfNot,
            p1: 1,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });

        let where_skip = compile_optional_where(&mut prog, select, schema, &tables, 1)?;
        compile_row_load(&mut prog, &plan, &tables)?;
        emit_scan_result(&mut prog, &plan, needs_buffer);

        let inner_next = prog.insns.len() as i32;
        prog.patch_p2(on_skip, inner_next);
        if let Some(addr) = where_skip {
            prog.patch_p2(addr, inner_next);
        }

        let inner_loop = (inner_rewind + 1) as i32;
        prog.emit(Insn {
            opcode: Opcode::Last,
            p1: inner_cursor,
            p2: inner_loop,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });

        let outer_next = prog.insns.len() as i32;
        prog.patch_p2(inner_rewind, outer_next);

        let outer_loop = (outer_rewind + 1) as i32;
        prog.emit(Insn {
            opcode: Opcode::Last,
            p1: outer_cursor,
            p2: outer_loop,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
    } else {
        let where_skip = compile_optional_where(&mut prog, select, schema, &tables, 1)?;
        compile_row_load(&mut prog, &plan, &tables)?;
        emit_scan_result(&mut prog, &plan, needs_buffer);

        let loop_next = prog.insns.len() as i32;
        if let Some(addr) = where_skip {
            prog.patch_p2(addr, loop_next);
        }

        let loop_top = (outer_rewind + 1) as i32;
        prog.emit(Insn {
            opcode: Opcode::Last,
            p1: outer_cursor,
            p2: loop_top,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
    }

    let post_scan = prog.insns.len();
    prog.patch_p2(outer_rewind, post_scan as i32);

    if plan.has_aggregates {
        if let Some(spec) = plan.group_spec.clone() {
            prog.emit(Insn {
                opcode: Opcode::GroupBy,
                p1: plan.row_width as i32,
                p2: 0,
                p3: 0,
                p4: InsnP4::GroupBy(spec),
                p5: 0,
            });
        }
    }

    if !select.order_by.is_empty() {
        prog.emit(Insn {
            opcode: Opcode::Sort,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: InsnP4::SortKeys(plan.sort_keys.clone()),
            p5: 0,
        });
    }

    if needs_buffer {
        let rewind_addr = prog.emit(Insn {
            opcode: Opcode::BufferRewind,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
        let emit_row = prog.insns.len();
        let next_addr = prog.emit(Insn {
            opcode: Opcode::BufferNext,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
        prog.emit(Insn {
            opcode: Opcode::ResultRow,
            p1: plan.output_width as i32,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
        prog.emit(Insn {
            opcode: Opcode::Goto,
            p1: 0,
            p2: emit_row as i32,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
        let halt_addr = prog.insns.len();
        prog.patch_p2(rewind_addr, halt_addr as i32);
        prog.patch_p2(next_addr, halt_addr as i32);
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

fn emit_scan_result(prog: &mut Program, plan: &SelectPlan, needs_buffer: bool) {
    if needs_buffer {
        prog.emit(Insn {
            opcode: Opcode::RowData,
            p1: plan.row_width as i32,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
    } else {
        prog.emit(Insn {
            opcode: Opcode::ResultRow,
            p1: plan.output_width as i32,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
    }
}

fn simple_count_star(select: &Select, from: &TableRef) -> Option<i32> {
    if !select.group_by.is_empty() || !select.order_by.is_empty() {
        return None;
    }
    if !matches!(from, TableRef::Table { .. }) {
        return None;
    }
    if select.columns.len() != 1 {
        return None;
    }
    if let Expr::Function { name, args, .. } = &select.columns[0].expr {
        if name.eq_ignore_ascii_case("count")
            && args.len() == 1
            && matches!(args[0], Expr::Star)
        {
            return Some(0);
        }
    }
    None
}

fn compile_simple_count(cursor: i32, schema: &Schema, from: &TableRef) -> Result<Program> {
    let TableRef::Table { name, .. } = from else {
        unreachable!();
    };
    if schema.table(name).is_none() {
        return Err(SqlliteError::sql(
            ResultCode::Error,
            format!("no such table: {name}"),
        ));
    }

    let mut prog = Program::new();
    prog.read_only = true;
    prog.n_reg = 8;
    prog.emit(Insn {
        opcode: Opcode::Init,
        p1: 0,
        p2: 1,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::OpenRead,
        p1: cursor,
        p2: 0,
        p3: 0,
        p4: InsnP4::String(name.clone()),
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::Count,
        p1: 0,
        p2: cursor,
        p3: 0,
        p4: InsnP4::None,
        p5: 0,
    });
    prog.emit(Insn {
        opcode: Opcode::ResultRow,
        p1: 1,
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

fn collect_from(
    from: &TableRef,
    schema: &Schema,
    tables: &mut Vec<TableBinding>,
    join: &mut Option<JoinInfo>,
    next_cursor: i32,
) -> Result<i32> {
    match from {
        TableRef::Table { name, alias } => {
            if schema.table(name).is_none() {
                return Err(SqlliteError::sql(
                    ResultCode::Error,
                    format!("no such table: {name}"),
                ));
            }
            tables.push(TableBinding {
                name: name.clone(),
                alias: alias.clone().unwrap_or_else(|| name.clone()),
                cursor: next_cursor,
            });
            Ok(next_cursor + 1)
        }
        TableRef::Join {
            left,
            right,
            join_type,
            on,
        } => {
            let c = collect_from(left, schema, tables, join, next_cursor)?;
            let c = collect_from(right, schema, tables, join, c)?;
            *join = Some(JoinInfo {
                join_type: *join_type,
                on: on.clone(),
            });
            Ok(c)
        }
        TableRef::Subquery { .. } => Err(SqlliteError::sql(
            ResultCode::Error,
            "subqueries in FROM are not supported",
        )),
    }
}

fn build_select_plan(
    select: &Select,
    schema: &Schema,
    tables: &[TableBinding],
) -> Result<SelectPlan> {
    let has_aggregates = select
        .columns
        .iter()
        .any(|c| matches!(c.expr, Expr::Function { .. }));

    let mut row_sources = Vec::new();
    let mut key_indices = Vec::new();
    let mut aggs = Vec::new();

    if has_aggregates {
        for expr in &select.group_by {
            let col = resolve_expr_column(expr, schema, tables)?;
            key_indices.push(push_unique_column(&mut row_sources, col));
        }

        for col in &select.columns {
            if let Expr::Function { name, args, .. } = &col.expr {
                let func = parse_agg_func(name)?;
                let col_index = match func {
                    AggFunc::Count if args.len() == 1 && matches!(args[0], Expr::Star) => None,
                    AggFunc::Count => {
                        let cref = resolve_expr_column(&args[0], schema, tables)?;
                        Some(push_unique_column(&mut row_sources, cref))
                    }
                    _ => {
                        if args.is_empty() {
                            return Err(SqlliteError::sql(
                                ResultCode::Error,
                                format!("{name} requires an argument"),
                            ));
                        }
                        let cref = resolve_expr_column(&args[0], schema, tables)?;
                        Some(push_unique_column(&mut row_sources, cref))
                    }
                };
                aggs.push(AggSpec { func, col_index });
            } else {
                let cref = resolve_expr_column(&col.expr, schema, tables)?;
                if !group_key_contains(&row_sources, &key_indices, &cref) {
                    return Err(SqlliteError::sql(
                        ResultCode::Error,
                        "non-aggregate column must appear in GROUP BY",
                    ));
                }
            }
        }
    } else {
        for col in &select.columns {
            match &col.expr {
                Expr::Star => {
                    if tables.len() != 1 {
                        return Err(SqlliteError::sql(
                            ResultCode::Error,
                            "* is not supported with JOIN",
                        ));
                    }
                    let table = schema.table(&tables[0].name).unwrap();
                    for i in 0..table.column_count() {
                        row_sources.push(ColumnRef {
                            table_idx: 0,
                            col_idx: i,
                        });
                    }
                }
                _ => {
                    let cref = resolve_result_column(&col.expr, schema, tables)?;
                    row_sources.push(cref);
                }
            }
        }
    }

    let row_width = row_sources.len();
    let output_width = if has_aggregates {
        select.columns.len()
    } else {
        row_width
    };

    let group_spec = if has_aggregates {
        Some(GroupBySpec {
            key_indices,
            aggs,
        })
    } else {
        None
    };

    let sort_keys = select
        .order_by
        .iter()
        .map(|term| {
            let col_index =
                resolve_order_index(&term.expr, schema, tables, select, has_aggregates)?;
            Ok(SortKey {
                col_index,
                desc: term.desc,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(SelectPlan {
        row_sources,
        row_width,
        output_width,
        has_aggregates,
        group_spec,
        sort_keys,
    })
}

fn group_key_contains(sources: &[ColumnRef], key_indices: &[usize], col: &ColumnRef) -> bool {
    key_indices.iter().any(|&idx| {
        sources
            .get(idx)
            .map(|k| k.table_idx == col.table_idx && k.col_idx == col.col_idx)
            .unwrap_or(false)
    })
}

fn push_unique_column(sources: &mut Vec<ColumnRef>, col: ColumnRef) -> usize {
    for (i, existing) in sources.iter().enumerate() {
        if existing.table_idx == col.table_idx && existing.col_idx == col.col_idx {
            return i;
        }
    }
    sources.push(col);
    sources.len() - 1
}

fn parse_agg_func(name: &str) -> Result<AggFunc> {
    match name.to_lowercase().as_str() {
        "count" => Ok(AggFunc::Count),
        "sum" => Ok(AggFunc::Sum),
        "min" => Ok(AggFunc::Min),
        "max" => Ok(AggFunc::Max),
        _ => Err(SqlliteError::sql(
            ResultCode::Error,
            format!("unsupported aggregate: {name}"),
        )),
    }
}

fn resolve_order_index(
    expr: &Expr,
    schema: &Schema,
    tables: &[TableBinding],
    select: &Select,
    has_aggregates: bool,
) -> Result<usize> {
    for (i, rc) in select.columns.iter().enumerate() {
        if expr_matches(&rc.expr, expr, schema, tables)? {
            return Ok(i);
        }
    }
    if has_aggregates {
        for (i, ge) in select.group_by.iter().enumerate() {
            if expr_matches(ge, expr, schema, tables)? {
                return Ok(i);
            }
        }
    }
    Err(SqlliteError::sql(
        ResultCode::Error,
        "unsupported ORDER BY expression",
    ))
}

fn expr_matches(
    a: &Expr,
    b: &Expr,
    schema: &Schema,
    tables: &[TableBinding],
) -> Result<bool> {
    match (a, b) {
        (Expr::Ident(n1), Expr::Ident(n2)) => Ok(n1.eq_ignore_ascii_case(n2)),
        (
            Expr::QualifiedIdent {
                table: t1,
                column: c1,
            },
            Expr::QualifiedIdent {
                table: t2,
                column: c2,
            },
        ) => Ok(t1.eq_ignore_ascii_case(t2) && c1.eq_ignore_ascii_case(c2)),
        _ => {
            let ca = resolve_expr_column(a, schema, tables).ok();
            let cb = resolve_expr_column(b, schema, tables).ok();
            Ok(match (ca, cb) {
                (Some(a), Some(b)) => a.table_idx == b.table_idx && a.col_idx == b.col_idx,
                _ => false,
            })
        }
    }
}

fn resolve_result_column(
    expr: &Expr,
    schema: &Schema,
    tables: &[TableBinding],
) -> Result<ColumnRef> {
    match expr {
        Expr::Star => Err(SqlliteError::sql(
            ResultCode::Error,
            "* is not supported with JOIN or GROUP BY",
        )),
        Expr::QualifiedStar(_) => Err(SqlliteError::sql(
            ResultCode::Error,
            "qualified * is not supported",
        )),
        _ => resolve_expr_column(expr, schema, tables),
    }
}

fn resolve_expr_column(
    expr: &Expr,
    schema: &Schema,
    tables: &[TableBinding],
) -> Result<ColumnRef> {
    match expr {
        Expr::Ident(name) => {
            let mut found = None;
            for (ti, tb) in tables.iter().enumerate() {
                if let Some(ci) = schema.table(&tb.name).and_then(|t| t.column_index(name)) {
                    if found.is_some() {
                        return Err(SqlliteError::sql(
                            ResultCode::Error,
                            format!("ambiguous column name: {name}"),
                        ));
                    }
                    found = Some(ColumnRef {
                        table_idx: ti,
                        col_idx: ci,
                    });
                }
            }
            found.ok_or_else(|| {
                SqlliteError::sql(ResultCode::Error, format!("no such column: {name}"))
            })
        }
        Expr::QualifiedIdent { table, column } => {
            let ti = find_table_index(tables, table)?;
            let ci = schema
                .table(&tables[ti].name)
                .and_then(|t| t.column_index(column))
                .ok_or_else(|| {
                    SqlliteError::sql(ResultCode::Error, format!("no such column: {column}"))
                })?;
            Ok(ColumnRef {
                table_idx: ti,
                col_idx: ci,
            })
        }
        _ => Err(SqlliteError::sql(
            ResultCode::Error,
            "expected column reference",
        )),
    }
}

fn find_table_index(tables: &[TableBinding], name: &str) -> Result<usize> {
    for (i, t) in tables.iter().enumerate() {
        if t.name.eq_ignore_ascii_case(name) || t.alias.eq_ignore_ascii_case(name) {
            return Ok(i);
        }
    }
    Err(SqlliteError::sql(
        ResultCode::Error,
        format!("no such table: {name}"),
    ))
}

fn compile_row_load(prog: &mut Program, plan: &SelectPlan, tables: &[TableBinding]) -> Result<()> {
    for (reg, col) in plan.row_sources.iter().enumerate() {
        prog.emit(Insn {
            opcode: Opcode::Column,
            p1: reg as i32,
            p2: tables[col.table_idx].cursor,
            p3: col.col_idx as i32,
            p4: InsnP4::None,
            p5: 0,
        });
    }
    Ok(())
}

fn compile_optional_where(
    prog: &mut Program,
    select: &Select,
    schema: &Schema,
    tables: &[TableBinding],
    dest_reg: i32,
) -> Result<Option<usize>> {
    if let Some(ref w) = select.where_clause {
        compile_pred(prog, w, schema, tables, dest_reg)?;
        let addr = prog.emit(Insn {
            opcode: Opcode::IfNot,
            p1: dest_reg,
            p2: 0,
            p3: 0,
            p4: InsnP4::None,
            p5: 0,
        });
        Ok(Some(addr))
    } else {
        Ok(None)
    }
}

fn compile_pred(
    prog: &mut Program,
    expr: &Expr,
    schema: &Schema,
    tables: &[TableBinding],
    dest_reg: i32,
) -> Result<()> {
    match expr {
        Expr::BinaryOp { op, left, right } => {
            compile_column_or_expr(prog, left, schema, tables, dest_reg + 1)?;
            compile_column_or_expr(prog, right, schema, tables, dest_reg + 2)?;
            let opcode = match op {
                BinaryOp::Eq => Opcode::Eq,
                BinaryOp::NotEq => Opcode::Ne,
                BinaryOp::Lt => Opcode::Lt,
                BinaryOp::Le => Opcode::Le,
                BinaryOp::Gt => Opcode::Gt,
                BinaryOp::Ge => Opcode::Ge,
                BinaryOp::And => Opcode::And,
                BinaryOp::Or => Opcode::Or,
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
        _ => compile_column_or_expr(prog, expr, schema, tables, dest_reg)?,
    }
    Ok(())
}

fn compile_column_or_expr(
    prog: &mut Program,
    expr: &Expr,
    schema: &Schema,
    tables: &[TableBinding],
    dest_reg: i32,
) -> Result<()> {
    if let Ok(col) = resolve_expr_column(expr, schema, tables) {
        prog.emit(Insn {
            opcode: Opcode::Column,
            p1: dest_reg,
            p2: tables[col.table_idx].cursor,
            p3: col.col_idx as i32,
            p4: InsnP4::None,
            p5: 0,
        });
    } else {
        super::compile_expr(prog, expr, dest_reg)?;
    }
    Ok(())
}
