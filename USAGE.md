# Astronomy IR — Library Usage Guide

This guide is for code (human- or agent-written) that consumes the
`astronomy` crate **as a library**: building IR in memory, verifying it,
and exchanging it as canonical `.arn` text. It complements
[`README.md`](README.md) (overview) and [`SPECIFICATION.md`](SPECIFICATION.md)
(normative semantics and the full `.arn` grammar).

---

## 1. What this library is

Astronomy is a language-agnostic, **library-first** intermediate
representation written in Rust, with **zero external dependencies**.

```text
Frontend (your code) → ModuleBuilder (Rust API) → Module
                     → Verifier → VerifiedModule → Backend (your code)
                                   ↕
                            .arn text (debug / snapshots / exchange only)
```

Core principle: **Astronomy Core IR contains facts, not questions.** Your
frontend must already have resolved types, symbol identity, call targets
(direct, by `FunctionId`), ABIs, and branch destinations. The IR performs
no name resolution, type inference, or overload resolution.

The type-level contract: backends accept `&VerifiedModule` only. There is
no public way to construct a `VerifiedModule` without passing the verifier.

## 2. Adding it to your project

```toml
[dependencies]
astronomy = { path = "../astronomy" }          # in-repo
# or
astronomy = { git = "https://example.com/astronomy" }
```

Everything you normally need is re-exported at the crate root:

```rust
use astronomy::{
    // construction
    ModuleBuilder, FunctionBuilder,
    // IDs (opaque, Copy, strongly typed)
    TypeId, SymbolId, ConstantId, ValueId, BlockId, FunctionId,
    // model
    Module, VerifiedModule, ArnVersion, Function, Linkage, Abi, Param,
    BasicBlock, BlockParam, Terminator, Instruction, InstructionKind,
    TypeData, TypeStore, SymbolStore, ConstantData, ConstantStore,
    ValueData, ValueKind, Span, FloatKind,
    // verification
    Verifier, verify,
    // errors
    BuildError, VerifyError, VerifyErrorReport, ParseError,
};
// astronomy::text::{print, parse} for the textual format
```

## 3. Core concepts

| Concept | Handle | Notes |
|---------|--------|-------|
| Type (interned) | `TypeId` | scalars pre-interned as constants: `TypeId::VOID, I1, I8..I128, U8..U128, F32, F64` |
| Symbol (interned name) | `SymbolId` | via `SymbolStore` |
| Constant (interned) | `ConstantId` | module-level pool, append-only |
| SSA value (per function) | `ValueId` | defined exactly once: param, block param, or instruction result |
| Basic block (per function) | `BlockId` | block 0 is the entry block |
| Function (per module) | `FunctionId` | definition (has blocks) or extern declaration (no blocks) |

All handles are dense `u32` newtypes (`Copy`, `Eq`, `Hash`, `Debug`,
`Display` like `v3`, `bb1`, `f0`). **IDs are untrusted until verified** — a
fabricated ID is a structured error, never undefined behavior.

SSA with **block arguments** replaces phi nodes: to merge values, declare
parameters on the target block and pass arguments on the jump/branch that
enters it.

First-class types (legal for values, params, pointees): integers (incl.
`i1`), floats, pointers, arrays, structs. `void` and `fn` signatures are
not first-class; `void` is legal only as a function result. Function
pointers are **not expressible** — calls are always direct by `FunctionId`.

## 4. Quick start

