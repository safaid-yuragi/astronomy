//! Acceptance tests (§50–§53, §62): build IR through the Rust API,
//! verify it, serialize to `.arn`, re-parse and verify again.
//!
//! The definition of done requires:
//!
//! ```text
//! IR → print → ARN → parse → IR → verify → OK
//! ```

use astronomy::{
    Abi, BlockId, BuildError, Instruction, InstructionKind, Linkage, Module, ModuleBuilder,
    Terminator, TypeId, ValueData, ValueKind, Verifier, VerifiedModule,
};

/// Build → verify → print → parse → verify → print, asserting that the
/// canonical text is stable (§33 Roundtrip, §32 Canonical Printer).
fn assert_roundtrip(module: Module) -> VerifiedModule {
    let verified = Verifier::verify(module).expect("module must verify");
    let text1 = verified.to_arn();
    let reparsed = Module::parse_arn(&text1).expect("re-parsing canonical ARN must succeed");
    let verified2 = Verifier::verify(reparsed).expect("reparsed module must verify");
    let text2 = verified2.to_arn();
    assert_eq!(text1, text2, "canonical printing must be idempotent");
    verified
}

// ---------------------------------------------------------------------------
// §50 — add
// ---------------------------------------------------------------------------

fn build_add() -> Module {
    let mut b = ModuleBuilder::with_name("add-demo");
    let add = b
        .declare_function(
            "add",
            Linkage::Exported,
            Abi::Astronomy,
            &[("a", TypeId::I64), ("b", TypeId::I64)],
            TypeId::I64,
        )
        .unwrap();
    let mut fb = b.function_builder(add).unwrap();
    fb.append_block();
    let (a, b_) = (fb.param(0), fb.param(1));
    let sum = fb.add(a, b_).unwrap();
    fb.ret(Some(sum)).unwrap();
    b.finish()
}

#[test]
fn acceptance_add() {
    let verified = assert_roundtrip(build_add());
    let text = verified.to_arn();
    assert!(text.contains("::ASTRONOMY::ADD i64 i64 %a, i64 %b"), "{text}");
    assert!(text.contains("::ASTRONOMY::RETURN i64"), "{text}");
    assert_eq!(verified.functions().len(), 1);
}

// ---------------------------------------------------------------------------
// §51 — CFG (abs)
// ---------------------------------------------------------------------------

fn build_abs() -> Module {
    let mut b = ModuleBuilder::with_name("cfg-demo");
    let abs = b
        .declare_function(
            "abs",
            Linkage::Exported,
            Abi::Astronomy,
            &[("x", TypeId::I32)],
            TypeId::I32,
        )
        .unwrap();
    let mut fb = b.function_builder(abs).unwrap();
    let x = fb.param(0);
    let entry = fb.append_named_block("entry").unwrap();
    let negative = fb.append_named_block("negative").unwrap();
    let positive = fb.append_named_block("positive").unwrap();
    fb.switch_to(entry).unwrap();
    let zero = fb.const_int(TypeId::I32, 0).unwrap();
    let cond = fb.lt(x, zero).unwrap();
    fb.branch(cond, negative, &[], positive, &[]).unwrap();
    fb.switch_to(negative).unwrap();
    let negated = fb.sub(zero, x).unwrap();
    fb.ret(Some(negated)).unwrap();
    fb.switch_to(positive).unwrap();
    fb.ret(Some(x)).unwrap();
    b.finish()
}

#[test]
fn acceptance_cfg() {
    let verified = assert_roundtrip(build_abs());
    let f = &verified.functions()[0];
    assert_eq!(f.blocks.len(), 3);
    assert!(matches!(
        &f.blocks[0].terminator,
        Some(Terminator::Branch { .. })
    ));
    assert!(matches!(
        &f.blocks[1].terminator,
        Some(Terminator::Return { .. })
    ));
    let text = verified.to_arn();
    assert!(text.contains("::ASTRONOMY::BRANCH i1"), "{text}");
}

// ---------------------------------------------------------------------------
// §52 — block arguments
// ---------------------------------------------------------------------------

