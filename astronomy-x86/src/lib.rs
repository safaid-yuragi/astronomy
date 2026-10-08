//! # astronomy-x86
//!
//! x86-64 (System V AMD64, Linux) instruction selection for Astronomy IR,
//! shared by Astronomy's two native backends:
//!
//! ```text
//!                                         ┌─▶ astronomy-object ─▶ ELF64 .o (built-in encoder)
//! VerifiedModule ─▶ astronomy-x86::lower ─┤
//!                                         └─▶ astronomy-nasm ───▶ NASM .asm text
//! ```
//!
//! [`lower`] turns a [`VerifiedModule`](astronomy::VerifiedModule) into a
//! [`Program`](asm::Program): typed x86-64 instructions ([`asm::Inst`]) per
//! function plus read-only data. The backends never re-derive anything
//! from the IR; they only print or encode this program, so their outputs
//! are the same code by construction.
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
//! * `i128`/`u128` are rejected with `A-X86-001` rather than lowered
//!   incorrectly.
//! * Aggregates (structs/arrays) are supported as in-memory values but not
//!   passed or returned by value; those cases return `A-X86-002`.
//! * Every IR `Abi` (`c`, `astronomy`, `system`, `custom`) maps to the
//!   System V AMD64 convention; the `Abi` fact is not otherwise varied.

#![warn(missing_docs)]

pub mod asm;
mod codegen;
mod error;
mod layout;

pub use codegen::lower;
pub use error::BackendError;
