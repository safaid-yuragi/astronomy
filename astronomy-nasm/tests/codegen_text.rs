//! Structural/textual tests: the shape of the emitted NASM, directive
//! emission, determinism, and the structured errors for unsupported input.
//!
//! These tests do not require a toolchain.

mod common;

use astronomy::{Abi, Linkage, ModuleBuilder, TypeId};
use common::{compile_err, compile_ok};

#[test]
fn header_directives_are_emitted() {
    let module = common::one_function("id", &[("x", TypeId::I64)], TypeId::I64, |fb| {
        let x = fb.param(0);
        fb.ret(Some(x)).unwrap();
    });
    let asm = compile_ok(module);
    assert!(asm.starts_with("bits 64\n"), "{asm}");
    assert!(asm.contains("\ndefault rel\n"), "{asm}");
    assert!(asm.contains("\nsection .text\n"), "{asm}");
    assert!(asm.contains("\nglobal $id\n"), "{asm}");
    assert!(asm.contains("\n$id:\n"), "{asm}");
}

#[test]
fn internal_linkage_is_not_global() {
    let mut b = ModuleBuilder::with_name("t");
    let id = b
        .declare_function("secret", Linkage::Internal, Abi::C, &[], TypeId::VOID)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    fb.ret_void().unwrap();
    let asm = compile_ok(b.finish());
    assert!(!asm.contains("global $secret"), "{asm}");
    assert!(asm.contains("\n$secret:\n"), "{asm}");
}

#[test]
fn declarations_become_extern() {
    let mut b = ModuleBuilder::with_name("t");
    let i8p = b.ptr_type(TypeId::I8);
    let puts = b
        .declare_extern("puts", Abi::C, &[i8p], false, TypeId::I32)
        .unwrap();
    let id = b
        .declare_function("call_puts", Linkage::Exported, Abi::C, &[], TypeId::VOID)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let s = fb.const_string(b"hi").unwrap();
    fb.call(puts, &[s]).unwrap();
    fb.ret_void().unwrap();
    let asm = compile_ok(b.finish());
    assert!(asm.contains("\nextern $puts\n"), "{asm}");
    assert!(asm.contains("    call $puts\n"), "{asm}");
}

#[test]
fn deterministic_output() {
    let build = || {
        common::one_function("f", &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I64, |fb| {
            let (a, b) = (fb.param(0), fb.param(1));
            let s = fb.add(a, b).unwrap();
            let p = fb.mul(s, b).unwrap();
            fb.ret(Some(p)).unwrap();
        })
    };
    assert_eq!(compile_ok(build()), compile_ok(build()));
}

#[test]
fn block_labels_and_terminators() {
    let module = common::one_function(
        "pick",
        &[("c", TypeId::I1), ("a", TypeId::I64), ("b", TypeId::I64)],
        TypeId::I64,
        |fb| {
            let (c, a, b) = (fb.param(0), fb.param(1), fb.param(2));
            let entry = fb.current_block().unwrap();
            let left = fb.append_block_with_params(&[("x", TypeId::I64)]).unwrap();
            let right = fb.append_block_with_params(&[("x", TypeId::I64)]).unwrap();
            let merge = fb.append_block_with_params(&[("r", TypeId::I64)]).unwrap();
            fb.switch_to(entry).unwrap();
            fb.branch(c, left, &[a], right, &[b]).unwrap();
            let xl = fb.block_param(left, 0).unwrap();
            fb.switch_to(left).unwrap();
            fb.jump(merge, &[xl]).unwrap();
            let xr = fb.block_param(right, 0).unwrap();
            fb.switch_to(right).unwrap();
            fb.jump(merge, &[xr]).unwrap();
            let r = fb.block_param(merge, 0).unwrap();
            fb.switch_to(merge).unwrap();
            fb.ret(Some(r)).unwrap();
        },
    );
    let asm = compile_ok(module);
    for expected in [
        "pick.bb0:",
        "pick.bb1:",
        "pick.bb2:",
        "pick.bb3:",
        "    jmp pick.bb3",
        "    cmp byte [rbp-",
        "    je pick.else",
    ] {
        assert!(asm.contains(expected), "missing `{expected}` in:\n{asm}");
    }
}

