//! Negative tests (§53): the verifier must reject invalid IR, and the
//! parser must reject invalid text — without panicking.
//!
//! Invalid modules are crafted through the low-level construction API
//! (§30 Direct Construction) because the builder already prevents most of
//! these mistakes.

use astronomy::{
    text, Abi, BasicBlock, BlockId, BlockParam, Function, Instruction, InstructionKind, Linkage,
    Module, Terminator, TypeId, ValueData, ValueKind, Verifier,
};

/// Creates a module containing one crafted function `f`.
fn module_with(f: Function) -> Module {
    let mut module = Module::new();
    module.functions_mut().push(f);
    module
}

fn named_function(module: &mut Module, params: &[TypeId], result: TypeId) -> Function {
    let sym = module.symbols_mut().intern("f");
    Function::new(
        sym,
        Linkage::Exported,
        Abi::Astronomy,
        params.iter().map(|&t| (None, t)).collect(),
        result,
        false,
    )
}

/// Allocates a fresh instruction-result value in `f` for `block`.
fn fresh_result(f: &mut Function, block: BlockId, ty: TypeId) -> astronomy::ValueId {
    let index = f.blocks[block.index()].instructions.len() as u32;
    let v = astronomy::ValueId::new(f.values.len() as u32);
    f.values.push(ValueData::new(
        ty,
        ValueKind::Inst { block, index },
    ));
    v
}

fn verify_codes(module: Module) -> Vec<String> {
    match Verifier::verify(module) {
        Ok(_) => panic!("verification unexpectedly succeeded"),
        Err(errors) => errors.iter().map(|e| e.code().to_string()).collect(),
    }
}

// ---------------------------------------------------------------------------
// §53 — type mismatch: `%0:i64 = add %a:i64, %b:i32`
// ---------------------------------------------------------------------------

#[test]
fn reject_add_operand_type_mismatch() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I64, TypeId::I32], TypeId::I64);
    let entry = BlockId::new(0);
    f.blocks.push(BasicBlock::new());
    let sum = fresh_result(&mut f, entry, TypeId::I64);
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: f.params[0].value,
            rhs: f.params[1].value,
        },
        Some(sum),
    ));
    f.blocks[0].terminator = Some(Terminator::Return { value: Some(sum) });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-010".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// §53 — wrong result type
// ---------------------------------------------------------------------------

#[test]
fn reject_wrong_result_type() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I64], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    let sum = fresh_result(&mut f, BlockId::new(0), TypeId::I32); // wrong
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: f.params[0].value,
            rhs: f.params[0].value,
        },
        Some(sum),
    ));
    f.blocks[0].terminator = Some(Terminator::Return { value: Some(sum) });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-012".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// §53 — branch argument mismatch (count and type)
// ---------------------------------------------------------------------------

