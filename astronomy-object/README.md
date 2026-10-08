# astronomy-object

**Astronomy's own native backend.** It compiles a verified
[Astronomy IR](../README.md) module for **x86-64 Linux (System V AMD64)**
straight to an **ELF64 relocatable object** (`.o`). The machine-code
encoder and the ELF writer live in this crate, and instruction selection in
[`astronomy-x86`](../astronomy-x86/README.md); there are no external
dependencies and no assembler or other tool is involved. Link the result
with `cc`/`ld` like any compiler output.

```text
VerifiedModule ─▶ astronomy-x86 (instruction selection) ─▶ encoder ─▶ ELF writer ─▶ .o ─▶ cc / ld
```

For assembly text of the same code, use
[`astronomy-nasm`](../astronomy-nasm/README.md); the two backends are
independent crates that share only `astronomy-x86`.

## Usage

As a library:

```rust
use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

let mut b = ModuleBuilder::with_name("demo");
let add = b.declare_function(
    "add", Linkage::Exported, Abi::C,
    &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I64,
)?;
let mut fb = b.function_builder(add)?;
fb.append_block();
let (a, c) = (fb.param(0), fb.param(1));
let sum = fb.add(a, c)?;
fb.ret(Some(sum))?;

let verified = Verifier::verify(b.finish())?;
let object: Vec<u8> = astronomy_object::compile(&verified)?; // ELF64 .o
std::fs::write("demo.o", &object)?;
```

`astronomy_object::assemble(&program)` encodes an already-lowered
`astronomy_x86::asm::Program`.

As a CLI (`.arn` or `.arb` input):

```bash
arn2obj hello.arn                  # writes hello.o (like `cc -c`)
arn2obj hello.arb -o out.o         # explicit output file
arn2obj -o hello.o < hello.arn     # read stdin
```

Link the object like any compiler output — PIE (the default on most
distributions), non-PIE and shared libraries all work:

```bash
cc main.c hello.o -o hello
cc -shared hello.o -o libhello.so
```

## Design

* **Encoder** (`encode.rs`) turns each `astronomy-x86` instruction into
  machine code: one canonical, shortest encoding per instruction form
  (REX/ModRM/SIB, `imm8` vs `imm32`, zero-extending `mov r32, imm`, ...),
  and branch relaxation that starts every `jmp`/`jcc` short and widens it
  only when its target is out of `rel8` range. Sizing runs in passes over
  the whole section in the same order NASM's optimizer uses, which keeps
  the object byte-identical to NASM's assembly of `astronomy-nasm`'s text;
  a final grow-only pass guarantees every short branch is in range.
* **ELF writer** (`elf.rs`) emits a deterministic `ET_REL` object:
  `.text`, `.rodata`, an empty `.note.GNU-stack` (no executable stack,
  no linker warning), `.rela.text`, `.symtab`, `.strtab`, `.shstrtab`.
  Internal functions become local `STT_FUNC` symbols, exported/external
  definitions global ones (with sizes, so debuggers and `objdump` see
  real functions); only declarations that are actually called appear as
  undefined symbols. Calls to declarations use `R_X86_64_PLT32` and data
  is reached RIP-relatively (`R_X86_64_PC32`), so there are no text
  relocations and the object links into PIE executables and shared
  libraries. Calls between functions of the module are resolved in place.
* **Deterministic** — no timestamps, paths or host data in the object.

How IR is lowered (stack slots, block arguments, frame, ABI), the supported
surface and the error codes are documented in
[`astronomy-x86`](../astronomy-x86/README.md). Modules an ELF object cannot
represent (a NUL byte in a symbol name, more than 2 GiB of code) fail with
`A-X86-005`.

## Tests

```bash
cargo test -p astronomy-object
```

* `tests/run_integers.rs`, `run_widths.rs`, `run_floats.rs`,
  `run_control_flow.rs`, `run_memory.rs`, `run_calls.rs` — each builds IR,
  **compiles it to an object, links it with `gcc` (as a PIE, with no linker
  warnings allowed) and runs the program**, asserting on real output.
* `tests/differential.rs` — all supported integer widths and operators,
  mixed-width shifts, float/integer conversions checked against Rust casts,
  and interleaved register/stack arguments, with boundary values and
  deterministic random inputs.
* `tests/regressions.rs` — division overflow, mixed-width shifts, signed
  pointer offsets, large strides, empty aggregates, register/keyword-named
  symbols and layout/frame overflow.
* `tests/object_file.rs` — ELF structure, symbol binding/types/sizes,
  PIE-friendly relocations, determinism, long-branch relaxation, linking
  into a shared library with `-z text`, and `readelf` acceptance.
* `tests/cli.rs` — the `arn2obj` binary: output naming, stdin/stdout,
  `.arb` input and error reporting.
* Encoder unit tests (`src/encode.rs`) check individual encodings.

**NASM as an independent reference.** NASM is never part of the pipeline,
but when it is installed the suite uses it as an oracle (through
`astronomy-nasm`, a dev-dependency only):

* every execution test also assembles `astronomy-nasm`'s text for the same
  module and requires its `.text`, `.rodata` and relocations to match the
  object;
* `tests/nasm_crosscheck.rs` encodes every instruction form over the full
  register file and every addressing shape (10,000+ instructions), plus 300
  random branch-dense functions, and requires NASM's bytes to be identical.

Execution tests need only `gcc` (the C driver and linker) and skip
themselves with a message when it is missing; the NASM cross-checks skip
when `nasm` is missing.

## License

MIT OR Apache-2.0.
