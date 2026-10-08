# astronomy-x86

x86-64 (System V AMD64, Linux) **instruction selection** for
[Astronomy IR](../README.md), shared by Astronomy's two native backends:

```text
                                          ┌─▶ astronomy-object ─▶ ELF64 .o   (built-in encoder, no assembler)
VerifiedModule ─▶ astronomy_x86::lower ───┤
                                          └─▶ astronomy-nasm ───▶ NASM .asm text
```

`lower` turns a verified module into an `asm::Program`: typed x86-64
instructions per function (sized registers, memory operands, labels, data
references, direct calls — never text) plus read-only data. The backends
never look at the IR again; they only print or encode this program, so
NASM text and object code are the same code by construction.

```rust
let program: astronomy_x86::asm::Program = astronomy_x86::lower(&verified)?;
```

Most users want one of the backends instead:
[`astronomy-object`](../astronomy-object/README.md) for object files,
[`astronomy-nasm`](../astronomy-nasm/README.md) for assembly text. This
crate depends only on the core `astronomy` crate.

## Code shape

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
| `A-X86-001` | `i128`/`u128` (or a type containing one) is rejected |
| `A-X86-002` | aggregates passed/returned by value are rejected     |
| `A-X86-003` | unsupported ABI                                      |
| `A-X86-004` | internal inconsistency (unreachable for verified IR) |
| `A-X86-005` | not representable in an ELF object (e.g. NUL in a symbol name, > 2 GiB of code; `astronomy-object` only) |

Every IR `Abi` (`c`, `astronomy`, `system`, `custom`) currently maps to the
System V convention.

Invalid integer widths, overflowing type layouts and stack frames too large
for signed 32-bit displacements also fail with `A-X86-001`. An outgoing
argument area that exceeds that frame limit fails with `A-X86-002`.
Pointers to large types remain usable for address arithmetic.

## Tests

Lowering is exercised end to end through both backends: `astronomy-object`
runs every test program as a linked executable and, when NASM is
installed, checks that `astronomy-nasm`'s text assembles to the very same
bytes; `astronomy-nasm` checks the text itself.

## License

MIT OR Apache-2.0.
