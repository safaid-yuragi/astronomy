# Astronomy IR

Astronomy is a **language-agnostic, library-first intermediate
representation** for compilers, written in Rust. Frontends build IR through
the in-memory Rust API, the verifier checks it, and backends consume only
verified modules. A canonical textual format (`.arn`) exists for debugging,
snapshots, tests and exchange — normal compilation never passes through it.

```text
             Astronomy (Rust IR library)
                        ▲
          ┌─────────────┼─────────────┐
          │             │             │
      Rust           C             Other
     frontend      frontend       frontends
          │             │             │
          └─────────────┼─────────────┘
                        ▼
                Astronomy Core IR
                        ▼
             Verified Astronomy IR
                        │
          ┌─────────────┼─────────────┐
          ▼             ▼             ▼
        C backend   LLVM backend   native backends
```

Core principle: **Astronomy Core IR contains facts, not questions.** Types,
symbols, signatures, call targets, ABIs and control flow are fully resolved
before reaching Astronomy — the IR performs no name resolution and no type
inference.

- **Library first** — the deliverable is `use astronomy::…`, not a CLI.
- **SSA + block arguments** — no phi nodes; CFG is a first-class concept.
- **Numeric IDs everywhere** — `TypeId`, `ValueId`, `BlockId`,
  `FunctionId`, `SymbolId`, `ConstantId`; interned types and symbols.
- **Verifying** — `Module → Verifier → VerifiedModule`; backends accept
  `&VerifiedModule` only.
- **Canonical `.arn` text** — deterministic printing, stable roundtrips.
- **Zero dependencies** — the core crate has none.

## Quick start

```rust
use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Construct IR through the in-memory API.
    let mut builder = ModuleBuilder::with_name("demo");

    let add = builder.declare_function(
        "add",
        Linkage::Exported,
        Abi::Astronomy,
        &[("a", TypeId::I64), ("b", TypeId::I64)],
        TypeId::I64,
    )?;

    let mut fb = builder.function_builder(add)?;
    fb.append_block();
    let (a, b) = (fb.param(0), fb.param(1));
    let sum = fb.add(a, b)?;
    fb.ret(Some(sum))?;

    let module = builder.finish();

    // 2. Verify: invalid IR never reaches a backend.
    let verified = Verifier::verify(module)?;

    // 3. Serialize to canonical .arn (debugging, snapshots, exchange).
    println!("{}", astronomy::text::print(&verified));
    Ok(())
}
```

Output:

```arn
::ASTRONOMY::MODULE_START
::ASTRONOMY::MODULE_NAME "demo"
::ASTRONOMY::MODULE_VERSION 1
::ASTRONOMY::FUNCTION_START add fn(i64 %a, i64 %b) -> i64 linkage=exported abi=astronomy
::ASTRONOMY::BLOCK_START bb0
%0 = ::ASTRONOMY::ADD i64 i64 %a, i64 %b
::ASTRONOMY::RETURN i64 %0
::ASTRONOMY::BLOCK_END
::ASTRONOMY::FUNCTION_END
::ASTRONOMY::MODULE_END
```

Reading `.arn` back is symmetric:

```rust
let module = astronomy::text::parse(&text)?;   // or Module::parse_arn
let verified = module.verify()?;
```

## A larger taste: control flow with block arguments

```rust
use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mut b = ModuleBuilder::new();
let pick = b.declare_function(
    "pick",
    Linkage::Exported,
    Abi::Astronomy,
    &[("cond", TypeId::I1), ("a", TypeId::I64), ("b", TypeId::I64)],
    TypeId::I64,
)?;
let mut fb = b.function_builder(pick)?;
let (cond, a, b_) = (fb.param(0), fb.param(1), fb.param(2));

let entry = fb.append_block();
let left  = fb.append_block_with_params(&[("x", TypeId::I64)])?;
let right = fb.append_block_with_params(&[("x", TypeId::I64)])?;
let merge = fb.append_block_with_params(&[("result", TypeId::I64)])?;

fb.switch_to(entry)?;
fb.branch(cond, left, &[a], right, &[b_])?;

let x = fb.block_param(left, 0)?;
fb.switch_to(left)?;
fb.jump(merge, &[x])?;

let x = fb.block_param(right, 0)?;
fb.switch_to(right)?;
fb.jump(merge, &[x])?;

let result = fb.block_param(merge, 0)?;
fb.switch_to(merge)?;
fb.ret(Some(result))?;

let verified = Verifier::verify(b.finish())?;
# Ok(())
# }
```

