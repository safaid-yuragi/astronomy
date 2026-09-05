# Astronomy IR Specification

Version 1.0 — normative for the `astronomy` crate MVP.

Astronomy is a language-agnostic intermediate representation library.
Frontends lower *resolved* programs into Astronomy; backends consume only
*verified* Astronomy. This document defines the core IR model, its
semantics, the verification rules and the `.arn` textual format.

---

## 1. Philosophy

> **Astronomy Core IR contains facts, not questions.**

By the time control reaches Astronomy, the following are already resolved
by the frontend:

* types of every expression,
* symbol identity and function signatures,
* call targets (direct, by `FunctionId`),
* ABI / calling convention of every function,
* branch destinations,
* operand identities and result types.

Astronomy never performs name resolution, type inference, overload
resolution or header inclusion. Concepts belonging to a source language
(move semantics, borrows, `Result<T>`, liveness) or to a backend
(`#include`, registers, instruction selection) do not exist in the core.

Design checklist used for every core decision:

1. Is the concept needed by more than one language?
2. Is it needed by more than one backend?
3. Does the core have to *infer* anything?
4. Can the fact be stated explicitly (types, targets)?
5. Can it be an interned ID instead of a string?
6. Does it lower to plain SSA/CFG?
7. Can the verifier check it mechanically?
8. Can a Rust frontend build it quickly through the API?

## 2. Core IR model

A **module** owns four interned tables plus functions:

| Table          | Contents                        | Handle          |
|----------------|---------------------------------|-----------------|
| Type store     | structural types                | `TypeId`        |
| Symbol store   | interned names                  | `SymbolId`      |
| Constant pool  | interned constants              | `ConstantId`    |
| Functions      | definitions and extern decls    | `FunctionId`    |

A **function** owns:

* symbol, linkage, ABI, variadic flag,
* parameter list and result type,
* a per-function **value arena** (`ValueId` indexes into it),
* a list of **basic blocks** (`BlockId` indexes into it); block 0 is the
  entry block.

A **basic block** owns block parameters, a straight-line instruction list,
and exactly one terminator. Instructions never contain control flow;
terminators never compute.

All storage is contiguous (`Vec`) and all references are dense numeric IDs.
IDs are untrusted until the verifier accepts the module: a fabricated ID is
an error, never undefined behavior.

### 2.1 Pipeline

```text
Frontend → ModuleBuilder (Rust API) → Module → Verifier → VerifiedModule → Backend
                      ▲                                            │
                      └──── .arn parser ← .arn printer ←───────────┘
                            (debug / snapshots / exchange only)
```

`.arn` text is never on the hot path of compilation.

## 3. Types

```text
void                     the empty type; only as a function result
i1                       boolean; unsigned; comparisons produce it
i8 i16 i32 i64 i128      signed integers
u8 u16 u32 u64 u128      unsigned integers
f32 f64                  IEEE 754 binary32 / binary64
ptr<T>                   pointer to T (address space 0)
ptr<T, N>                pointer in address space N
array<T, N>              N elements of T
struct<T1, ..., Tn>      anonymous structural struct
fn(T1, ..., Tn, ...) -> R   function signature type
```

* All types are interned; type equality is `TypeId` equality. The scalar
  types are pre-interned in every module and available as `TypeId::I64`
  etc.
* **First-class types** (legal for SSA values and memory): integers,
  floats, pointers, aggregates. `void` and `fn` signatures are not
  first-class in the MVP.
* Pointee types must be first-class. Function pointers are therefore not
  expressible in the MVP (calls are direct by `FunctionId`).
* Future extensions (vector, opaque, union) add new `TypeData` variants
  without changing existing IDs.

## 4. Values and SSA

* Every SSA value is defined **exactly once**, by a function parameter, a
  block parameter, or an instruction result.
* Values are immutable; memory mutation is expressed with `load`/`store`.
* Debug names (`Option<SymbolId>`) and source spans (`Option<Span>`, from
  the parser) are metadata — never part of semantic identity.

Block arguments replace phi nodes (§6).

## 5. Functions, linkage, ABI, extern

```rust
Linkage = internal | external | exported
Abi     = astronomy | c | system | custom(SymbolId)
```

* A function **with** blocks is a definition; a function **without** blocks
  is an extern declaration.
* Declarations must use `external` linkage; definitions may use any
  linkage. Variadic is allowed only on declarations.
