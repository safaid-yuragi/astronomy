//! Acceptance: the full pipeline the backend promises —
//! build → verify → `.arn` → parse → verify → compile → assemble → run —
//! including a module that exercises many instructions at once.

mod common;

use astronomy::{Abi, Linkage, ModuleBuilder, Module, TypeId, Verifier};
use common::{assemble_and_run, one_function};

const I64: TypeId = TypeId::I64;

#[test]
fn arn_roundtrip_then_execute() {
    let module = one_function(
        "clamp_add",
        &[("c", TypeId::I1), ("a", I64), ("b", I64)],
        I64,
        |fb| {
            let (c, a, b) = (fb.param(0), fb.param(1), fb.param(2));
            let entry = fb.current_block().unwrap();
            let merge = fb.append_block_with_params(&[("r", I64)]).unwrap();
            fb.switch_to(entry).unwrap();
            // if c then a else a + b
            let sum = fb.add(a, b).unwrap();
            fb.branch(c, merge, &[sum], merge, &[a]).unwrap();
            let r = fb.block_param(merge, 0).unwrap();
            fb.switch_to(merge).unwrap();
            fb.ret(Some(r)).unwrap();
        },
    );

    // Canonical round-trip first.
    let verified = Verifier::verify(module).unwrap();
    let arn = verified.to_arn();
    let reparsed = Module::parse_arn(&arn).unwrap();
    let reverified = Verifier::verify(reparsed).unwrap();
    assert_eq!(reverified.to_arn(), arn, "canonical text must be stable");

    // Re-parse from text and run the *parsed* module, not the built one.
    let reparsed = Module::parse_arn(&arn).unwrap();
    let Some(out) = assemble_and_run(
        reparsed,
        "#include <stdio.h>\n#include <stdbool.h>\nextern long long clamp_add(bool, long long, long long);\nint main(void){ printf(\"%lld %lld\\n\", clamp_add(true, 2, 40), clamp_add(false, 2, 40)); return 0; }",
    ) else {
        return;
    };
    assert_eq!(out.out(), "42 2");
}

#[test]
fn kitchen_sink_module_runs() {
    // Memory + aggregates + conversions + bitwise + shift + compare, all in
    // one function, executed for real.
    let mut b = ModuleBuilder::with_name("kitchen");
    let pair = b.struct_type(&[I64, I64]);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("x", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let x = fb.param(0);

    // memory
    let slot = fb.alloca(I64).unwrap();
    fb.store(slot, x).unwrap();
    let loaded = fb.load(slot).unwrap();

    // struct construct/extract/insert
    let p = fb.construct(pair, &[loaded, loaded]).unwrap();
    let p2 = fb.insert(p, 1, x).unwrap();
    let a = fb.extract(p2, 0).unwrap();
    let c = fb.extract(p2, 1).unwrap();

    // conversions and float math
    let af = fb.int_to_float(TypeId::F64, a).unwrap();
    let half = fb.const_f64(0.5).unwrap();
    let scaled = fb.mul(af, half).unwrap();
    let back = fb.float_to_int(I64, scaled).unwrap();

    // bitwise + shifts
    let mask = fb.const_int(I64, 0xFF).unwrap();
    let masked = fb.bit_and(c, mask).unwrap();
    let three = fb.const_int(I64, 3).unwrap();
    let shifted = fb.shl(masked, three).unwrap();

    // compare + select via block arg
    let entry = fb.current_block().unwrap();
    let merge = fb.append_block_with_params(&[("r", I64)]).unwrap();
    fb.switch_to(entry).unwrap();
    let cond = fb.gt(shifted, back).unwrap();
    fb.branch(cond, merge, &[shifted], merge, &[back]).unwrap();
    let r = fb.block_param(merge, 0).unwrap();
    fb.switch_to(merge).unwrap();
    let total = fb.add(r, back).unwrap();
    fb.ret(Some(total)).unwrap();

    // x = 20: slot=20, pair=(20,20), insert->(20,20), af=20.0, scaled=10.0,
    // back=10, masked=20&255=20, shifted=20<<3=160, cond=160>10 -> 160,
    // total=160+10=170
    let Some(out) = assemble_and_run(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld\\n\", f(20)); return 0; }",
    ) else {
        return;
    };
    assert_eq!(out.out(), "170");
}
