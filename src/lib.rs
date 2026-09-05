//! # Astronomy IR
//!
//! Astronomy is a language-agnostic, library-first intermediate
//! representation for compilers. Frontends build IR through the Rust API,
//! the [`Verifier`] validates it, and backends consume only
//! [`VerifiedModule`]s:
//!
//! ```text
//! Frontend (Rust/C/...) → Astronomy in-memory IR → Verifier → Backend
//! ```
//!
//! Core principle: **Astronomy Core IR contains facts, not questions.**
//! Types, symbols, call targets and control flow are fully resolved before
//! reaching Astronomy — the IR never performs name resolution or type
//! inference.
//!
//! ## Quick start
//!
//! ```
//! use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut builder = ModuleBuilder::with_name("demo");
//!
//! let add = builder.declare_function(
//!     "add",
//!     Linkage::Exported,
//!     Abi::Astronomy,
//!     &[("a", TypeId::I64), ("b", TypeId::I64)],
//!     TypeId::I64,
//! )?;
//!
//! let mut fb = builder.function_builder(add)?;
//! let entry = fb.append_block();
//! let (a, b) = (fb.param(0), fb.param(1));
//! let sum = fb.add(a, b)?;
//! fb.ret(Some(sum))?;
//!
//! let module = builder.finish();
//! let verified = Verifier::verify(module)?;
//!
//! // Canonical .arn text for debugging, snapshots and exchange.
//! println!("{}", astronomy::text::print(&verified));
//! # Ok(())
//! # }
//! ```
//!
//! ## Layout
//!
//! * [`types`], [`symbol`], [`constant`], [`value`] — interned tables and
//!   SSA value metadata.
//! * [`instruction`], [`block`], [`function`], [`module`] — the core model.
//! * [`builder`] — the frontend-facing construction API.
//! * [`verify`] — the verifier and [`VerifiedModule`].
//! * [`text`] — the `.arn` parser and canonical printer.
//!
//! ## Dependencies
//!
//! None. The core library is dependency-free by design (§56).

#![warn(missing_docs)]

pub mod block;
pub mod builder;
pub mod constant;
pub mod error;
pub mod function;
pub mod id;
pub mod instruction;
pub mod module;
pub mod span;
pub mod symbol;
pub mod types;
pub mod value;
pub mod verify;

pub mod text;

// ---------------------------------------------------------------------------
// Re-exports: the ergonomic surface of the library
// ---------------------------------------------------------------------------

pub use block::{BasicBlock, BlockParam, Terminator};
pub use builder::{FunctionBuilder, ModuleBuilder};
pub use constant::{ConstantData, ConstantStore};
pub use error::{BuildError, ParseError, VerifyError, VerifyErrorReport};
pub use function::{Abi, Function, Linkage, Param};
pub use id::{
    BlockId, ConstantId, FunctionId, GlobalId, MetadataId, SymbolId, TypeId, ValueId,
};
pub use instruction::{Instruction, InstructionKind};
pub use module::{ArnVersion, Module, VerifiedModule};
pub use span::Span;
pub use symbol::SymbolStore;
pub use types::{FloatKind, TypeData, TypeStore};
pub use value::{ValueData, ValueKind};
pub use verify::{verify, Verifier};

/// Convenience alias matching `use astronomy::Type` (§3).
pub type Type = TypeData;