```rust
use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Build IR through the in-memory API.
    let mut builder = ModuleBuilder::with_name("demo");

    let add = builder.declare_function(
        "add",
        Linkage::Exported,
        Abi::Astronomy,
        &[("a", TypeId::I64), ("b", TypeId::I64)],
        TypeId::I64,
    )?;

    let mut fb = builder.function_builder(add)?;
    fb.append_block();                       // first block = entry
    let (a, b) = (fb.param(0), fb.param(1)); // function parameters
    let sum = fb.add(a, b)?;
    fb.ret(Some(sum))?;

    let module = builder.finish();

    // 2. Verify: invalid IR never reaches a backend.
    let verified = Verifier::verify(module)?;

    // 3. Serialize to canonical .arn (optional).
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

## 5. Building modules

### 5.1 `ModuleBuilder` — module level

| Method | Purpose |
|--------|---------|
| `new()` / `with_name(name)` / `set_module_name(name)` | create / name the module |
| `finish()` / `into_module()` | consume the builder, return the `Module` |
| `module()` | borrow the module under construction |
| `types()` | `&mut TypeStore` for interning composite types |
| `ptr_type(pointee)` / `array_type(elem, len)` / `struct_type(&[fields])` / `fn_type(params, variadic, result)` | intern common composite types |
| `intern_symbol(name)` | intern a name → `SymbolId` |
| `intern_constant_data(data)` | intern a `ConstantData` → `ConstantId` |
| `declare_function(name, linkage, abi, &[(param_name, ty)], result_ty)` | declare a function → `FunctionId` (no body yet; `""` param name = anonymous) |
| `declare_extern(name, abi, &[fixed_param_tys], variadic, result_ty)` | declare an external function (always `Linkage::External`) |
| `build_function(...)` | `declare_function` + `function_builder` in one step |
| `function_builder(id)` | body cursor for an already-declared function |

Types are interned: interning the same structure twice returns the same
`TypeId`. Type equality is `TypeId` equality. For address-space pointers
use `builder.types().ptr_in(pointee, space)`.

### 5.2 `FunctionBuilder` — function bodies

The builder holds a **cursor** on one current block:

* Instructions append to the current block.
* Terminators (`jump`, `branch`, `ret`, `unreachable`) complete the block.
  The cursor stays on the terminated block, so further appends fail with
  `A-BUILD-012 BlockTerminated` until you call `switch_to` or `append_block`.
* `append_block()` / `append_named_block(name)` / `append_block_with_params(&[(name, ty)])`
  append a block **and** move the cursor to it. The first block appended is
  the entry block.
* `param(i)` is the function parameter value; `block_param(block, i)` reads
  a block parameter value.

Naming rules:

* Value/block names must match `[A-Za-z_][A-Za-z0-9_]*`.
* Block labels of the form `bb<digits>` are reserved (the printer uses them
  for anonymous blocks).
* Block parameters may only be added while the block is still empty, never
  on the entry block (function parameters play that role).

### 5.3 Instruction catalog (builder methods)

| Category | Methods | Result type |
|----------|---------|-------------|
| Constants | `const_int(ty, i128)`¹, `const_uint(ty, u128)`¹, `const_f32(f32)`, `const_f64(f64)`, `const_null(pointee)` → `ptr<T>`, `const_string(&[u8])` → `ptr<i8>`, `const_aggregate(ty, &[ConstantId])` | the constant's type |
| Arithmetic | `add`, `sub`, `mul` (int: wrapping / float), `div` (int: truncating / float), `rem` (int only) | operand type |
| Bitwise | `bit_and`, `bit_or`, `bit_xor` (any int incl. `i1`), `shl`, `shr` (≥8-bit ints; `shr` is arithmetic for signed, logical for unsigned) | operand type |
| Comparison | `eq`, `ne` (int/float/ptr), `lt`, `le`, `gt`, `ge` (ints — signedness from the type — or floats) | `i1` |
| Memory | `alloca(pointee)` → `ptr<T>`, `load(ptr)` → `T`, `store(ptr, value)` → no result, `ptr_offset(ptr, int_offset)` → same ptr type | — |
| Conversion | `ext(to, v)` (must widen; sign-extends iff source is signed), `trunc(to, v)` (must narrow, wraps), `int_to_float(to, v)`, `float_to_int(to, v)` (saturating, NaN → 0), `ptr_cast(to, ptr)` (both sides pointer) | `to` |
| Calls | `call(callee: FunctionId, &[args])` → `Option<ValueId>` (`None` iff callee returns `void`) | callee result |
| Aggregates | `construct(ty, &[field_values])`, `extract(agg, index)`, `insert(agg, index, value)` (functional update) | field / aggregate type |
| Terminators | `jump(target, args)`, `branch(cond: i1, then, targs, else, eargs)`, `ret(Some(v))` / `ret(None)` / `ret_void()`, `unreachable()` | — |

¹ Values wrap (two's complement) to the type width; out-of-range literals
are masked, not rejected.

Debug metadata: `set_value_name(v, name)` attaches a name (never part of
semantic identity). `Span`s exist but only the parser fills them.

### 5.4 Control flow with block arguments (phi-free)

```rust
use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

let mut b = ModuleBuilder::new();
let pick = b.declare_function(
    "pick", Linkage::Exported, Abi::Astronomy,
    &[("cond", TypeId::I1), ("a", TypeId::I64), ("bb", TypeId::I64)],
    TypeId::I64,
)?;
let mut fb = b.function_builder(pick)?;
let (cond, a, b_) = (fb.param(0), fb.param(1), fb.param(2));

let entry = fb.append_block();
let left  = fb.append_block_with_params(&[("x", TypeId::I64)])?;
let right = fb.append_block_with_params(&[("x", TypeId::I64)])?;
let merge = fb.append_block_with_params(&[("result", TypeId::I64)])?;

