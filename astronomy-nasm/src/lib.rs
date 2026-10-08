//! # astronomy-nasm
//!
//! An out-of-tree **native backend** for Astronomy IR targeting x86-64
//! (System V AMD64, Linux). It lowers a
//! [`VerifiedModule`] to
//!
//! * an **ELF64 relocatable object** with [`compile_object`], using the
//!   crate's own machine-code encoder and ELF writer — no assembler is
//!   involved; link the result with `cc`/`ld` like any compiler output; or
//! * **NASM** assembly text with [`compile`].
//!
//! ```text
//! VerifiedModule ──▶ instruction selection ──┬──▶ encoder + ELF writer ──▶ .o
//!                                            └──▶ NASM printer ──────────▶ .asm
//! ```
//!
//! Both outputs come from the same structured instruction stream and agree
//! byte for byte: `nasm -f elf64` on the text yields exactly the `.text`
//! and `.rodata` of [`compile_object`]'s output. The backend accepts only
//! verified IR (the type-level contract from the core crate), has no
//! external dependencies, and its output is deterministic.
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
//!
//! // An object file, ready for `cc main.c demo.o`:
//! std::fs::write("demo.o", astronomy_nasm::compile_object(&verified)?)?;
//!
//! // The same code as NASM text:
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
//! * Modules that an ELF object cannot represent (a NUL byte in a symbol
//!   name, more than 2 GiB of code) fail [`compile_object`] with
//!   `A-NASM-005`.

#![warn(missing_docs)]

mod asm;
mod codegen;
mod elf;
mod encode;
mod error;
mod layout;
mod nasm;

#[cfg(test)]
mod nasm_crosscheck;

pub use error::BackendError;

use astronomy::VerifiedModule;

/// Lowers a verified module to NASM (x86-64, System V AMD64) source text.
pub fn compile(module: &VerifiedModule) -> Result<String, BackendError> {
    codegen::lower(module).map(|program| nasm::print(&program))
}

/// Lowers a verified module straight to an ELF64 relocatable object
/// (`.o`) for x86-64 Linux, using the built-in encoder — no assembler is
/// involved. Link the result with `cc`/`ld` like any compiler output.
pub fn compile_object(module: &VerifiedModule) -> Result<Vec<u8>, BackendError> {
    let program = codegen::lower(module)?;
    let text = encode::assemble(&program)?;
    elf::write(&program, &text)
}
