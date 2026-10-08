# astronomy-nasm

An **out-of-tree native backend** for [Astronomy IR](../README.md) targeting
**x86-64, System V AMD64** (Linux/ELF64), with no external dependencies. It
lowers a verified Astronomy module to

* an **ELF64 relocatable object** (`.o`), encoded by the crate's own
  machine-code encoder and ELF writer — no assembler involved; or
* **NASM** assembly text for the very same instructions.

```text
                                                   ┌─▶ encoder ─▶ ELF writer ─▶ .o ─▶ cc / ld
VerifiedModule ─▶ instruction selection ─▶ x86-64 ─┤
                                           insts   └─▶ NASM printer ─▶ .asm (optional)
```

Both outputs come from one structured instruction stream, and they agree
byte for byte: assembling the text with `nasm -f elf64` yields exactly the
`.text` and `.rodata` of the built-in object (the test suite checks this).

The core crate stays backend-free (§17 Non-goals, §56): this crate depends on
`astronomy` by path and consumes only `&VerifiedModule`.

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
let object: Vec<u8> = astronomy_nasm::compile_object(&verified)?;  // ELF64 .o
std::fs::write("demo.o", &object)?;
let asm: String = astronomy_nasm::compile(&verified)?;             // NASM text
```

As a CLI (`.arn` or `.arb` input):

```bash
arn2nasm --emit obj hello.arn -o hello.o   # ELF object, built-in encoder
arn2nasm hello.arn > hello.asm             # NASM text (default)
arn2nasm --emit obj -o hello.o < hello.arn # stdin works too
```

Link the object like any compiler output — PIE (the default on most
distributions), non-PIE and shared libraries all work:

```bash
cc main.c hello.o -o hello
cc -shared hello.o -o libhello.so
```

The NASM text remains available for reading and debugging; to assemble it
yourself use `nasm -f elf64 hello.asm -o hello.o`.

## Design

Correctness first, not peak performance.

### Pipeline

* **Instruction selection** (`codegen.rs`) lowers IR to a typed x86-64
  instruction model (`asm.rs`): sized registers, memory operands, labels,
  data references and direct calls — never text.
* **NASM printer** (`nasm.rs`) renders that model as NASM source.
* **Encoder** (`encode.rs`) turns it into machine code: one canonical,
  shortest encoding per instruction form (REX/ModRM/SIB, `imm8` vs `imm32`,
  zero-extending `mov r32, imm`, ...), and branch relaxation that starts
  every `jmp`/`jcc` short and widens it only when its target is out of
  `rel8` range. Sizing runs in passes over the whole section in the same
  order NASM's optimizer uses, which is what makes the two outputs
  byte-identical; a final grow-only pass guarantees every short branch is
  in range.
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

### Code shape

* **Everything lives in a stack slot.** Every SSA value — parameter, block
  parameter or instruction result — gets an `rbp`-relative slot; instructions
  load operands into registers, compute, and store the result back. There is
  no register allocator, so lowering stays local and auditable.
* **Blocks are labels; terminators are jumps.** `jump`/`branch` become
  `jmp`/`je`; `return` becomes `leave; ret`; `unreachable` becomes `ud2`.
* **Block arguments are parallel copies.** A jump stages all its argument
  values into a per-block scratch area, then commits them to the target
  block's parameter slots. The staging pass is what makes `jump bb(p1, p0)`
  correct instead of clobbering a slot mid-copy.
* **Fixed, aligned frame.** All slots are `rbp`-relative and `rsp` stays at a
  16-byte boundary, with an outgoing stack-argument area reserved at the
  bottom, so calls always see a properly aligned stack.
* **System V AMD64 ABI** for every function: integer/pointer arguments in
  `rdi, rsi, rdx, rcx, r8, r9`, float arguments in `xmm0..xmm7`, the rest on
  the stack, returns in `rax`/`xmm0`, and `%al` set for variadic calls.
* **Deterministic output** — the same module always produces byte-identical
  assembly and object files (no timestamps, paths or host data).

## Supported surface

* Types: `i1`, `i8`..`i64`, `u8`..`u64`, `f32`, `f64`, pointers, arrays,
  structs (used as in-memory values).
* Instructions: `const` (int, float, null, string, aggregate), `add`/`sub`/
  `mul`/`div`/`rem`, `and`/`or`/`xor`/`shl`/`shr`, `eq`/`ne`/`lt`/`le`/`gt`/`ge`,
  `alloca`/`load`/`store`/`ptr_offset`, `ext`/`trunc`/`int_to_float`/
  `float_to_int`/`ptr_cast`, `call`, `construct`/`extract`/`insert`.
* Terminators: `jump`, `branch`, `return`, `unreachable` — all with block
  arguments.

Semantics follow `SPECIFICATION.md`: wrapping integer arithmetic, truncating
signed division, arithmetic vs logical `shr` by signedness, out-of-range shifts
producing `0`/the sign bit, saturating `float_to_int` with `NaN → 0`, and
IEEE-754 float comparisons that treat `NaN` as unordered.
`INT_MIN / -1` wraps and its remainder is zero. Shift counts may use any
supported integer type independently of the shifted value; `ptr_offset`
interprets the offset as signed even when its IR type is unsigned.

## Deliberate limits

Unsupported input fails with a structured error, never wrong code:

| Code        | Meaning                                             |
|-------------|-----------------------------------------------------|
| `A-NASM-001`| `i128`/`u128` (or a type containing one) is rejected |
| `A-NASM-002`| aggregates passed/returned by value are rejected     |
| `A-NASM-003`| unsupported ABI                                      |
| `A-NASM-004`| internal inconsistency (unreachable for verified IR) |
| `A-NASM-005`| not representable in an ELF object (e.g. NUL in a symbol name, > 2 GiB of code) |

Every IR `Abi` (`c`, `astronomy`, `system`, `custom`) currently maps to the
System V convention.

Invalid integer widths, overflowing type layouts and stack frames too large
for signed 32-bit displacements also fail with `A-NASM-001`. An outgoing
argument area that exceeds that frame limit fails with `A-NASM-002`.
Pointers to large types remain usable for address arithmetic.

## Tests

```bash
cargo test -p astronomy-nasm
```

* `tests/run_integers.rs`, `run_widths.rs`, `run_floats.rs`,
  `run_control_flow.rs`, `run_memory.rs`, `run_calls.rs` — each builds IR,
  **emits an ELF object with the built-in encoder, links it with `gcc` (as a
  PIE, with no linker warnings allowed) and runs the program**, asserting on
  real output.
* `tests/acceptance.rs` — full `.arn` round-trip then execution.
* `tests/differential.rs` — all supported integer widths and operators,
  mixed-width shifts, float/integer conversions checked against Rust casts,
  and interleaved register/stack arguments, with boundary values and
  deterministic random inputs.
* `tests/regressions.rs` — division overflow, mixed-width shifts, signed
  pointer offsets, large strides, empty aggregates, NASM keyword symbols
  and layout/frame overflow.
* `tests/object_file.rs` — ELF structure, symbol binding/types/sizes,
  PIE-friendly relocations, determinism, long-branch relaxation, linking
  into a shared library with `-z text`, and `readelf` acceptance.
* `tests/codegen_text.rs` — emitted NASM directives, labels, determinism,
  error codes (no toolchain needed).
* `tests/cli.rs` — the `arn2nasm` binary, including `--emit obj`.
* Encoder unit tests (`src/encode.rs`) check individual encodings.

**NASM as an independent reference.** NASM is never part of the pipeline,
but when it is installed the suite uses it as an oracle:

* every execution test also assembles the printed NASM text and requires
  its `.text`, `.rodata` and relocations to match the built-in object;
* `src/nasm_crosscheck.rs` encodes every instruction form over the full
  register file and every addressing shape (10,000+ instructions), plus 300
  random branch-dense functions, and requires NASM's bytes to be identical.

Execution tests need only `gcc` (the C driver and linker) and skip
themselves with a message when it is missing; the NASM cross-checks skip
when `nasm` is missing.

## License

MIT OR Apache-2.0.