## External symbols (e.g. libc)

Astronomy records the *fact* of an external function — signature and ABI —
and nothing else. Header paths and C specifics belong to a C backend:

```rust
# use astronomy::{Abi, ModuleBuilder, TypeId};
# fn main() -> Result<(), astronomy::BuildError> {
let mut b = ModuleBuilder::new();
let i8_ptr = b.ptr_type(TypeId::I8);
let printf = b.declare_extern("printf", Abi::C, &[i8_ptr], /* variadic */ true, TypeId::I32)?;
# let _ = printf;
# Ok(())
# }
```

prints as:

```arn
::ASTRONOMY::FUNCTION_START printf fn(ptr<i8>, ...) -> i32 linkage=external abi=c
::ASTRONOMY::FUNCTION_END
```

## Project layout

```text
astronomy/
├── Cargo.toml           # zero dependencies
├── SPECIFICATION.md     # semantics + .arn grammar
├── USAGE.md             # library usage guide (build → verify → .arn)
├── src/
│   ├── lib.rs           # public API re-exports
│   ├── id.rs            # numeric ID newtypes
│   ├── types.rs         # type system + interning
│   ├── symbol.rs        # symbol interning
│   ├── constant.rs      # constant pool
│   ├── value.rs         # SSA value metadata
│   ├── instruction.rs   # instruction set
│   ├── block.rs         # basic blocks + terminators
│   ├── function.rs      # functions, linkage, ABI
│   ├── module.rs        # Module, VerifiedModule, ArnVersion
│   ├── builder.rs       # ModuleBuilder / FunctionBuilder
│   ├── verify/          # verifier (ssa, types, cfg + dominance)
│   ├── text/            # .arn lexer / parser / canonical printer
│   └── bin/astronomy.rs # small dev CLI (verify/fmt/inspect)
├── examples/            # add, cfg, block_args, extern_decl
├── tests/               # acceptance, negative, roundtrip suites
└── astronomy-nasm/      # out-of-tree NASM (x86-64) native backend crate
```

## Development CLI

```bash
cargo run --quiet --example add          # build + verify + print .arn
astronomy verify hello.arn               # parse + verify
astronomy fmt    hello.arn               # parse + canonical print
astronomy inspect hello.arn              # module summary
```

## Error model

All failures are structured enums with stable codes, never `String`
errors:

| Prefix    | Meaning                                   |
|-----------|-------------------------------------------|
| `A-BUILD` | builder rejected invalid construction     |
| `A-VERIFY`| verifier rejected invalid IR              |
| `A-ARN`   | `.arn` text could not be parsed           |

The verifier reports *all* errors it finds via `VerifyErrorReport`
(iterable, `Display`-able, `std::error::Error`).

## Status

MVP complete per the prototype definition of done:

- [x] Rust API construction (add, branches, block arguments, externs)
- [x] Verifier accepts valid IR / rejects invalid IR
- [x] Canonical `.arn` serialization
- [x] `.arn` re-parse into an equivalent, verifying module
- [x] Deterministic printing (snapshot- and cache-friendly)

One out-of-tree backend ships in this repository: **`astronomy-nasm`** — a
NASM (x86-64, System V AMD64) native backend that lowers verified modules to
assembly text (see [`astronomy-nasm/README.md`](astronomy-nasm/README.md)).
It is a separate workspace member, so the core crate keeps zero backend code
and zero backend dependencies.

Future work (by design, not yet implemented): `.arb` binary format, optimizer
passes, and further out-of-tree backends (`astronomy-c`, `astronomy-llvm`, …).

## License

MIT OR Apache-2.0.
