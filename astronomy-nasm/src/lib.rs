//! # astronomy-nasm
//!
//! An out-of-tree **native backend** for [Astronomy IR](https://example.com):
//! it lowers a [`VerifiedModule`](astronomy::VerifiedModule) to NASM
//! (x86-64, System V AMD64) assembly text.
//!
//! ```text
//! Astronomy in-memory IR → Verifier → VerifiedModule ──▶ astronomy-nasm ──▶ .asm
//! ```
//!
//! The backend accepts only verified IR (the type-level contract from the
//! core crate), has no external dependencies, and emits deterministic text.
//!
//! ## Example
//!
//! ```no_run
//! use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut b = ModuleBuilder::with_name("demo");
//! let add = b.declare_function(
//!     "add",
//!     Linkage::Exported,
//!     Abi::C,
//!     &[("a", TypeId::I64), ("b", TypeId::I64)],
//!     TypeId::I64,
//! )?;
//! let mut fb = b.function_builder(add)?;
//! fb.append_block();
//! let (a, c) = (fb.param(0), fb.param(1));
//! let sum = fb.add(a, c)?;
//! fb.ret(Some(sum))?;
//!
//! let verified = Verifier::verify(b.finish())?;
//! let asm: String = astronomy_nasm::compile(&verified)?;
//! assert!(asm.contains("add:"));
//! # Ok(())
//! # }
//! ```
//!
//! ## Supported surface
//!
//! * All integer widths up to 64 bits (`i1`..`i64`, `u8`..`u64`), `f32`,
//!   `f64`, pointers, arrays and structs.
//! * Every instruction in the core instruction set, including
//!   `alloca`/`load`/`store`/`ptr_offset`, conversions, calls,
//!   `construct`/`extract`/`insert`, and all terminators with block
//!   arguments.
//! * The System V AMD64 calling convention, including variadic calls
//!   (`%al` semantics).
//!
//! ## Deliberate limits
//!
//! * `i128`/`u128` are rejected with `A-NASM-001` rather than lowered
//!   incorrectly.
//! * Aggregates (structs/arrays) are supported as in-memory values but not
//!   passed or returned by value; those cases return `A-NASM-002`.
//! * Every IR `Abi` (`c`, `astronomy`, `system`, `custom`) maps to the
//!   System V AMD64 convention; the `Abi` fact is not otherwise varied.

#![warn(missing_docs)]

mod codegen;
mod error;
mod layout;

pub use codegen::compile;
pub use error::BackendError;
