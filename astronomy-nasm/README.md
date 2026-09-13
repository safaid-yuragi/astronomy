# astronomy-nasm

An **out-of-tree native backend** for [Astronomy IR](../README.md). It lowers
a verified Astronomy module to **NASM** assembly for **x86-64, System V
AMD64** (Linux/ELF64), with no external dependencies.

```text
Astronomy IR ──▶ Verifier ──▶ VerifiedModule ──▶ astronomy-nasm ──▶ .asm ──▶ nasm −f elf64
```

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
let asm = astronomy_nasm::compile(&verified)?;   // NASM text
```

As a CLI (`.arn` → NASM):

```bash
cargo run -p astronomy-nasm --bin arn2nasm -- hello.arn > hello.asm
cargo run -p astronomy-nasm --bin arn2nasm < hello.arn
```

Assemble and link (with `gcc` as the C driver/linker):

```bash
nasm -f elf64 hello.asm -o hello.o
gcc -no-pie driver.c hello.o -o hello
```

## Design

Correctness first, not peak performance:

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
  assembly.

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

Every IR `Abi` (`c`, `astronomy`, `system`, `custom`) currently maps to the
System V convention.

Invalid integer widths, overflowing type layouts and stack frames too large
for signed 32-bit displacements also fail with `A-NASM-001`. An outgoing
argument area that exceeds that frame limit fails with `A-NASM-002`.
Pointers to large types remain usable for address arithmetic.

## Tests

The suite is split between structural tests and real execution tests:

```bash
cargo test -p astronomy-nasm
```

* `tests/codegen_text.rs` — emitted directives, labels, determinism, error
  codes (no toolchain needed).
* `tests/run_integers.rs`, `run_widths.rs`, `run_floats.rs`,
  `run_control_flow.rs`, `run_memory.rs`, `run_calls.rs` — each builds IR,
  lowers it, **assembles with `nasm`, links with `gcc` and runs the
  resulting program**, asserting on real output.
* `tests/acceptance.rs` — full `.arn` round-trip then execution.
* `tests/cli.rs` — the `arn2nasm` binary.
* `tests/regressions.rs` — division overflow, mixed-width shifts, signed
  pointer offsets, large strides, empty aggregates, NASM keyword symbols
  and layout/frame overflow.
* `tests/differential.rs` — all supported integer widths and operators,
  mixed-width shifts, float/integer conversions checked against Rust casts,
  and interleaved register/stack arguments. Numeric checks use boundary
  values and deterministic random inputs through the ARN-to-executable pipeline.

Execution tests skip themselves (with a message) when `nasm`/`gcc` are not
installed. Ensure both tools are present when validating executable code;
otherwise Cargo reports those early-returning tests as successful skips.

## License

MIT OR Apache-2.0.