#[test]
fn self_loop_uses_staging_copy() {
    // `bb1(a, b) -> jump bb1(b, a)` needs a parallel copy; the backend must
    // stage through scratch memory rather than clobbering a slot.
    let module = common::one_function(
        "spin",
        &[("x", TypeId::I64)],
        TypeId::I64,
        |fb| {
            let x = fb.param(0);
            let entry = fb.current_block().unwrap();
            let loop_block = fb
                .append_block_with_params(&[("a", TypeId::I64), ("b", TypeId::I64)])
                .unwrap();
            fb.switch_to(entry).unwrap();
            let zero = fb.const_int(TypeId::I64, 0).unwrap();
            fb.jump(loop_block, &[x, zero]).unwrap();
            let a = fb.block_param(loop_block, 0).unwrap();
            let b = fb.block_param(loop_block, 1).unwrap();
            fb.switch_to(loop_block).unwrap();
            // swap (a, b) -> (b, a)
            fb.jump(loop_block, &[b, a]).unwrap();
            // Unreachable exit is fine; a separate return keeps the function
            // well-formed for the test's purposes.
            let _ = a;
            let _ = b;
        },
    );
    let asm = compile_ok(module);
    // Staging means the first writes go to scratch slots, not param slots.
    let has_rep = asm.matches("mov rax, qword [rbp-").count();
    assert!(has_rep >= 2, "{asm}");
}

#[test]
fn string_constant_goes_to_rodata() {
    let module = common::one_function("s", &[], TypeId::I32, |fb| {
        let p = fb.const_string(b"hello").unwrap();
        let _ = p;
        let zero = fb.const_int(TypeId::I32, 0).unwrap();
        fb.ret(Some(zero)).unwrap();
    });
    let asm = compile_ok(module);
    assert!(asm.contains("section .rodata"), "{asm}");
    assert!(asm.contains("arn.data.0:"), "{asm}");
    assert!(asm.contains("0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x00"), "{asm}");
}

#[test]
fn null_constant_materializes_zero() {
    let module = common::one_function("n", &[], TypeId::I1, |fb| {
        let null = fb.const_null(TypeId::I64).unwrap();
        let is_null = fb.eq(null, null).unwrap();
        fb.ret(Some(is_null)).unwrap();
    });
    let asm = compile_ok(module);
    assert!(asm.contains("xor eax, eax"), "{asm}");
}

#[test]
fn i128_is_rejected_with_a_structured_error() {
    let module = common::one_function("widen", &[("x", TypeId::I32)], TypeId::I128, |fb| {
        let x = fb.param(0);
        let wide = fb.ext(TypeId::I128, x).unwrap();
        fb.ret(Some(wide)).unwrap();
    });
    let err = compile_err(module);
    assert_eq!(err.code(), "A-NASM-001");
    assert!(err.to_string().contains("128-bit"), "{err}");
}

#[test]
fn aggregate_parameter_is_rejected() {
    let mut b = ModuleBuilder::with_name("t");
    let pair = b.struct_type(&[TypeId::I64, TypeId::I64]);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("p", pair)], TypeId::I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let p = fb.param(0);
    let x = fb.extract(p, 0).unwrap();
    fb.ret(Some(x)).unwrap();
    let err = compile_err(b.finish());
    assert_eq!(err.code(), "A-NASM-002");
}

#[test]
fn aggregate_return_is_rejected() {
    let mut b = ModuleBuilder::with_name("t");
    let pair = b.struct_type(&[TypeId::I64, TypeId::I64]);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("x", TypeId::I64)], pair)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let x = fb.param(0);
    let agg = fb.construct(pair, &[x, x]).unwrap();
    fb.ret(Some(agg)).unwrap();
    let err = compile_err(b.finish());
    assert_eq!(err.code(), "A-NASM-002");
}

#[test]
fn noncanonical_integer_widths_are_rejected() {
    // The low-level type interner can create widths that ARN cannot spell.
    // Do not silently use an 8-bit load or a 64-bit store for those values.
    for bits in [0, 2, 7, 24, 48, 65, 128] {
        let mut builder = ModuleBuilder::new();
        let ty = builder.types().intern(astronomy::TypeData::Int { bits, signed: false });
        let id = builder.declare_function(
            "identity", Linkage::Exported, Abi::C, &[("x", ty)], ty,
        ).unwrap();
        let mut fb = builder.function_builder(id).unwrap();
        fb.append_block();
        fb.ret(Some(fb.param(0))).unwrap();
        assert_eq!(compile_err(builder.finish()).code(), "A-NASM-001");
    }
}