* Parameters and block parameters must be first-class; the result type must
  be first-class or `void`.
* Backends must not guess calling conventions — the ABI is an explicit
  fact on every function.
* Header paths, system headers and C macros are **not** core concepts; a C
  backend maps `extern … abi=c` to platform includes itself.

Example: the *concept* `printf` is

```arn
::ASTRONOMY::FUNCTION_START printf fn(ptr<i8>, ...) -> i32 linkage=external abi=c
::ASTRONOMY::FUNCTION_END
```

## 6. Control flow: blocks and terminators

```text
Jump      target, args            unconditional
Branch    cond, then, targs, else, eargs   cond : i1
Return    value?                  value present ⇔ result type ≠ void
Unreachable                       must not be reached at runtime
```

* Every block has exactly one terminator; there are no instructions after
  it (structurally impossible: instructions and terminator are separate
  fields).
* The **entry block** (block 0) must have no parameters; function
  parameters serve that role.
* Branch argument count and types must match the target block's parameters
  exactly.
* Phi nodes do not exist; merging values is expressed by passing arguments
  to the target block:

```text
entry:   branch %cond, left(%a), right(%b)
left(%x: i64):   jump merge(%x)
right(%x: i64):  jump merge(%x)
merge(%r: i64):  return %r
```

* Uses must be dominated by definitions (function parameters dominate the
  whole function; block parameters dominate their block and everything it
  dominates). Uses inside unreachable blocks are exempt from dominance
  checking because such blocks are dead and have no meaningful ordering.

## 7. Instructions

Results are written `%v = op …`; `store` and calls to `void` functions
produce no result.

### Constants
```text
const cN            materialize a pool constant
```

### Arithmetic
```text
add, sub, mul       integers (wrapping) or floats
div                 integers (truncating) or floats (IEEE)
rem                 integers only
```

### Bitwise (integers, any width incl. i1)
```text
and, or, xor
shl, shr            shr is arithmetic for signed operands, logical otherwise
```

### Comparison → `i1`
```text
eq, ne              integers, floats, pointers
lt, le, gt, ge      integers (signedness per type) or floats
```

### Memory
```text
alloca T            → ptr<T>          stack slot
load  ptr<T>        → T
store ptr<T>, T     → void
ptr_offset ptr<T>, iN → ptr<T>        element-wise; offset interpreted signed
```

### Conversion
```text
ext    iN → iM      widen; sign-extend iff source type is signed
trunc  iN → iM      narrow; wrap (drop high bits)
int_to_float  iN → fM   round to nearest, ties to even
float_to_int  fM → iN   truncate toward zero; NaN → 0; out-of-range saturates
ptr_cast ptr<A> → ptr<B>  address-preserving pointer cast
```

### Functions
```text
call @f(args…)      direct call; result present iff callee result ≠ void
```

### Aggregates
```text
construct T(f1..fn) → T
extract  a, i       → field type
insert   a, i, v    → T (functional update)
```

## 8. Integer semantics (normative)