fn build_block_args() -> Module {
    let mut b = ModuleBuilder::with_name("block-args-demo");
    let pick = b
        .declare_function(
            "pick",
            Linkage::Exported,
            Abi::Astronomy,
            &[("cond", TypeId::I1), ("a", TypeId::I64), ("b", TypeId::I64)],
            TypeId::I64,
        )
        .unwrap();
    let mut fb = b.function_builder(pick).unwrap();
    let (cond, a, b_) = (fb.param(0), fb.param(1), fb.param(2));
    let entry = fb.append_named_block("entry").unwrap();
    let left = fb.append_block_with_params(&[("x", TypeId::I64)]).unwrap();
    let right = fb.append_block_with_params(&[("x", TypeId::I64)]).unwrap();
    let merge = fb.append_block_with_params(&[("result", TypeId::I64)]).unwrap();
    fb.switch_to(entry).unwrap();
    fb.branch(cond, left, &[a], right, &[b_]).unwrap();
    let x_left = fb.block_param(left, 0).unwrap();
    fb.switch_to(left).unwrap();
    fb.jump(merge, &[x_left]).unwrap();
    let x_right = fb.block_param(right, 0).unwrap();
    fb.switch_to(right).unwrap();
    fb.jump(merge, &[x_right]).unwrap();
    let result = fb.block_param(merge, 0).unwrap();
    fb.switch_to(merge).unwrap();
    fb.ret(Some(result)).unwrap();
    b.finish()
}

#[test]
fn acceptance_block_arguments() {
    let verified = assert_roundtrip(build_block_args());
    let f = &verified.functions()[0];
    assert_eq!(f.blocks[2].params.len(), 1, "merge block has one argument");
    // Entry branches; both arms jump into merge passing one argument.
    match &f.blocks[0].terminator {
        Some(Terminator::Branch { then_args, else_args, .. }) => {
            assert_eq!(then_args.len(), 1);
            assert_eq!(else_args.len(), 1);
        }
        other => panic!("unexpected terminator: {other:?}"),
    }
    for b in &f.blocks[1..3] {
        assert!(
            matches!(&b.terminator, Some(Terminator::Jump { args, .. }) if args.len() == 1),
            "expected jump with one argument, got {:?}",
            b.terminator
        );
    }
}

// ---------------------------------------------------------------------------
// §19/§62 — extern declarations and calls
// ---------------------------------------------------------------------------

fn build_extern() -> Module {
    let mut b = ModuleBuilder::with_name("extern-demo");
    let i8_ptr = b.ptr_type(TypeId::I8);
    let printf = b
        .declare_extern("printf", Abi::C, &[i8_ptr], true, TypeId::I32)
        .unwrap();
    let hello = b
        .build_function(
            "hello",
            Linkage::Exported,
            Abi::Astronomy,
            &[("n", TypeId::I32)],
            TypeId::VOID,
        )
        .unwrap();
    let mut fb = hello;
    fb.append_block();
    let n = fb.param(0);
    let fmt = fb.const_string(b"n = %d\n").unwrap();
    let printed = fb.call(printf, &[fmt, n]).unwrap();
    assert!(printed.is_some(), "printf returns i32");
    fb.ret_void().unwrap();
    drop(fb);
    b.finish()
}

#[test]
fn acceptance_extern_declaration() {
    let verified = assert_roundtrip(build_extern());
    let printf = verified.function_by_name("printf").unwrap();
    let f = verified.function(printf).unwrap();
    assert!(f.is_declaration());
    assert!(f.variadic);
    assert_eq!(f.abi, Abi::C);
    assert_eq!(f.linkage, Linkage::External);
    let text = verified.to_arn();
    assert!(
        text.contains("::ASTRONOMY::FUNCTION_START printf fn(ptr<i8>, ...) -> i32 linkage=external abi=c"),
        "{text}"
    );
}

// ---------------------------------------------------------------------------
// Broader instruction coverage: memory, aggregates, conversions, floats
// ---------------------------------------------------------------------------