#[test]
fn reject_branch_argument_count_mismatch() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I1], TypeId::VOID);
    f.blocks.push(BasicBlock::new());
    let merge = BlockId::new(1);
    f.blocks.push(BasicBlock::new());
    // merge takes one i64 parameter...
    let param = astronomy::ValueId::new(f.values.len() as u32);
    f.values.push(ValueData::new(
        TypeId::I64,
        ValueKind::BlockParam {
            block: merge,
            index: 0,
        },
    ));
    f.blocks[1].params.push(BlockParam {
        value: param,
        ty: TypeId::I64,
    });
    f.blocks[1].terminator = Some(Terminator::Return { value: None });
    // ...but the branch passes zero arguments.
    f.blocks[0].terminator = Some(Terminator::Branch {
        condition: f.params[0].value,
        then_block: merge,
        then_args: vec![],
        else_block: merge,
        else_args: vec![],
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-040".to_string()), "{codes:?}");
}

#[test]
fn reject_branch_argument_type_mismatch() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I1, TypeId::I32], TypeId::VOID);
    f.blocks.push(BasicBlock::new());
    let merge = BlockId::new(1);
    f.blocks.push(BasicBlock::new());
    let param = astronomy::ValueId::new(f.values.len() as u32);
    f.values.push(ValueData::new(
        TypeId::I64,
        ValueKind::BlockParam {
            block: merge,
            index: 0,
        },
    ));
    f.blocks[1].params.push(BlockParam {
        value: param,
        ty: TypeId::I64,
    });
    f.blocks[1].terminator = Some(Terminator::Return { value: None });
    // jump merge(%x:i32) but merge expects i64.
    f.blocks[0].terminator = Some(Terminator::Branch {
        condition: f.params[0].value,
        then_block: merge,
        then_args: vec![f.params[1].value],
        else_block: merge,
        else_args: vec![f.params[1].value],
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-041".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// §53 — return type mismatch
// ---------------------------------------------------------------------------

#[test]
fn reject_return_type_mismatch() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I32], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    f.blocks[0].terminator = Some(Terminator::Return {
        value: Some(f.params[0].value), // i32 in an i64 function
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-050".to_string()), "{codes:?}");
}

#[test]
fn reject_missing_return_value() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    f.blocks[0].terminator = Some(Terminator::Return { value: None });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-050".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// §53 — nonexistent value
// ---------------------------------------------------------------------------

#[test]
fn reject_unknown_value_operand() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I64], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    let sum = fresh_result(&mut f, BlockId::new(0), TypeId::I64);
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: astronomy::ValueId::new(99), // does not exist
            rhs: f.params[0].value,
        },
        Some(sum),
    ));
    f.blocks[0].terminator = Some(Terminator::Return { value: Some(sum) });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-001".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// §53 — nonexistent block
// ---------------------------------------------------------------------------

#[test]
fn reject_unknown_block_target() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I1], TypeId::VOID);
    f.blocks.push(BasicBlock::new());
    f.blocks[0].terminator = Some(Terminator::Jump {
        target: BlockId::new(99),
        args: vec![],
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-002".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// §53 — block without terminator
// ---------------------------------------------------------------------------

#[test]
fn reject_missing_terminator() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I64], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    let sum = fresh_result(&mut f, BlockId::new(0), TypeId::I64);
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: f.params[0].value,
            rhs: f.params[0].value,
        },
        Some(sum),
    ));
    // No terminator set.
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-030".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// SSA violations
// ---------------------------------------------------------------------------

#[test]
fn reject_value_redefinition() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I64], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    let sum = fresh_result(&mut f, BlockId::new(0), TypeId::I64);
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: f.params[0].value,
            rhs: f.params[0].value,
        },
        Some(sum),
    ));
    // Second instruction claims the same result value.
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: sum,
            rhs: f.params[0].value,
        },
        Some(sum),
    ));
    f.blocks[0].terminator = Some(Terminator::Return { value: Some(sum) });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-020".to_string()), "{codes:?}");
}

#[test]
fn reject_use_not_dominated() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I1, TypeId::I64], TypeId::I64);
    // bb0: branch %c, bb1, bb2
    // bb1: %2 = add %x, %x ; jump bb3
    // bb2: jump bb3
    // bb3: %3 = add %2, %2 ; return %3   <- %2 not dominated on the bb2 path
    for _ in 0..4 {
        f.blocks.push(BasicBlock::new());
    }
    let (cond, x) = (f.params[0].value, f.params[1].value);
    f.blocks[0].terminator = Some(Terminator::Branch {
        condition: cond,
        then_block: BlockId::new(1),
        then_args: vec![],
        else_block: BlockId::new(2),
        else_args: vec![],
    });
    let sum = fresh_result(&mut f, BlockId::new(1), TypeId::I64);
    f.blocks[1].instructions.push(Instruction::new(
        InstructionKind::Add { lhs: x, rhs: x },
        Some(sum),
    ));
    f.blocks[1].terminator = Some(Terminator::Jump {
        target: BlockId::new(3),
        args: vec![],
    });
    f.blocks[2].terminator = Some(Terminator::Jump {
        target: BlockId::new(3),
        args: vec![],
    });
    let result = fresh_result(&mut f, BlockId::new(3), TypeId::I64);
    f.blocks[3].instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: sum,
            rhs: sum,
        },
        Some(result),
    ));
    f.blocks[3].terminator = Some(Terminator::Return {
        value: Some(result),
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-022".to_string()), "{codes:?}");
}

#[test]
fn reject_reserved_value_never_defined() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[], TypeId::VOID);
    f.blocks.push(BasicBlock::new());
    f.blocks[0].terminator = Some(Terminator::Return { value: None });
    f.values.push(ValueData::new(
        TypeId::I64,
        ValueKind::Reserved,
    ));
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-021".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// CFG shape violations
// ---------------------------------------------------------------------------

