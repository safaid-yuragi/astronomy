//! SSA values (§12 SSA).
//!
//! Values are per-function entities numbered by a dense arena. A value is
//! defined exactly once by one of:
//!
//! * a function parameter,
//! * a block parameter,
//! * the result of an instruction.
//!
//! Values are immutable; memory mutation is expressed with `load`/`store`.

use crate::id::{BlockId, SymbolId, TypeId};
use crate::span::Span;

/// Where a value came from. Used by the verifier to check SSA form.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// Reserved but not yet attached to a definition. Only visible while
    /// low-level construction (parsers/deserializers) is in progress; the
    /// verifier rejects any value left in this state.
    Reserved,
    /// Function parameter at position `index`.
    Param {
        /// Zero-based parameter index.
        index: u32,
    },
    /// Basic block parameter.
    BlockParam {
        /// Owning block.
        block: BlockId,
        /// Zero-based parameter index within the block.
        index: u32,
    },
    /// Result of an instruction.
    Inst {
        /// Block containing the defining instruction.
        block: BlockId,
        /// Index of the defining instruction within its block.
        index: u32,
    },
}

/// Arena entry describing one SSA value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueData {
    /// Type of the value.
    pub ty: TypeId,
    /// Definition site.
    pub kind: ValueKind,
    /// Optional debug name. Never part of semantic identity.
    pub name: Option<SymbolId>,
    /// Optional source span (parsed IR only).
    pub span: Option<Span>,
}

impl ValueData {
    /// Creates a value description.
    pub fn new(ty: TypeId, kind: ValueKind) -> Self {
        ValueData {
            ty,
            kind,
            name: None,
            span: None,
        }
    }
}