fn build_kitchen_sink() -> Module {
    let mut b = ModuleBuilder::with_name("kitchen-sink");

    let pair_ty = b.struct_type(&[TypeId::I64, TypeId::I64]);
    let byte_ptr_ty = b.ptr_type(TypeId::I8);

    let f = b
        .build_function(
            "kitchen_sink",
            Linkage::Internal,
            Abi::Astronomy,
            &[("a", TypeId::I64), ("b", TypeId::I64)],
            TypeId::I64,
        )
        .unwrap();
    let mut fb = f;
    fb.append_block();
    let (a, b_) = (fb.param(0), fb.param(1));

    // Memory: alloca / store / load / ptr_offset.
    let slot = fb.alloca(TypeId::I64).unwrap();
    fb.store(slot, a).unwrap();
    let loaded = fb.load(slot).unwrap();
    let one = fb.const_int(TypeId::I64, 1).unwrap();
    let slot2 = fb.ptr_offset(slot, one).unwrap();
    fb.store(slot2, b_).unwrap();
    let second = fb.load(slot2).unwrap();

    // Aggregates: construct / extract / insert.
    let pair = fb.construct(pair_ty, &[loaded, second]).unwrap();
    let x = fb.extract(pair, 1).unwrap();
    let pair2 = fb.insert(pair, 0, x).unwrap();
    let y = fb.extract(pair2, 0).unwrap();

    // Conversions and float arithmetic.
    let xf = fb.int_to_float(TypeId::F64, x).unwrap();
    const HALF: f64 = 0.5;
    let half = fb.const_f64(HALF).unwrap();
    let scaled = fb.mul(xf, half).unwrap();
    let _rounded = fb.float_to_int(TypeId::I64, scaled).unwrap();
    let extended = fb.ext(TypeId::I128, y).unwrap();
    let narrowed = fb.trunc(TypeId::I32, extended).unwrap();
    let widened = fb.ext(TypeId::I64, narrowed).unwrap();

    // Bitwise and shifts.
    let mask = fb.const_int(TypeId::I64, 0xff).unwrap();
    let masked = fb.bit_and(widened, mask).unwrap();
    let shift_by = fb.const_int(TypeId::I64, 1).unwrap();
    let shifted = fb.shl(masked, shift_by).unwrap();

    // Null pointer + pointer cast.
    let null = fb.const_null(TypeId::I64).unwrap();
    let _as_bytes = fb.ptr_cast(byte_ptr_ty, slot).unwrap();
    let is_null = fb.eq(slot, null).unwrap();
    let any_null = fb.bit_and(is_null, is_null).unwrap();
    let any_null_wide = fb.ext(TypeId::I64, any_null).unwrap();
    let chosen = fb.bit_or(shifted, any_null_wide).unwrap();
    fb.ret(Some(chosen)).unwrap();
    drop(fb);
    b.finish()
}

#[test]
fn acceptance_kitchen_sink() {
    let verified = assert_roundtrip(build_kitchen_sink());
    let text = verified.to_arn();
    for op in [
        "::ASTRONOMY::ALLOCA",
        "::ASTRONOMY::STORE",
        "::ASTRONOMY::LOAD",
        "::ASTRONOMY::PTR_OFFSET",
        "::ASTRONOMY::CONSTRUCT",
        "::ASTRONOMY::EXTRACT",
        "::ASTRONOMY::INSERT",
        "::ASTRONOMY::INT_TO_FLOAT",
        "::ASTRONOMY::FLOAT_TO_INT",
        "::ASTRONOMY::EXT",
        "::ASTRONOMY::TRUNC",
        "::ASTRONOMY::AND",
        "::ASTRONOMY::OR",
        "::ASTRONOMY::SHL",
        "::ASTRONOMY::CONSTANT_NULL",
        "::ASTRONOMY::PTR_CAST",
    ] {
        assert!(text.contains(op), "missing {op} in:\n{text}");
    }
}

// ---------------------------------------------------------------------------
// Constant pool roundtrip fidelity
// ---------------------------------------------------------------------------

#[test]
fn constants_survive_roundtrip_exactly() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function("f", Linkage::Internal, Abi::Astronomy, &[], TypeId::F64)
        .unwrap();
    let mut fb = f;
    fb.append_block();
    let _ = fb.const_int(TypeId::I64, -9223372036854775808).unwrap();
    let _ = fb.const_uint(TypeId::U64, 18446744073709551615).unwrap();
    let _ = fb.const_f64(-1.5e300).unwrap();
    let _ = fb.const_f32(3.5).unwrap();
    let _ = fb.const_string(b"esc\"aped\n\tbytes\x00\xff").unwrap();
    let zero = fb.const_f64(0.0).unwrap();
    fb.ret(Some(zero)).unwrap();
    drop(fb);
    let module = b.finish();

    let verified = assert_roundtrip(module);
    let text = verified.to_arn();
    assert!(text.contains("::ASTRONOMY::CONSTANT_INT"), "{text}");
    assert!(text.contains("-9223372036854775808"), "{text}");
    assert!(text.contains("18446744073709551615"), "{text}");
    assert!(text.contains("18446744073709551615"), "{text}");
    assert!(text.contains("\\xFF"), "{text}");
}

// ---------------------------------------------------------------------------
// Direct (low-level) construction (§30)
// ---------------------------------------------------------------------------