#[test]
fn reject_entry_block_with_params() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[], TypeId::VOID);
    f.blocks.push(BasicBlock::new());
    let param = astronomy::ValueId::new(f.values.len() as u32);
    f.values.push(ValueData::new(
        TypeId::I64,
        ValueKind::BlockParam {
            block: BlockId::new(0),
            index: 0,
        },
    ));
    f.blocks[0].params.push(BlockParam {
        value: param,
        ty: TypeId::I64,
    });
    f.blocks[0].terminator = Some(Terminator::Return { value: None });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-031".to_string()), "{codes:?}");
}

#[test]
fn reject_branch_condition_not_i1() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I64], TypeId::VOID);
    f.blocks.push(BasicBlock::new());
    f.blocks.push(BasicBlock::new());
    f.blocks[1].terminator = Some(Terminator::Return { value: None });
    f.blocks[0].terminator = Some(Terminator::Branch {
        condition: f.params[0].value, // i64, not i1
        then_block: BlockId::new(1),
        then_args: vec![],
        else_block: BlockId::new(1),
        else_args: vec![],
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-013".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// Memory typing violations
// ---------------------------------------------------------------------------

#[test]
fn reject_load_from_non_pointer() {
    let mut module = Module::new();
    let mut f = named_function(&mut module, &[TypeId::I64], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    let loaded = fresh_result(&mut f, BlockId::new(0), TypeId::I64);
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Load {
            ty: TypeId::I64,
            pointer: f.params[0].value, // i64, not a pointer
        },
        Some(loaded),
    ));
    f.blocks[0].terminator = Some(Terminator::Return {
        value: Some(loaded),
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-011".to_string()), "{codes:?}");
}

#[test]
fn reject_store_type_mismatch() {
    let mut module = Module::new();
    let sym = module.symbols_mut().intern("f");
    let mut f = Function::new(
        sym,
        Linkage::Exported,
        Abi::Astronomy,
        vec![(None, TypeId::I64), (None, TypeId::I32)],
        TypeId::VOID,
        false,
    );
    f.blocks.push(BasicBlock::new());
    let slot = fresh_result(&mut f, BlockId::new(0), TypeId::I64);
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Alloca {
            pointee: TypeId::I64,
        },
        Some(slot),
    ));
    // Storing an i32 into ptr<i64>.
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Store {
            pointer: slot,
            value: f.params[1].value,
        },
        None,
    ));
    f.blocks[0].terminator = Some(Terminator::Return { value: None });
    // The pointer type must be interned in the module that owns the
    // function, so set it only after deciding on the final module.
    let ptr_ty = module.types_mut().ptr(TypeId::I64);
    f.values[slot.index()].ty = ptr_ty;
    module.functions_mut().push(f);
    let codes = verify_codes(module);
    assert!(codes.contains(&"A-VERIFY-010".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// Call verification
// ---------------------------------------------------------------------------

#[test]
fn reject_call_arity_mismatch() {
    let mut module = Module::new();
    let callee_sym = module.symbols_mut().intern("g");
    let callee = Function::new(
        callee_sym,
        Linkage::External,
        Abi::Astronomy,
        vec![(None, TypeId::I64)],
        TypeId::I64,
        false,
    );
    module.functions_mut().push(callee);

    let mut f = named_function(&mut module, &[], TypeId::I64);
    f.blocks.push(BasicBlock::new());
    let result = fresh_result(&mut f, BlockId::new(0), TypeId::I64);
    f.blocks[0].instructions.push(Instruction::new(
        InstructionKind::Call {
            callee: astronomy::FunctionId::new(0),
            args: vec![], // g expects one i64
        },
        Some(result),
    ));
    f.blocks[0].terminator = Some(Terminator::Return {
        value: Some(result),
    });
    module.functions_mut().push(f);
    let codes = verify_codes(module);
    assert!(codes.contains(&"A-VERIFY-051".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// Function-level violations
// ---------------------------------------------------------------------------

#[test]
fn reject_variadic_definition() {
    let mut module = Module::new();
    let sym = module.symbols_mut().intern("f");
    let mut f = Function::new(
        sym,
        Linkage::External,
        Abi::C,
        vec![(None, TypeId::I64)],
        TypeId::I64,
        true, // variadic — must not have a body
    );
    f.blocks.push(BasicBlock::new());
    f.blocks[0].terminator = Some(Terminator::Return {
        value: Some(f.params[0].value),
    });
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-072".to_string()), "{codes:?}");
}

#[test]
fn reject_declaration_with_internal_linkage() {
    let mut module = Module::new();
    let sym = module.symbols_mut().intern("f");
    let f = Function::new(
        sym,
        Linkage::Internal, // declarations must be external
        Abi::C,
        vec![(None, TypeId::I64)],
        TypeId::I64,
        false,
    );
    let codes = verify_codes(module_with(f));
    assert!(codes.contains(&"A-VERIFY-071".to_string()), "{codes:?}");
}

#[test]
fn reject_duplicate_function_names() {
    let mut module = Module::new();
    let sym = module.symbols_mut().intern("f");
    let a = Function::new(
        sym,
        Linkage::Internal,
        Abi::Astronomy,
        vec![],
        TypeId::VOID,
        false,
    );
    let mut a = a;
    a.blocks.push(BasicBlock::new());
    a.blocks[0].terminator = Some(Terminator::Return { value: None });
    let mut b = a.clone();
    b.blocks.clear(); // make it a declaration so only the name clashes
    b.linkage = Linkage::External;
    module.functions_mut().push(a);
    module.functions_mut().push(b);
    let codes = verify_codes(module);
    assert!(codes.contains(&"A-VERIFY-070".to_string()), "{codes:?}");
}

// ---------------------------------------------------------------------------
// Parser negative tests (malformed text must not panic)
// ---------------------------------------------------------------------------

fn parse_err(src: &str) -> String {
    match text::parse(src) {
        Ok(_) => panic!("parsing unexpectedly succeeded"),
        Err(e) => e.code().to_string(),
    }
}

#[test]
fn parser_rejects_operand_annotation_mismatch() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn(i64 %a, i32 %b) -> i64 linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "%2 = ::ASTRONOMY::ADD i64 i64 %a, i64 %b\n", // %b annotated i64 but is i32
        "::ASTRONOMY::RETURN i64 %2\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-012");
}

#[test]
fn parser_rejects_undefined_value() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn(i64 %a) -> i64 linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "%2 = ::ASTRONOMY::ADD i64 i64 %a, i64 %missing\n",
        "::ASTRONOMY::RETURN i64 %2\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-007");
}

#[test]
fn parser_rejects_undefined_block() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn() -> void linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "::ASTRONOMY::JUMP nowhere\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-008");
}