* Widths are exact: 1, 8, 16, 32, 64, 128 bits. `i1` holds 0 or 1.
* **Overflow** on `add`, `sub`, `mul` wraps (two's complement). The MVP has
  one arithmetic semantics; future extensions may add `add.checked`,
  `add.saturating`, … as distinct operations.
* **Division** `a / b` on signed types truncates toward zero;
  `INT_MIN / -1` wraps. **Unsigned** division is modulo 2^N.
* **Remainder** `a % b` takes the sign of the dividend; `rem` by zero is
  undefined behavior (backends may trap or poison; the verifier does not
  require constant divisors).
* **Division by zero** is undefined behavior.
* **Shifts**: the shift amount is any integer type and is interpreted as an
  unsigned count; counts ≥ width produce 0 (logical) / the sign bit
  (arithmetic). `shl`/`shr` require ≥ 8-bit operands.
* **Comparisons** use the signedness of the operand type; mixing
  signedness requires explicit conversion.
* **Conversions** (`ext`, `trunc`) are total; `trunc` wraps. Constants are
  stored as two's-complement bit patterns masked to the type width.

## 9. Pointer semantics (normative)

* A pointer carries its pointee type and an address space (0 = default).
* `null` is a distinct constant per pointee type; loading from null is
  undefined behavior.
* `ptr_offset p, n` computes `p + n * sizeof(pointee)` with the offset
  interpreted as a **signed** integer; address arithmetic wraps. Pointer
  arithmetic is never language-dependent (no C array-decay rules, no
  indexing sugar).
* `load`/`store` require an operand of exactly `ptr<T>` matching the
  accessed type; type punning requires an explicit `ptr_cast`.
* `alloca` yields a pointer to a stack slot; its lifetime is the function
  invocation (a backend may lower it to a frame slot or promote it).
* Alignment, aliasing and dereferenceability guarantees are not modeled in
  the MVP; they are future metadata.

## 10. Float semantics

IEEE 754 binary32/binary64. Arithmetic follows the platform's IEEE
conformant operations. NaN payloads are **not** preserved through `.arn`
text (the binary format will preserve them). `float_to_int` saturates like
Rust's `as` casts; NaN converts to 0.

## 11. Constants

The module constant pool interns:

| Kind       | Value stored                              | Result type      |
|------------|-------------------------------------------|------------------|
| integer    | two's-complement bits, masked to width    | the integer type |
| float      | IEEE bit pattern                          | f32 / f64        |
| null       | —                                         | `ptr<T>`         |
| string     | bytes (no trailing NUL stored)            | `ptr<i8>`        |
| aggregate  | element constant IDs                      | array/struct     |

Aggregate elements must reference **earlier** pool entries (the pool is
append-only), which makes cyclic constants impossible.

## 12. Verification rules

`Verifier::verify(Module) -> Result<VerifiedModule, VerifyErrorReport>`
checks, per module:

* function symbol names are unique;
* every pool constant is well-formed (widths, field counts, element types,
  no forward references).

Per function:

* declarations: external linkage, variadic allowed; definitions: not
  variadic;
* parameters/result types are first-class (or `void`);
* **SSA**: every value defined exactly once; arena `ValueKind` agrees with
  the actual definition site; no value left `Reserved`;
* entry block has no parameters;
* every block has a terminator;
* per instruction: operands exist, operand types are valid for the opcode,
  result type equals the computed type, result presence is correct;
* calls: callee exists, arity (fixed/variadic) and argument types match,
  result matches the callee's result type;
* terminators: targets exist, argument counts/types match block parameters,
  branch condition is `i1`, return matches the function result;
* **dominance**: every use is dominated by its definition.

The verifier collects *all* errors; each carries a stable code
(`A-VERIFY-001` …). Invalid IR is rejected, never repaired.

## 13. ARN textual format

`.arn` is explicit and machine-checkable; every operand carries its type,
every structure is bracketed by markers. Lines are directives; the grammar
is newline-agnostic (structure is determined by markers), and `#` starts a
line comment.

### 13.1 Grammar (informal)

```text
module      ::= MODULE_START header* constants? function* MODULE_END
header      ::= MODULE_NAME string | MODULE_VERSION int
constants   ::= CONSTANTS_START constant* CONSTANTS_END
constant    ::= CONSTANT_INT    cref type int
             | CONSTANT_FLOAT  cref (f32|f64) float
             | CONSTANT_NULL   cref ptr-type
             | CONSTANT_STRING cref string
             | CONSTANT_AGGREGATE cref type cref*
function    ::= FUNCTION_START ident fntype attrs BLOCK_START block* BLOCK_END FUNCTION_END
             | FUNCTION_START ident fntype attrs FUNCTION_END      (declaration)
fntype      ::= fn lparen [param (, param)*] [, ...] rparen -> type
param       ::= type [%handle]
attrs       ::= [linkage = (internal|external|exported)] [abi = (astronomy|c|system|custom ident)]
block       ::= BLOCK_START label [( param-decl (, param-decl)* )] stmt* BLOCK_END
param-decl  ::= type %handle
stmt        ::= [%handle =] OPCODE operands…      (see below)
terminator  ::= JUMP label [(args)] 
             | BRANCH ty %v , label [(args)] , label [(args)]
             | RETURN type %v | RETURN void
             | UNREACHABLE
```

### 13.2 Instruction syntax

Result-producing instructions are `%handle = ::ASTRONOMY::OP <result-type>
<operands>` where each operand is `<type> %handle`:

```arn
%2 = ::ASTRONOMY::CONST i64 c0
%3 = ::ASTRONOMY::ADD i64 i64 %a, i64 %b
%4 = ::ASTRONOMY::LT i1 i32 %x, i32 %2
%5 = ::ASTRONOMY::ALLOCA ptr<i64> i64
%6 = ::ASTRONOMY::LOAD i64 ptr<i64> %5
::ASTRONOMY::STORE ptr<i64> %5, i64 %6
%7 = ::ASTRONOMY::PTR_OFFSET ptr<i64> ptr<i64> %5, i64 %2
%8 = ::ASTRONOMY::EXT i64 i32 %x
%9 = ::ASTRONOMY::TRUNC i32 i64 %x
%a = ::ASTRONOMY::INT_TO_FLOAT f64 i64 %x
%b = ::ASTRONOMY::FLOAT_TO_INT i64 f64 %a
%c = ::ASTRONOMY::PTR_CAST ptr<i8> ptr<i64> %5
%d = ::ASTRONOMY::CALL i32 @printf(ptr<i8> %s, i32 %x)
::ASTRONOMY::CALL void @notify(i32 %x)
%e = ::ASTRONOMY::CONSTRUCT struct<i32, i64> (i32 %x, i64 %y)
%f = ::ASTRONOMY::EXTRACT i32 struct<i32, i64> %e, 0
%g = ::ASTRONOMY::INSERT struct<i32, i64> struct<i32, i64> %e, 1, i64 %y
```

### 13.3 Canonical printing (§32)

* Directives are emitted in a fixed order; one directive per line; no
  comments; two-space-free uniform spacing.
* Blocks are emitted in reverse post-order (dominators first), unreachable
  blocks appended in index order. The entry block is always first.
* Anonymous values are numbered by emission order (`%0, %1, …`); named
  values keep their names, except duplicated names which fall back to
  anonymous numbering. Value names are identifiers; digit-only handles are
  reserved for anonymous numbering.
* Anonymous blocks are labeled `bb0, bb1, …` in emission order; labels of
  that form are reserved and cannot be user-assigned.
* Constants are printed in pool index order (`c0, c1, …`).
* Floats print in shortest-roundtrip exponent form (`1.5e300`, `5e-1`),
  with `nan`, `inf`, `-inf` special forms.
* Strings escape `\n \t \r \0 \\ \"` and non-printable bytes as `\xNN`.

The same module always prints to the same text; therefore

```text
IR → print → ARN → parse → IR → verify → OK, and print(print(parse(print(m)))) is stable
```

### 13.4 Versioning (§47)

`::ASTRONOMY::MODULE_VERSION 1` — the major version. Parsers reject
unknown major versions (`A-ARN-001`). Minor versions are additive.

## 14. Error codes

| Code        | Meaning                                     |
|-------------|---------------------------------------------|
| A-BUILD-001…| builder rejected invalid construction       |
| A-VERIFY-001…| verifier rejected invalid IR               |
| A-ARN-001…  | `.arn` parse failure (with source line)     |

Errors are data-carrying enums (`BuildError`, `VerifyError`,
`ParseError`), never strings. The verifier returns a `VerifyErrorReport`
listing every problem found.

## 15. Determinism and performance

* No `HashMap` iteration ever reaches output; all printing orders are
  derived from arena indices or RPO.
* Interning (`types`, `symbols`, `constants`) keeps hot structures small
  and `Copy`.
* Instructions live in `Vec`s; IDs are indices; no `Rc`/`Arc`/`Box` per
  instruction; the MVP is 100% safe Rust (no `unsafe`).
* Functions are independent (per-function arenas), enabling parallel
  verify/optimize/codegen by the *consumer* — the library itself spawns no
  threads.

## 16. Binary format (future, non-normative)

`.arb` will mirror the in-memory layout for fast load with minimal
allocation:

```text
Header | TypeTable | SymbolTable | ConstantTable | GlobalTable
      | FunctionTable | BlockTable | InstructionTable
      | MetadataTable | StringTable
```

The MVP's numeric IDs, interned tables and contiguous storage are chosen
specifically so this is a serialization exercise, not a redesign. An
mmap-able layout may follow; it is explicitly out of scope today.

## 17. Non-goals (MVP)

Frontends, backends, linkers, register allocation, instruction selection,
dialect systems, production optimizers, debug info, JIT, GC. The core
library comes first; everything above builds on `VerifiedModule` from
external crates.
