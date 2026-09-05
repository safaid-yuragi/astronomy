//! The instruction set (§15 Instruction).
//!
//! Instructions are structured enums — never opcode strings. Control flow
//! lives in [`crate::Terminator`], not in instructions.

use crate::id::{ConstantId, FunctionId, TypeId, ValueId};
use crate::span::Span;

/// What an instruction does, together with its operands.
///
/// All binary forms (`Add`..`Ge`) name their operands `lhs`/`rhs`.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstructionKind {
    /// Materialize a constant from the module constant pool.
    Const(ConstantId),

    // ---- Arithmetic (wrapping two's-complement, §21) ----
    /// `lhs + rhs` (wrapping).
    Add { lhs: ValueId, rhs: ValueId },
    /// `lhs - rhs` (wrapping).
    Sub { lhs: ValueId, rhs: ValueId },
    /// `lhs * rhs` (wrapping).
    Mul { lhs: ValueId, rhs: ValueId },
    /// `lhs / rhs` (signed types: truncating; div-by-zero is undefined).
    Div { lhs: ValueId, rhs: ValueId },
    /// `lhs % rhs` (sign follows the dividend; rem-by-zero is undefined).
    Rem { lhs: ValueId, rhs: ValueId },

    // ---- Bitwise ----
    /// Bitwise AND.
    And { lhs: ValueId, rhs: ValueId },
    /// Bitwise OR.
    Or { lhs: ValueId, rhs: ValueId },
    /// Bitwise XOR.
    Xor { lhs: ValueId, rhs: ValueId },
    /// Shift left (`rhs` is an unsigned count; counts >= width yield 0).
    Shl { lhs: ValueId, rhs: ValueId },
    /// Shift right (arithmetic for signed `lhs`, logical for unsigned).
    Shr { lhs: ValueId, rhs: ValueId },

    // ---- Comparison (result is `i1`) ----
    /// Equality.
    Eq { lhs: ValueId, rhs: ValueId },
    /// Inequality.
    Ne { lhs: ValueId, rhs: ValueId },
    /// Less than (signed/unsigned/float per operand type).
    Lt { lhs: ValueId, rhs: ValueId },
    /// Less or equal.
    Le { lhs: ValueId, rhs: ValueId },
    /// Greater than.
    Gt { lhs: ValueId, rhs: ValueId },
    /// Greater or equal.
    Ge { lhs: ValueId, rhs: ValueId },

    // ---- Memory ----
    /// Allocate a stack slot for a value of type `pointee`; yields
    /// `ptr<pointee>`.
    Alloca { pointee: TypeId },
    /// Load a value of type `ty` from `pointer` (`ptr<ty>`).
    Load { ty: TypeId, pointer: ValueId },
    /// Store `value` into `pointer` (no result).
    Store { pointer: ValueId, value: ValueId },
    /// Element-wise pointer offset: yields a pointer to the `offset`-th
    /// element after `pointer`; same pointer type as `pointer`.
    PtrOffset {
        pointer: ValueId,
        offset: ValueId,
    },

    // ---- Conversion ----
    /// Integer extend (sign-extend for signed sources, zero-extend for
    /// unsigned). Strictly widening.
    Ext { to: TypeId, value: ValueId },
    /// Integer truncate. Strictly narrowing; wraps.
    Trunc { to: TypeId, value: ValueId },
    /// Integer to float (round to nearest, ties to even).
    IntToFloat { to: TypeId, value: ValueId },
    /// Float to integer (truncate toward zero, NaN -> 0, saturating).
    FloatToInt { to: TypeId, value: ValueId },
    /// Pointer to pointer bitcast (address preserved).
    PtrCast { to: TypeId, pointer: ValueId },

    // ---- Functions ----
    /// Direct call. `callee` may be a defined function or a declaration.
    Call {
        callee: FunctionId,
        args: Vec<ValueId>,
    },

    // ---- Aggregate ----
    /// Construct an aggregate from field values.
    Construct { ty: TypeId, fields: Vec<ValueId> },
    /// Read field `index` of an aggregate.
    Extract { aggregate: ValueId, index: u32 },
    /// `aggregate` with field `index` replaced by `value`; yields a new
    /// aggregate.
    Insert {
        aggregate: ValueId,
        index: u32,
        value: ValueId,
    },
}

/// A single instruction with its optional SSA result and span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// What the instruction does.
    pub kind: InstructionKind,
    /// SSA value produced by this instruction, if any (`store` and
    /// `void`-returning calls produce none).
    pub result: Option<ValueId>,
    /// Optional source span (parsed IR only).
    pub span: Option<Span>,
}

impl Instruction {
    /// Creates an instruction.
    pub fn new(kind: InstructionKind, result: Option<ValueId>) -> Self {
        Instruction {
            kind,
            result,
            span: None,
        }
    }
}

impl InstructionKind {
    /// Visits every [`ValueId`] referenced by this instruction.
    ///
    /// Useful for verifiers and future optimizer passes.
    pub fn visit_values(&self, f: &mut impl FnMut(ValueId)) {
        use InstructionKind::*;
        match self {
            Const(_) | Alloca { .. } => {}
            Add { lhs, rhs }
            | Sub { lhs, rhs }
            | Mul { lhs, rhs }
            | Div { lhs, rhs }
            | Rem { lhs, rhs }
            | And { lhs, rhs }
            | Or { lhs, rhs }
            | Xor { lhs, rhs }
            | Shl { lhs, rhs }
            | Shr { lhs, rhs }
            | Eq { lhs, rhs }
            | Ne { lhs, rhs }
            | Lt { lhs, rhs }
            | Le { lhs, rhs }
            | Gt { lhs, rhs }
            | Ge { lhs, rhs } => {
                f(*lhs);
                f(*rhs);
            }
            Load { pointer, .. } | PtrCast { pointer, .. } => {
                f(*pointer);
            }
            Extract { aggregate, .. } => {
                f(*aggregate);
            }
            Store { pointer, value } => {
                f(*pointer);
                f(*value);
            }
            PtrOffset { pointer, offset } => {
                f(*pointer);
                f(*offset);
            }
            Ext { value, .. }
            | Trunc { value, .. }
            | IntToFloat { value, .. }
            | FloatToInt { value, .. } => {
                f(*value);
            }
            Call { args, .. } | Construct { fields: args, .. } => {
                for &a in args {
                    f(a);
                }
            }
            Insert {
                aggregate,
                index: _,
                value,
            } => {
                f(*aggregate);
                f(*value);
            }
        }
    }
}
