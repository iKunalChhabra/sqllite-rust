//! VDBE execution engine.

use crate::error::{Result, ResultCode, SqlliteError};
use crate::pragma::{execute_pragma, PragmaRow};
use crate::schema::{Schema, Table};
use crate::storage::btree::{btree_insert_row, Btree, BtreeCursor, BtreeFlags};
use crate::storage::pager::Pager;
use crate::types::Value;
use crate::vdbe::program::{AggFunc, GroupBySpec, Insn, InsnP4, Opcode, Program, SortKey};
use std::collections::HashMap;
use std::sync::Arc;

/// Execution state for a prepared statement.
pub struct Vdbe {
    program: Program,
    pc: usize,
    regs: Vec<Value>,
    cursors: HashMap<i32, CursorState>,
    pager: Arc<Pager>,
    schema: Arc<parking_lot::RwLock<Schema>>,
    halted: bool,
    halt_code: ResultCode,
    halt_message: String,
    result_columns: Vec<usize>,
    changes: i64,
    last_rowid: i64,
    pragma_pending: Vec<PragmaRow>,
    pragma_row_idx: usize,
    row_buffer: Vec<Vec<Value>>,
    buffer_idx: usize,
}

struct CursorState {
    btree: Arc<Btree>,
    cursor: BtreeCursor,
    table: String,
}

impl Vdbe {
    pub fn new(
        program: Program,
        pager: Arc<Pager>,
        schema: Arc<parking_lot::RwLock<Schema>>,
    ) -> Self {
        let n_reg = program.n_reg.max(8);
        Self {
            program,
            pc: 0,
            regs: vec![Value::Null; n_reg],
            cursors: HashMap::new(),
            pager,
            schema,
            halted: false,
            halt_code: ResultCode::Ok,
            halt_message: String::new(),
            result_columns: Vec::new(),
            changes: 0,
            last_rowid: 0,
            pragma_pending: Vec::new(),
            pragma_row_idx: 0,
            row_buffer: Vec::new(),
            buffer_idx: 0,
        }
    }

    pub fn changes(&self) -> i64 {
        self.changes
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.last_rowid
    }

    pub fn step(&mut self) -> Result<ResultCode> {
        if self.halted {
            return Ok(self.halt_code);
        }

        if self.pragma_row_idx < self.pragma_pending.len() {
            self.emit_pragma_row(self.pragma_row_idx);
            self.pragma_row_idx += 1;
            return Ok(ResultCode::Row);
        }

        while self.pc < self.program.insns.len() {
            let insn = self.program.insns[self.pc].clone();
            self.pc += 1;
            match self.execute(&insn)? {
                StepResult::Continue => {}
                StepResult::Row => return Ok(ResultCode::Row),
                StepResult::Done => return Ok(ResultCode::Done),
                StepResult::Halt(code, msg) => {
                    self.halted = true;
                    self.halt_code = code;
                    self.halt_message = msg;
                    if code.is_ok() {
                        return Ok(ResultCode::Done);
                    }
                    return Err(SqlliteError::sql(code, self.halt_message.clone()));
                }
            }
        }
        Ok(ResultCode::Done)
    }

    pub fn column_count(&self) -> usize {
        self.result_columns.len()
    }

    pub fn column_value(&self, idx: usize) -> Option<&Value> {
        self.result_columns.get(idx).and_then(|&r| self.regs.get(r))
    }

    pub fn column_name(&self, _idx: usize) -> &str {
        ""
    }

    pub fn reset(&mut self) {
        self.pc = 0;
        self.halted = false;
        self.halt_code = ResultCode::Ok;
        for r in &mut self.regs {
            *r = Value::Null;
        }
        self.cursors.clear();
        self.pragma_pending.clear();
        self.pragma_row_idx = 0;
        self.row_buffer.clear();
        self.buffer_idx = 0;
    }

    fn emit_pragma_row(&mut self, idx: usize) {
        let row = self.pragma_pending[idx].clone();
        for (i, val) in row.iter().enumerate() {
            self.set_reg(i as i32, val.clone());
        }
        self.result_columns.clear();
        for i in 0..row.len() {
            self.result_columns.push(i);
        }
    }