fb.switch_to(entry)?;
fb.branch(cond, left, &[a], right, &[b_])?;   // args must match block params

let x = fb.block_param(left, 0)?;
fb.switch_to(left)?;
fb.jump(merge, &[x])?;

let x = fb.block_param(right, 0)?;
fb.switch_to(right)?;
fb.jump(merge, &[x])?;

let result = fb.block_param(merge, 0)?;
fb.switch_to(merge)?;
fb.ret(Some(result))?;

let verified = Verifier::verify(b.finish())?;   // dominance is checked here
```

### 5.5 Externs and calls (e.g. libc)

```rust
use astronomy::{Abi, ModuleBuilder, TypeId};

let mut b = ModuleBuilder::new();
let i8_ptr = b.ptr_type(TypeId::I8);
let printf = b.declare_extern("printf", Abi::C, &[i8_ptr], /* variadic */ true, TypeId::I32)?;

// Inside a function body:
// let s = fb.const_string(b"hello\n")?;
// let n = fb.call(printf, &[s])?;   // Option<ValueId>, i32 here
```

`Abi` is `Astronomy | C | System | Custom(SymbolId)`; `Linkage` is
`Internal | External | Exported`. Declarations must be `external`; variadic
is allowed only on declarations (definitions with bodies cannot be
variadic).

## 6. Verification

```rust
let verified = Verifier::verify(module)?;      // Module → VerifiedModule
// equivalent: let verified = module.verify()?;
// free function: astronomy::verify(module)
```

The verifier collects **all** errors, not just the first:

```rust
match module.verify() {
    Ok(verified) => { /* backends consume &verified */ }
    Err(report) => {
        eprintln!("{report}");                       // human-readable, all errors
        for e in report.errors() {                   // structured access
            eprintln!("error[{}]: {e}", e.code());   // stable code, e.g. A-VERIFY-040
        }
    }
}
```

`VerifyErrorReport` implements `std::error::Error`, `Display`, `len()`,
`is_empty()`, `iter()`, `into_errors()`, and `IntoIterator`.

What is checked: unique function names; constant-pool well-formedness;
linkage/variadic rules; first-class signatures; single SSA definition per
value; entry block without parameters; one terminator per block; operand/
result types per instruction; call arity and types; terminator targets and
branch-argument counts/types; return vs. result type; **dominance** (every
use dominated by its definition; uses in unreachable blocks are exempt).

The builder already rejects most mistakes eagerly, but the verifier is the
final authority — always verify before handing IR to a backend.

## 7. Text format (`.arn`)

```rust
// print (Module or VerifiedModule; VerifiedModule derefs to Module)
let text: String = verified.to_arn();              // or astronomy::text::print(&verified)

// parse
let module = Module::parse_arn(&text)?;            // or astronomy::text::parse(&text)
let verified = module.verify()?;                   // re-verify parsed input
```

Properties (normative in SPECIFICATION.md §13):

* **Canonical & deterministic**: the same module always prints byte-for-byte
  the same text (`print(print(parse(print(m))))` is stable), so it is safe
  for snapshots, golden tests, and cache keys.
* Roundtrip: `IR → print → parse → verify` succeeds; anonymous values are
  renumbered `%0, %1, …` and anonymous blocks `bb0, bb1, …` in emission
  order (blocks in reverse post-order, unreachable blocks last).
* `#` starts a line comment; the parser accepts comments and free spacing,
  the printer never emits them.
* Versioning: `::ASTRONOMY::MODULE_VERSION 1` (`ArnVersion::CURRENT` is
  `1.0`). Parsers reject unknown major versions with `A-ARN-001`.
* Floats: shortest-roundtrip form (`1.5e300`, `nan`, `inf`); NaN payloads
  do not survive text.
* Strings escape `\n \t \r \0 \\ \"` and non-printables as `\xNN`; no
  trailing NUL is stored.

## 8. Inspecting IR programmatically

`Module` (and `VerifiedModule` via `Deref`) exposes read access:

```rust
module.name();                       // Option<&str>
module.functions();                  // &[Function], declaration order
module.function_by_name("add");      // Option<FunctionId>
module.function_name(id);            // &str
module.types();                      // &TypeStore
module.type_name(ty);                // canonical string, e.g. "ptr<i8>"
module.symbol_name(sym);             // Option<&str>
module.constants();                  // &ConstantStore
```

