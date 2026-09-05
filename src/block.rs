//! Basic blocks and terminators (§13 Basic Block, §16 Terminator).

use crate::id::{BlockId, SymbolId, TypeId, ValueId};
use crate::instruction::Instruction;
use crate::span::Span;

/// A basic block parameter (§14 Block Arguments — used instead of phi
/// nodes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockParam {
    /// The SSA value that binds on entry to the block.
    pub value: ValueId,
    /// Declared type (must equal `values[value].ty`; the verifier
    /// cross-checks this).
    pub ty: TypeId,
}

/// A basic block: parameters, straight-line instructions, and exactly one
/// terminator. A block must always end in a terminator; during construction
/// the terminator is temporarily absent and the verifier rejects blocks
/// left in that state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    /// Block parameters bound by predecessor branch arguments.
    pub params: Vec<BlockParam>,
    /// Straight-line instructions; no control flow in here.
    pub instructions: Vec<Instruction>,
    /// Exactly one terminator once the block is complete.
    pub terminator: Option<Terminator>,
    /// Optional debug label (never part of semantic identity).
    pub name: Option<SymbolId>,
    /// Optional source span (parsed IR only).
    pub span: Option<Span>,
}

impl BasicBlock {
    /// Creates an empty block.
    pub fn new() -> Self {
        BasicBlock {
            params: Vec::new(),
            instructions: Vec::new(),
            terminator: None,
            name: None,
            span: None,
        }
    }

    /// Returns the terminator, if the block is complete.
    pub fn terminator(&self) -> Option<&Terminator> {
        self.terminator.as_ref()
    }
}

impl Default for BasicBlock {
    fn default() -> Self {
        Self::new()
    }
}

/// Control-flow terminator of a basic block.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminator {
    /// Unconditional jump, passing `args` to `target`'s block parameters.
    Jump {
        /// Destination block.
        target: BlockId,
        /// Arguments bound to the target's block parameters.
        args: Vec<ValueId>,
    },
    /// Conditional branch. Both destinations receive their own arguments.
    Branch {
        /// Condition; must have type `i1`.
        condition: ValueId,
        /// Block taken when the condition is true.
        then_block: BlockId,
        /// Arguments passed to `then_block`.
        then_args: Vec<ValueId>,
        /// Block taken when the condition is false.
        else_block: BlockId,
        /// Arguments passed to `else_block`.
        else_args: Vec<ValueId>,
    },
    /// Return from the function.
    Return {
        /// Returned value; `None` exactly when the function returns `void`.
        value: Option<ValueId>,
    },
    /// A control-flow edge that must never be reached at runtime.
    Unreachable,
}

impl Terminator {
    /// Visits every [`ValueId`] referenced by this terminator.
    pub fn visit_values(&self, f: &mut impl FnMut(ValueId)) {
        match self {
            Terminator::Jump { args, .. } => {
                for &a in args {
                    f(a);
                }
            }
            Terminator::Branch {
                condition,
                then_args,
                else_args,
                ..
            } => {
                f(*condition);
                for &a in then_args {
                    f(a);
                }
                for &a in else_args {
                    f(a);
                }
            }
            Terminator::Return { value } => {
                if let Some(v) = value {
                    f(*v);
                }
            }
            Terminator::Unreachable => {}
        }
    }

    /// Visits every successor [`BlockId`] of this terminator.
    pub fn visit_targets(&self, f: &mut impl FnMut(BlockId)) {
        match self {
            Terminator::Jump { target, .. } => f(*target),
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                f(*then_block);
                f(*else_block);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }
}