    fn execute(&mut self, insn: &Insn) -> Result<StepResult> {
        match insn.opcode {
            Opcode::Init => Ok(StepResult::Continue),
            Opcode::Halt => {
                let code = match insn.p1 {
                    0 => ResultCode::Ok,
                    c => ResultCode::try_from_i32(c).unwrap_or(ResultCode::Ok),
                };
                let msg = match &insn.p4 {
                    InsnP4::String(s) => s.clone(),
                    _ => String::new(),
                };
                Ok(StepResult::Halt(code, msg))
            }
            Opcode::Goto => {
                self.pc = insn.p2 as usize;
                Ok(StepResult::Continue)
            }
            Opcode::Integer => {
                self.set_reg(insn.p1, Value::Integer(insn.p2 as i64));
                Ok(StepResult::Continue)
            }
            Opcode::Int64 => {
                if let InsnP4::Int64(v) = insn.p4 {
                    self.set_reg(insn.p1, Value::Integer(v));
                }
                Ok(StepResult::Continue)
            }
            Opcode::Real => {
                if let InsnP4::Real(v) = insn.p4 {
                    self.set_reg(insn.p1, Value::Real(v));
                }
                Ok(StepResult::Continue)
            }
            Opcode::String => {
                if let InsnP4::String(s) = &insn.p4 {
                    self.set_reg(insn.p1, Value::Text(s.clone()));
                }
                Ok(StepResult::Continue)
            }
            Opcode::Null => {
                self.set_reg(insn.p1, Value::Null);
                Ok(StepResult::Continue)
            }
            Opcode::Blob => {
                Ok(StepResult::Continue)
            }
            Opcode::Move => {
                let val = self.get_reg(insn.p2).clone();
                self.set_reg(insn.p1, val);
                Ok(StepResult::Continue)
            }
            Opcode::Copy => {
                let val = self.get_reg(insn.p2).clone();
                self.set_reg(insn.p1, val);
                Ok(StepResult::Continue)
            }
            Opcode::ResultRow => {
                self.result_columns.clear();
                for i in 0..insn.p1 {
                    self.result_columns.push(i as usize);
                }
                Ok(StepResult::Row)
            }
            Opcode::Transaction => {
                self.pager.begin()?;
                Ok(StepResult::Continue)
            }
            Opcode::OpenRead | Opcode::OpenWrite => {
                let table_name = match &insn.p4 {
                    InsnP4::String(s) => s.clone(),
                    _ => return Err(SqlliteError::sql(ResultCode::Internal, "missing table name")),
                };
                let schema = self.schema.read();
                let table = schema.table(&table_name).ok_or_else(|| {
                    SqlliteError::sql(ResultCode::Error, format!("no such table: {table_name}"))
                })?;
                let btree = Arc::new(Btree::new(
                    self.pager.clone(),
                    table.root_page,
                    BtreeFlags {
                        intkey: true,
                        blobkey: false,
                    },
                ));
                let cursor = btree.cursor();
                self.cursors.insert(
                    insn.p1,
                    CursorState {
                        btree,
                        cursor,
                        table: table_name,
                    },
                );
                Ok(StepResult::Continue)
            }
            Opcode::Rewind => {
                if let Some(cs) = self.cursors.get_mut(&insn.p1) {
                    let found = cs.cursor.first()?;
                    if !found {
                        self.pc = insn.p2 as usize;
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::Last | Opcode::SorterNext => {
                if let Some(cs) = self.cursors.get_mut(&insn.p1) {
                    let found = cs.cursor.next()?;
                    if found {
                        self.pc = insn.p2 as usize;
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::Column => {
                if let Some(cs) = self.cursors.get(&insn.p2) {
                    let values = cs.cursor.values()?;
                    if let Some(val) = values.get(insn.p3 as usize) {
                        self.set_reg(insn.p1, val.clone());
                    } else {
                        self.set_reg(insn.p1, Value::Null);
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::Rowid => {
                if let Some(cs) = self.cursors.get(&insn.p2) {
                    if let Some(id) = cs.cursor.key() {
                        self.set_reg(insn.p1, Value::Integer(id));
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::MakeRecord => {
                let mut values = Vec::new();
                for i in 0..insn.p1 {
                    values.push(self.get_reg(insn.p2 + i).clone());
                }
                let record = crate::record::encode_record(&values);
                self.set_reg(insn.p3, Value::Blob(record));
                Ok(StepResult::Continue)
            }
            Opcode::NewRowid => {
                let rowid = self.allocate_rowid(insn.p2)?;
                self.set_reg(insn.p1, Value::Integer(rowid));
                self.last_rowid = rowid;
                Ok(StepResult::Continue)
            }
            Opcode::Insert | Opcode::InsertInt => {
                if let Some(cs) = self.cursors.get(&insn.p2) {
                    let rowid = if insn.opcode == Opcode::InsertInt {
                        insn.p3 as i64
                    } else {
                        match self.get_reg(insn.p3) {
                            Value::Integer(i) => *i,
                            _ => return Err(SqlliteError::sql(ResultCode::Mismatch, "rowid must be integer")),
                        }
                    };
                    let record = match self.get_reg(insn.p1) {
                        Value::Blob(b) => b.clone(),
                        _ => return Err(SqlliteError::sql(ResultCode::Internal, "expected record blob")),
                    };
                    if insn.p5 != 0 {
                        cs.btree.replace(rowid, &record)?;
                    } else {
                        cs.btree.insert(rowid, &record)?;
                    }
                    let table_name = cs.table.clone();
                    let new_root = cs.btree.root_page();
                    drop(cs);
                    self.schema
                        .write()
                        .set_table_root_page(&table_name, new_root)?;
                    self.changes += 1;
                    self.last_rowid = rowid;
                }
                Ok(StepResult::Continue)
            }
            Opcode::Delete => {
                if let Some(cs) = self.cursors.get(&insn.p2) {
                    if let Some(rowid) = cs.cursor.key() {
                        cs.btree.delete(rowid)?;
                        self.changes += 1;
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::Eq | Opcode::Ne | Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge => {
                let left = self.get_reg(insn.p2);
                let right = self.get_reg(insn.p3);
                let cmp = left.compare(right);
                let cond = match insn.opcode {
                    Opcode::Eq => cmp == std::cmp::Ordering::Equal,
                    Opcode::Ne => cmp != std::cmp::Ordering::Equal,
                    Opcode::Lt => cmp == std::cmp::Ordering::Less,
                    Opcode::Le => cmp != std::cmp::Ordering::Less,
                    Opcode::Gt => cmp == std::cmp::Ordering::Greater,
                    Opcode::Ge => cmp != std::cmp::Ordering::Greater,
                    _ => false,
                };
                self.set_reg(insn.p1, Value::Integer(cond as i64));
                Ok(StepResult::Continue)
            }
            Opcode::If => {
                let val = self.get_reg(insn.p1);
                if !val.is_null() && val.as_integer().unwrap_or(0) != 0 {
                    self.pc = insn.p2 as usize;
                }
                Ok(StepResult::Continue)
            }
            Opcode::IfNot => {
                let val = self.get_reg(insn.p1);
                if val.is_null() || val.as_integer().unwrap_or(0) == 0 {
                    self.pc = insn.p2 as usize;
                }
                Ok(StepResult::Continue)
            }
            Opcode::IsNull => {
                if self.get_reg(insn.p1).is_null() {
                    self.pc = insn.p2 as usize;
                }
                Ok(StepResult::Continue)
            }
            Opcode::NotNull => {
                if !self.get_reg(insn.p1).is_null() {
                    self.pc = insn.p2 as usize;
                }
                Ok(StepResult::Continue)
            }
            Opcode::Add => self.binary_numeric(insn, |a, b| Value::Real(a + b)),
            Opcode::Subtract => self.binary_numeric(insn, |a, b| Value::Real(a - b)),
            Opcode::Multiply => self.binary_numeric(insn, |a, b| Value::Real(a * b)),
            Opcode::Divide => self.binary_numeric(insn, |a, b| Value::Real(a / b)),
            Opcode::Remainder => self.binary_int(insn, |a, b| Value::Integer(a % b)),
            Opcode::Concat => {
                let left = self.get_reg(insn.p2).to_text();
                let right = self.get_reg(insn.p3).to_text();
                self.set_reg(insn.p1, Value::Text(format!("{left}{right}")));
                Ok(StepResult::Continue)
            }
            Opcode::And => {
                let l = self.get_reg(insn.p2);
                let r = self.get_reg(insn.p3);
                let result = !l.is_null()
                    && !r.is_null()
                    && l.as_integer().unwrap_or(0) != 0
                    && r.as_integer().unwrap_or(0) != 0;
                self.set_reg(insn.p1, Value::Integer(result as i64));
                Ok(StepResult::Continue)
            }
            Opcode::Or => {
                let l = self.get_reg(insn.p2);
                let r = self.get_reg(insn.p3);
                let result = !l.is_null()
                    && !r.is_null()
                    && (l.as_integer().unwrap_or(0) != 0 || r.as_integer().unwrap_or(0) != 0);
                self.set_reg(insn.p1, Value::Integer(result as i64));
                Ok(StepResult::Continue)
            }
            Opcode::Not => {
                let v = self.get_reg(insn.p2);
                let result = v.is_null() || v.as_integer().unwrap_or(0) == 0;
                self.set_reg(insn.p1, Value::Integer(result as i64));
                Ok(StepResult::Continue)
            }
            Opcode::SeekRowid => {
                let rowid = match self.get_reg(insn.p3) {
                    Value::Integer(i) => *i,
                    _ => 0,
                };
                if let Some(cs) = self.cursors.get_mut(&insn.p1) {
                    let found = cs.cursor.seek(rowid)?;
                    if !found {
                        self.pc = insn.p2 as usize;
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::NotFound => {
                if let Some(cs) = self.cursors.get(&insn.p1) {
                    if cs.cursor.is_eof() {
                        self.pc = insn.p2 as usize;
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::Found => {
                if let Some(cs) = self.cursors.get(&insn.p1) {
                    if !cs.cursor.is_eof() {
                        self.pc = insn.p2 as usize;
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::Closer => Ok(StepResult::Continue),
            Opcode::CreateBtree => Ok(StepResult::Continue),
            Opcode::Destroy => Ok(StepResult::Continue),
            Opcode::DropTable => Ok(StepResult::Continue),
            Opcode::SetCookie => Ok(StepResult::Continue),
            Opcode::Count => {
                if let Some(cs) = self.cursors.get(&insn.p2) {
                    let mut count = 0i64;
                    let mut cur = cs.btree.cursor();
                    if cur.first()? {
                        loop {
                            count += 1;
                            if !cur.next()? {
                                break;
                            }
                        }
                    }
                    self.set_reg(insn.p1, Value::Integer(count));
                }
                Ok(StepResult::Continue)
            }
            Opcode::Pragma => {
                if let InsnP4::Pragma { name, value } = &insn.p4 {
                    let rows = execute_pragma(
                        name,
                        value.as_deref(),
                        &self.pager,
                        &self.schema.read(),
                    )?;
                    self.pragma_pending = rows;
                    self.pragma_row_idx = 0;
                    if !self.pragma_pending.is_empty() {
                        self.emit_pragma_row(0);
                        self.pragma_row_idx = 1;
                        return Ok(StepResult::Row);
                    }
                }
                Ok(StepResult::Continue)
            }
            Opcode::RowData => {
                let n = insn.p1 as usize;
                let mut row = Vec::with_capacity(n);
                for i in 0..n {
                    row.push(self.get_reg(i as i32).clone());
                }
                self.row_buffer.push(row);
                Ok(StepResult::Continue)
            }
            Opcode::GroupBy => {
                if let InsnP4::GroupBy(spec) = &insn.p4 {
                    self.apply_group_by(spec);
                }
                Ok(StepResult::Continue)
            }
            Opcode::Sort => {
                if let InsnP4::SortKeys(keys) = &insn.p4 {
                    self.sort_buffer(keys);
                }
                Ok(StepResult::Continue)
            }
            Opcode::BufferRewind => {
                self.buffer_idx = 0;
                if self.row_buffer.is_empty() {
                    self.pc = insn.p2 as usize;
                }
                Ok(StepResult::Continue)
            }
            Opcode::BufferNext => {
                if self.buffer_idx >= self.row_buffer.len() {
                    self.pc = insn.p2 as usize;
                } else {
                    let row = self.row_buffer[self.buffer_idx].clone();
                    for (i, val) in row.into_iter().enumerate() {
                        self.set_reg(i as i32, val);
                    }
                    self.buffer_idx += 1;
                }
                Ok(StepResult::Continue)
            }
            _ => Ok(StepResult::Continue),
        }
    }

    fn set_reg(&mut self, idx: i32, val: Value) {
        let idx = idx as usize;
        if idx >= self.regs.len() {
            self.regs.resize(idx + 1, Value::Null);
        }
        self.regs[idx] = val;
    }

    fn get_reg(&self, idx: i32) -> &Value {
        self.regs.get(idx as usize).unwrap_or(&Value::Null)
    }

    fn binary_numeric<F>(&mut self, insn: &Insn, f: F) -> Result<StepResult>
    where
        F: Fn(f64, f64) -> Value,
    {
        let a = self.get_reg(insn.p2).as_real().unwrap_or(0.0);
        let b = self.get_reg(insn.p3).as_real().unwrap_or(0.0);
        self.set_reg(insn.p1, f(a, b));
        Ok(StepResult::Continue)
    }

    fn binary_int<F>(&mut self, insn: &Insn, f: F) -> Result<StepResult>
    where
        F: Fn(i64, i64) -> Value,
    {
        let a = self.get_reg(insn.p2).as_integer().unwrap_or(0);
        let b = self.get_reg(insn.p3).as_integer().unwrap_or(0);
        self.set_reg(insn.p1, f(a, b));
        Ok(StepResult::Continue)
    }

    fn allocate_rowid(&self, cursor_id: i32) -> Result<i64> {
        if let Some(cs) = self.cursors.get(&cursor_id) {
            let mut max_id = 0i64;
            let mut cur = cs.btree.cursor();
            if cur.first()? {
                loop {
                    if let Some(id) = cur.key() {
                        max_id = max_id.max(id);
                    }
                    if !cur.next()? {
                        break;
                    }
                }
            }
            return Ok(max_id + 1);
        }
        Ok(1)
    }

    fn sort_buffer(&mut self, keys: &[SortKey]) {
        self.row_buffer.sort_by(|a, b| {
            for key in keys {
                let av = a.get(key.col_index).unwrap_or(&Value::Null);
                let bv = b.get(key.col_index).unwrap_or(&Value::Null);
                let ord = av.compare(bv);
                let ord = if key.desc { ord.reverse() } else { ord };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    fn apply_group_by(&mut self, spec: &GroupBySpec) {
        struct AggState {
            count: i64,
            sum: Option<f64>,
            min: Option<Value>,
            max: Option<Value>,
        }

        impl AggState {
            fn new() -> Self {
                Self {
                    count: 0,
                    sum: None,
                    min: None,
                    max: None,
                }
            }

            fn step(&mut self, func: AggFunc, val: Option<&Value>) {
                self.count += 1;
                match func {
                    AggFunc::Count => {}
                    AggFunc::Sum => {
                        if let Some(v) = val {
                            if !v.is_null() {
                                *self.sum.get_or_insert(0.0) += v.as_real().unwrap_or(0.0);
                            }
                        }
                    }
                    AggFunc::Min => {
                        if let Some(v) = val {
                            if !v.is_null() {
                                self.min = Some(match &self.min {
                                    Some(cur) if v.compare(cur) == std::cmp::Ordering::Less => {
                                        v.clone()
                                    }
                                    Some(cur) => cur.clone(),
                                    None => v.clone(),
                                });
                            }
                        }
                    }
                    AggFunc::Max => {
                        if let Some(v) = val {
                            if !v.is_null() {
                                self.max = Some(match &self.max {
                                    Some(cur) if v.compare(cur) == std::cmp::Ordering::Greater => {
                                        v.clone()
                                    }
                                    Some(cur) => cur.clone(),
                                    None => v.clone(),
                                });
                            }
                        }
                    }
                }
            }

            fn finalize(&self, func: AggFunc) -> Value {
                match func {
                    AggFunc::Count => Value::Integer(self.count),
                    AggFunc::Sum => Value::Real(self.sum.unwrap_or(0.0)),
                    AggFunc::Min => self.min.clone().unwrap_or(Value::Null),
                    AggFunc::Max => self.max.clone().unwrap_or(Value::Null),
                }
            }
        }

        let mut groups: HashMap<String, (Vec<Value>, Vec<AggState>)> = HashMap::new();
        let rows = std::mem::take(&mut self.row_buffer);

        for row in rows {
            let key_vals: Vec<Value> = spec
                .key_indices
                .iter()
                .map(|&i| row.get(i).cloned().unwrap_or(Value::Null))
                .collect();
            let key_str: String = key_vals
                .iter()
                .map(|v| v.to_text())
                .collect::<Vec<_>>()
                .join("\x1f");

            let entry = groups
                .entry(key_str)
                .or_insert_with(|| (key_vals.clone(), spec.aggs.iter().map(|_| AggState::new()).collect()));

            for (agg, state) in spec.aggs.iter().zip(entry.1.iter_mut()) {
                let val = agg.col_index.and_then(|i| row.get(i));
                state.step(agg.func, val);
            }
        }

        let mut out = Vec::new();
        for (_, (key, states)) in groups {
            let mut row = key;
            for (agg, state) in spec.aggs.iter().zip(states.iter()) {
                row.push(state.finalize(agg.func));
            }
            out.push(row);
        }
        self.row_buffer = out;
        self.buffer_idx = 0;
    }
}

enum StepResult {
    Continue,
    Row,
    Done,
    Halt(ResultCode, String),
}

impl ResultCode {
    fn try_from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(ResultCode::Ok),
            1 => Some(ResultCode::Error),
            19 => Some(ResultCode::Constraint),
            101 => Some(ResultCode::Done),
            _ => Some(ResultCode::Error),
        }
    }
}
