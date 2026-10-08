//! # astronomy-nasm
//!
//! A native backend for Astronomy IR that lowers a
//! [`VerifiedModule`] to **NASM** assembly text for x86-64 (System V AMD64,
//! Linux/ELF64).
//!
//! ```text
//! VerifiedModule ──▶ astronomy-x86 (instruction selection) ──▶ astronomy-nasm ──▶ .asm ──▶ nasm -f elf64
//! ```
//!
//! Instruction selection is shared with `astronomy-object`, Astronomy's own
//! object-file backend, through `astronomy-x86`; this crate only prints.
//! The backend accepts only verified IR, has no external dependencies, and
//! its output is deterministic.
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
//! Supported surface and limits are those of `astronomy-x86`; errors are
//! its [`BackendError`] (`A-X86-*`).

#![warn(missing_docs)]

mod printer;

pub use astronomy_x86::BackendError;

use astronomy::VerifiedModule;
use astronomy_x86::asm::Program;

/// Lowers a verified module to NASM (x86-64, System V AMD64) source text.
pub fn compile(module: &VerifiedModule) -> Result<String, BackendError> {
    astronomy_x86::lower(module).map(|program| print(&program))
}

/// Renders an already-lowered program as NASM source text.
pub fn print(program: &Program) -> String {
    printer::print(program)
}
