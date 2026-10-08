//! # astronomy-object
//!
//! Astronomy's own **native backend**: it compiles a [`VerifiedModule`] for
//! x86-64 Linux (System V AMD64) **straight to an ELF64 relocatable object**
//! (`.o`). The machine-code encoder and the ELF writer are part of this
//! crate, and instruction selection comes from `astronomy-x86` — no
//! assembler or other external tool is involved. Link the result with
//! `cc`/`ld` like any compiler output.
//!
//! ```text
//! VerifiedModule ──▶ astronomy-x86 (instruction selection) ──▶ encoder ──▶ ELF writer ──▶ .o
//! ```
//!
//! The object is deterministic and links into PIE executables, non-PIE
//! executables and shared libraries. The backend accepts only verified IR
//! and has no external dependencies.
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
//! // An object file, ready for `cc main.c demo.o`:
//! std::fs::write("demo.o", astronomy_object::compile(&verified)?)?;
//! # Ok(())
//! # }
//! ```
//!
//! Supported surface and lowering limits are those of `astronomy-x86`;
//! errors are its [`BackendError`] (`A-X86-*`). Modules an ELF object
//! cannot represent (a NUL byte in a symbol name, more than 2 GiB of code)
//! fail with `A-X86-005`.

#![warn(missing_docs)]

mod elf;
mod encode;

pub use astronomy_x86::BackendError;

use astronomy::VerifiedModule;
use astronomy_x86::asm::Program;

/// Compiles a verified module straight to an ELF64 relocatable object
/// (`.o`) for x86-64 Linux, using the built-in encoder — no assembler is
/// involved. Link the result with `cc`/`ld` like any compiler output.
pub fn compile(module: &VerifiedModule) -> Result<Vec<u8>, BackendError> {
    astronomy_x86::lower(module).and_then(|program| assemble(&program))
}

/// Encodes an already-lowered program into an ELF64 relocatable object.
pub fn assemble(program: &Program) -> Result<Vec<u8>, BackendError> {
    let text = encode::encode_text(program)?;
    elf::write(program, &text)
}