#[test]
fn parser_rejects_instruction_after_terminator() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn(i64 %a) -> i64 linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "::ASTRONOMY::RETURN i64 %a\n",
        "%2 = ::ASTRONOMY::ADD i64 i64 %a, i64 %a\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-018");
}

#[test]
fn parser_rejects_duplicate_value() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn(i64 %a) -> i64 linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "%2 = ::ASTRONOMY::ADD i64 i64 %a, i64 %a\n",
        "%2 = ::ASTRONOMY::ADD i64 i64 %a, i64 %a\n",
        "::ASTRONOMY::RETURN i64 %2\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-005");
}

#[test]
fn parser_rejects_block_without_terminator() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn() -> void linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-016");
}

#[test]
fn parser_rejects_unknown_directive() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::BOGUS_DIRECTIVE\n",
        "::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-003");
}

#[test]
fn parser_rejects_undefined_function_call() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn() -> void linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "::ASTRONOMY::CALL void @missing()\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-020");
}

#[test]
fn parser_rejects_missing_result() {
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn(i64 %a) -> i64 linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "::ASTRONOMY::ADD i64 i64 %a, i64 %a\n", // no result handle
        "::ASTRONOMY::RETURN i64 %a\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    assert_eq!(parse_err(src), "A-ARN-021");
}

