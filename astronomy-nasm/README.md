# astronomy-nasm

A native backend for [Astronomy IR](../README.md) that lowers a verified
module to **NASM** assembly text for **x86-64, System V AMD64**
(Linux/ELF64), with no external dependencies.

```text
VerifiedModule ──▶ astronomy-x86 (instruction selection) ──▶ astronomy-nasm ──▶ .asm ──▶ nasm -f elf64
```

Instruction selection lives in [`astronomy-x86`](../astronomy-x86/README.md)
and is shared with [`astronomy-object`](../astronomy-object/README.md),
Astronomy's own backend that writes `.o` files directly without any
assembler. This crate only prints; assembling its output with
`nasm -f elf64` gives exactly the `.text` and `.rodata` that
`astronomy-object` produces for the same module.

The core crate stays backend-free (§17 Non-goals, §56): this crate depends on
`astronomy` and `astronomy-x86` by path and consumes only `&VerifiedModule`.

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

`astronomy_nasm::print(&program)` renders an already-lowered
`astronomy_x86::asm::Program`.

As a CLI (`.arn` or `.arb` input):

```bash
cargo run -p astronomy-nasm --bin arn2nasm -- hello.arn > hello.asm
cargo run -p astronomy-nasm --bin arn2nasm < hello.arn
```

Assemble and link (with `gcc` as the C driver/linker):

```bash
nasm -f elf64 hello.asm -o hello.o
gcc -no-pie driver.c hello.o -o hello
```

## Output

* `bits 64` / `default rel`, `extern` for declarations and `global` for
  exported/external definitions, in module order.
* One label per function (`$name`, so even register or directive names are
  valid symbols), block labels `name.bbN`, local labels for internal
  branches.
* `section .rodata` with string literals and float-conversion bounds
  (`arn.data.N`).
* Deterministic: the same module always produces byte-identical text.

How IR is lowered (stack slots, block arguments, frame, ABI), the supported
surface and the error codes (`A-X86-*`) are documented in
[`astronomy-x86`](../astronomy-x86/README.md).

## Tests

```bash
cargo test -p astronomy-nasm
```

* `tests/codegen_text.rs` — emitted directives, labels, determinism, error
  codes (no toolchain needed).
* `tests/acceptance.rs` — `.arn` round-trip, then assemble with `nasm`, link
  with `gcc` and run.
* `tests/cli.rs` — the `arn2nasm` binary, including `.arb` input.

Execution tests skip themselves (with a message) when `nasm`/`gcc` are not
installed. The full execution suites live in `astronomy-object`; whenever
`nasm` is installed they also assemble this crate's text for every program
and require it to match the object byte for byte.

## License

MIT OR Apache-2.0.
