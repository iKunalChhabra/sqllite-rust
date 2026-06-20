//! VDBE virtual machine opcodes and execution.

mod exec;
pub mod program;

pub use exec::Vdbe;
pub use program::{AggFunc, AggSpec, GroupBySpec, Insn, InsnP4, Opcode, Program, SortKey};