`Function` offers `params`, `result`, `linkage`, `abi`, `variadic`,
`is_declaration()`, `entry()`, `block(id)`, `value(id)`; blocks expose
`params`, `instructions`, `terminator`. Values carry `ValueData { ty, kind,
name, span }`. Use `TypeData` (`Int { bits, signed }`, `Pointer { pointee,
address_space }`, `Array { element, length }`, `Struct { fields }`,
`Function { params, variadic, result }`, `Void`) plus predicates
`is_integer()`, `is_arith_integer()`, `is_float()`, `is_pointer()`,
`is_aggregate()`, `is_first_class()`, `is_signed_int()` for backend-type
mapping.

IDs returned by the builder are stable for the lifetime of that module;
indexes are dense, so `Vec`-based side tables keyed by `.index()` work.

## 9. Errors

All failures are structured enums with stable codes — never `String`
errors. Each variant has a `.code()` method and implements
`std::error::Error` + `Display` (`[CODE] message`).

| Prefix | Enum | Meaning |
|--------|------|---------|
| `A-BUILD-*` | `BuildError` (`.code()`: `A-BUILD-001`…`019`) | builder rejected invalid construction (unknown value/block/function, type mismatch, double terminator, invalid/reserved names, duplicate function, no current block, invalid signature, …) |
| `A-VERIFY-*` | `VerifyError` (collected in `VerifyErrorReport`; codes `A-VERIFY-001`…`073`) | verifier rejected invalid IR |
| `A-ARN-*` | `ParseError` (codes `A-ARN-001`…`021`; `.line()` gives the source line when known) | `.arn` text could not be parsed |

Match on variants (they carry IDs/names/reasons) to emit your own
diagnostics; use `.code()` for stable, greppable tags in logs and tests.

## 10. Semantics cheat sheet (normative in SPECIFICATION.md)

* Integer arithmetic (`add`/`sub`/`mul`) **wraps** (two's complement).
  Signed division truncates toward zero (`INT_MIN / -1` wraps); `rem` takes
  the dividend's sign. **Division/removal by zero is undefined behavior** —
  the verifier does not check it.
* Shift amounts: any integer type, read as unsigned; counts ≥ width give 0
  (logical) or the sign bit (arithmetic).
* Comparisons take the signedness of the operand type; mixing signedness
  requires explicit `ext`/`trunc`.
* `ext` widens and sign-extends iff the source type is signed;
  `trunc` wraps (drops high bits). Both directions are enforced
  (`ext` must widen, `trunc` must narrow).
* `float_to_int` truncates toward zero, saturates out-of-range, NaN → 0.
* `ptr_offset` is element-wise (`p + n * sizeof(pointee)`), offset signed,
  address arithmetic wraps. `load`/`store` require exactly `ptr<T>`;
  type punning needs an explicit `ptr_cast`.
* `i1` participates in bitwise ops; `shl`/`shr` require ≥8-bit operands.

## 11. Rules and gotchas

1. **Everything is resolved before Astronomy.** No header paths, no C
   macros, no move semantics, no liveness — state facts only.
2. **No indirect calls.** Function-pointer *types* are not expressible;
   model vtables in your frontend (e.g. function tables + `ptr_offset` +
   external trampolines) or extend the IR upstream.
3. **Definitions with bodies must not be variadic**; `declare_extern` is
   the only variadic path.
4. **Entry block = first `append_block*` call**, and it must remain
   parameterless.
5. Branch/jump argument lists must match the target block's parameters
   exactly (count and types) — the builder checks this at emission time.
6. After a terminator the cursor does not auto-advance; emitting into a
   completed block is `A-BUILD-012`, not silent misplacement.
7. `bb<digits>` block labels are reserved; digit-only value handles are
   reserved for anonymous numbering.
8. Constants in aggregates must reference **earlier** pool entries (no
   cycles, by construction).
9. IDs are module/function-scoped: a `TypeId` from one module means nothing
   in another. A fabricated ID is an error at verify time, never UB.
10. `.arn` is for debugging/exchange, not the compile hot path — prefer the
    builder API in pipelines.

## 12. Development CLI (not the product)

```bash
cargo run --quiet --example add          # build + verify + print .arn
cargo run -- --help
astronomy verify  hello.arn              # parse + verify
astronomy fmt    hello.arn               # parse + canonical print
astronomy inspect hello.arn              # module summary
```

Runnable library examples live in `examples/` (`add`, `cfg`,
`block_args`, `extern_decl`); acceptance/negative/roundtrip suites in
`tests/`.

## 13. Not in the MVP (by design)

`.arb` binary format, optimizer passes, global variables and metadata
(`GlobalId`/`MetadataId` are reserved), vector/opaque/union types,
indirect calls, aliasing/alignment metadata, and out-of-tree backends.
The core crate has zero dependencies and no backend-specific code.
