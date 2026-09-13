use astronomy::{
    Abi, BasicBlock, BlockId, Function, Instruction, InstructionKind, Linkage, Module,
    ModuleBuilder, Terminator, TypeId, ValueData, ValueId, ValueKind, Verifier,
};

#[test]
fn mixed_shift_count_types_survive_roundtrip() {
    for count_ty in [
        TypeId::I1,
        TypeId::I8,
        TypeId::U16,
        TypeId::I32,
        TypeId::U64,
        TypeId::I128,
    ] {
        let mut builder = ModuleBuilder::new();
        let id = builder
            .declare_function(
                "shift",
                Linkage::Exported,
                Abi::C,
                &[("x", TypeId::I64), ("count", count_ty)],
                TypeId::I64,
            )
            .unwrap();
        let mut fb = builder.function_builder(id).unwrap();
        fb.append_block();
        let a = fb.shl(fb.param(0), fb.param(1)).unwrap();
        let b = fb.shr(a, fb.param(1)).unwrap();
        fb.ret(Some(b)).unwrap();
        let verified = Verifier::verify(builder.finish()).unwrap();
        let arn = verified.to_arn();
        let reparsed = Verifier::verify(Module::parse_arn(&arn).unwrap()).unwrap();
        assert_eq!(reparsed.to_arn(), arn);
    }
}

#[test]
fn builder_rejects_invalid_shift_operand_types() {
    for (lhs_ty, count_ty) in [(TypeId::I64, TypeId::F64), (TypeId::I1, TypeId::I64)] {
        let mut builder = ModuleBuilder::new();
        let id = builder
            .declare_function(
                "shift",
                Linkage::Exported,
                Abi::C,
                &[("x", lhs_ty), ("count", count_ty)],
                TypeId::VOID,
            )
            .unwrap();
        let mut fb = builder.function_builder(id).unwrap();
        fb.append_block();
        assert!(fb.shl(fb.param(0), fb.param(1)).is_err());
        assert!(fb.shr(fb.param(0), fb.param(1)).is_err());
    }
}

fn raw_shift(lhs_ty: TypeId, count_ty: TypeId, result_ty: TypeId, left: bool) -> Module {
    let mut module = Module::new();
    let symbol = module.symbols_mut().intern("shift");
    let mut function = Function::new(
        symbol,
        Linkage::Exported,
        Abi::C,
        vec![(None, lhs_ty), (None, count_ty)],
        result_ty,
        false,
    );
    let (lhs, rhs) = (function.params[0].value, function.params[1].value);
    let result = ValueId::new(function.values.len() as u32);
    function.values.push(ValueData::new(
        result_ty,
        ValueKind::Inst {
            block: BlockId::new(0),
            index: 0,
        },
    ));
    let mut block = BasicBlock::new();
    block.instructions.push(Instruction::new(
        if left {
            InstructionKind::Shl { lhs, rhs }
        } else {
            InstructionKind::Shr { lhs, rhs }
        },
        Some(result),
    ));
    block.terminator = Some(Terminator::Return {
        value: Some(result),
    });
    function.blocks.push(block);
    module.functions_mut().push(function);
    module
}

#[test]
fn verifier_rejects_invalid_shift_operand_types() {
    for left in [true, false] {
        for (lhs, count) in [(TypeId::I64, TypeId::F32), (TypeId::I1, TypeId::I64)] {
            let errors = Verifier::verify(raw_shift(lhs, count, lhs, left)).unwrap_err();
            assert!(
                errors.iter().any(|e| e.code() == "A-VERIFY-011"),
                "{errors:?}"
            );
        }
    }
}

#[test]
fn verifier_requires_the_shift_result_to_match_the_left_operand() {
    for left in [true, false] {
        let errors =
            Verifier::verify(raw_shift(TypeId::I64, TypeId::U8, TypeId::U8, left)).unwrap_err();
        assert!(
            errors.iter().any(|e| e.code() == "A-VERIFY-012"),
            "{errors:?}"
        );
    }
}