#[test]
fn direct_construction_verifies() {
    let mut module = Module::new();
    let sym = module.symbols_mut().intern("id");
    let mut f = astronomy::Function::new(
        sym,
        Linkage::Internal,
        Abi::Astronomy,
        vec![(None, TypeId::I64)],
        TypeId::I64,
        false,
    );
    let x = f.params[0].value;
    let result = astronomy::id::ValueId::new(1);
    f.values.push(ValueData::new(
        TypeId::I64,
        ValueKind::Inst {
            block: BlockId::new(0),
            index: 0,
        },
    ));
    let mut bb = astronomy::BasicBlock::new();
    bb.instructions
        .push(Instruction::new(InstructionKind::Add { lhs: x, rhs: x }, Some(result)));
    bb.terminator = Some(Terminator::Return { value: Some(result) });
    f.blocks.push(bb);
    module.functions_mut().push(f);
    assert_roundtrip(module);
}

// ---------------------------------------------------------------------------
// Builder guards (§29: the builder makes invalid IR hard to build)
// ---------------------------------------------------------------------------

#[test]
fn builder_rejects_type_mismatch() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function(
            "f",
            Linkage::Internal,
            Abi::Astronomy,
            &[("a", TypeId::I64), ("c", TypeId::I32)],
            TypeId::I64,
        )
        .unwrap();
    let mut fb = f;
    fb.append_block();
    let err = fb.add(fb.param(0), fb.param(1)).unwrap_err();
    assert_eq!(err.code(), "A-BUILD-010");
}

#[test]
fn builder_rejects_double_terminator() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function("f", Linkage::Internal, Abi::Astronomy, &[("a", TypeId::I64)], TypeId::I64)
        .unwrap();
    let mut fb = f;
    fb.append_block();
    fb.ret(Some(fb.param(0))).unwrap();
    let err = fb.ret(Some(fb.param(0))).unwrap_err();
    assert_eq!(err.code(), "A-BUILD-012");
}

#[test]
fn builder_rejects_unknown_value() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function("f", Linkage::Internal, Abi::Astronomy, &[("a", TypeId::I64)], TypeId::I64)
        .unwrap();
    let mut fb = f;
    fb.append_block();
    let err = fb
        .add(fb.param(0), astronomy::id::ValueId::new(99))
        .unwrap_err();
    assert_eq!(err.code(), "A-BUILD-001");
}

#[test]
fn builder_rejects_duplicate_function() {
    let mut b = ModuleBuilder::new();
    b.declare_function("f", Linkage::Internal, Abi::Astronomy, &[], TypeId::VOID)
        .unwrap();
    let err = b
        .declare_function("f", Linkage::Internal, Abi::Astronomy, &[], TypeId::VOID)
        .unwrap_err();
    assert_eq!(err.code(), "A-BUILD-017");
}

#[test]
fn builder_rejects_entry_block_params() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function("f", Linkage::Internal, Abi::Astronomy, &[("a", TypeId::I64)], TypeId::I64)
        .unwrap();
    let mut fb = f;
    let entry = fb.append_block();
    let err = fb.add_block_param(entry, TypeId::I64).unwrap_err();
    assert_eq!(err.code(), "A-BUILD-013");
}

#[test]
fn builder_rejects_reserved_block_name() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function("f", Linkage::Internal, Abi::Astronomy, &[("a", TypeId::I64)], TypeId::I64)
        .unwrap();
    let mut fb = f;
    let err = fb.append_named_block("bb0").unwrap_err();
    assert_eq!(err.code(), "A-BUILD-014");
}

#[test]
fn builder_rejects_branch_arg_mismatch() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function(
            "f",
            Linkage::Internal,
            Abi::Astronomy,
            &[("c", TypeId::I1), ("x", TypeId::I64)],
            TypeId::I64,
        )
        .unwrap();
    let mut fb = f;
    let entry = fb.append_block();
    let merge = fb.append_block_with_params(&[("", TypeId::I32)]).unwrap();
    fb.switch_to(entry).unwrap();
    let err: BuildError = fb
        .branch(fb.param(0), merge, &[fb.param(1)], merge, &[])
        .unwrap_err();
    assert_eq!(err.code(), "A-BUILD-010");
}

#[test]
fn builder_rejects_store_type_mismatch() {
    let mut b = ModuleBuilder::new();
    let f = b
        .build_function("f", Linkage::Internal, Abi::Astronomy, &[("a", TypeId::I64)], TypeId::I64)
        .unwrap();
    let mut fb = f;
    fb.append_block();
    let slot = fb.alloca(TypeId::I64).unwrap();
    let one = fb.const_int(TypeId::I32, 1).unwrap();
    let err = fb.store(slot, one).unwrap_err();
    assert_eq!(err.code(), "A-BUILD-010");
}