#[test]
fn parser_accepts_anonymous_block_labels() {
    // `bb0` is the printer's own anonymous label (§32) and must parse back
    // as an anonymous block.
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn() -> void linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START bb0\n",
        "::ASTRONOMY::RETURN void\n",
        "::ASTRONOMY::BLOCK_END\n::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    let module = text::parse(src).unwrap();
    let f = &module.functions()[0];
    assert!(f.blocks[0].name.is_none(), "bb0 stays anonymous");
    Verifier::verify(module).unwrap();
}

#[test]
fn verifier_rejects_parsed_branch_argument_mismatch() {
    // Parses fine, fails verification (§53: `jump merge(%x:i32)` into
    // `merge(%value: i64)`).
    let src = concat!(
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
        "::ASTRONOMY::FUNCTION_START f fn(i1 %c, i32 %x) -> i64 linkage=exported abi=astronomy\n",
        "::ASTRONOMY::BLOCK_START entry\n",
        "::ASTRONOMY::BRANCH i1 %c, merge, merge\n",
        "::ASTRONOMY::BLOCK_END\n",
        "::ASTRONOMY::BLOCK_START merge(i64 %v)\n",
        "::ASTRONOMY::RETURN i64 %v\n",
        "::ASTRONOMY::BLOCK_END\n",
        "::ASTRONOMY::FUNCTION_END\n::ASTRONOMY::MODULE_END\n"
    );
    let module = text::parse(src).unwrap();
    // merge has one parameter but both branch edges pass none.
    let codes = match Verifier::verify(module) {
        Ok(_) => panic!("verification unexpectedly succeeded"),
        Err(errors) => errors.iter().map(|e| e.code().to_string()).collect::<Vec<_>>(),
    };
    assert!(codes.contains(&"A-VERIFY-040".to_string()), "{codes:?}");
}

#[test]
fn malformed_inputs_never_panic() {
    // A batch of hostile inputs: the parser must return errors, not panic
    // (§55 Fuzzing awareness).
    let inputs = [
        "",
        "::ASTRONOMY::",
        "::ASTRONOMY::MODULE_START",
        "%",
        "@",
        "\"",
        "::ASTRONOMY::MODULE_START ::ASTRONOMY::MODULE_END extra",
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::FUNCTION_START f fn(-> void\n::ASTRONOMY::MODULE_END",
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::CONSTANT_INT c1 i64 1\n::ASTRONOMY::MODULE_END",
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::FUNCTION_START f fn(i64 %a) -> i64\n%2 = ::ASTRONOMY::LOAD i64 i64 %a\n::ASTRONOMY::MODULE_END",
        "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION -1\n::ASTRONOMY::MODULE_END",
        "::ASTRONOMY::MODULE_START\n%1 = ::ASTRONOMY::ADD\n::ASTRONOMY::MODULE_END",
        "\u{00e9}\u{00ff}\u{4e2d}",
    ];
    for src in inputs {
        let _ = text::parse(src);
    }
}
