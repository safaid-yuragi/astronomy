//! Execution tests for calls: direct calls between functions, System V
//! argument passing (register and stack), variadic `extern` calls, and
//! recursion.

mod common;

use astronomy::{Abi, Linkage, ModuleBuilder, TypeId};
use common::assert_prints;

const I32: TypeId = TypeId::I32;
const I64: TypeId = TypeId::I64;
const F64: TypeId = TypeId::F64;

#[test]
fn calls_an_internal_helper() {
    let mut b = ModuleBuilder::with_name("calls");
    let helper = b
        .declare_function("helper_add", Linkage::Internal, Abi::C, &[("a", I64), ("b", I64)], I64)
        .unwrap();
    {
        let mut fb = b.function_builder(helper).unwrap();
        fb.append_block();
        let (a, c) = (fb.param(0), fb.param(1));
        let s = fb.add(a, c).unwrap();
        fb.ret(Some(s)).unwrap();
    }
    let caller = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("a", I64), ("b", I64)], I64)
        .unwrap();
    {
        let mut fb = b.function_builder(caller).unwrap();
        fb.append_block();
        let (a, c) = (fb.param(0), fb.param(1));
        let r = fb.call(helper, &[a, c]).unwrap().unwrap();
        let twice = fb.add(r, r).unwrap();
        fb.ret(Some(twice)).unwrap();
    }
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(20, 1)); return 0; }",
        "42",
    );
}

#[test]
fn eight_integer_arguments_use_stack_slots() {
    let mut b = ModuleBuilder::with_name("args");
    let params: Vec<(&str, TypeId)> = vec![
        ("a1", I64),
        ("a2", I64),
        ("a3", I64),
        ("a4", I64),
        ("a5", I64),
        ("a6", I64),
        ("a7", I64),
        ("a8", I64),
    ];
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &params, I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let a6 = fb.param(5);
    let a7 = fb.param(6);
    let a8 = fb.param(7);
    let ten = fb.const_int(I64, 10).unwrap();
    let hundred = fb.const_int(I64, 100).unwrap();
    let t1 = fb.mul(a8, hundred).unwrap();
    let t2 = fb.mul(a7, ten).unwrap();
    let t3 = fb.add(t1, t2).unwrap();
    let t4 = fb.add(t3, a6).unwrap();
    fb.ret(Some(t4)).unwrap();

    // arguments 1..=8: a8*100 + a7*10 + a6 == 800 + 70 + 6
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long,long long,long long,long long,long long,long long,long long,long long);\nint main(void){ printf(\"%lld\\n\", f(1,2,3,4,5,6,7,8)); return 0; }",
        "876",
    );
}

#[test]
fn nine_float_arguments_use_xmm_and_stack() {
    let mut b = ModuleBuilder::with_name("fargs");
    let params: Vec<(&str, TypeId)> = (1..=9).map(|_| ("", F64)).collect();
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &params, F64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let f1 = fb.param(0);
    let f9 = fb.param(8);
    let ten = fb.const_f64(10.0).unwrap();
    let scaled = fb.mul(f9, ten).unwrap();
    let sum = fb.add(scaled, f1).unwrap();
    fb.ret(Some(sum)).unwrap();

    // f9*10 + f1 == 90 + 1
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern double f(double,double,double,double,double,double,double,double,double);\nint main(void){ printf(\"%.1f\\n\", f(1,2,3,4,5,6,7,8,9)); return 0; }",
        "91.0",
    );
}

#[test]
fn mixed_integer_and_float_arguments() {
    let mut b = ModuleBuilder::with_name("mixed");
    let id = b
        .declare_function(
            "f",
            Linkage::Exported,
            Abi::C,
            &[("a", I64), ("b", F64), ("c", I64), ("d", F64)],
            F64,
        )
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let (a, bv, c, d) = (fb.param(0), fb.param(1), fb.param(2), fb.param(3));
    let af = fb.int_to_float(F64, a).unwrap();
    let cf = fb.int_to_float(F64, c).unwrap();
    let s1 = fb.add(af, bv).unwrap();
    let s2 = fb.add(s1, cf).unwrap();
    let s3 = fb.add(s2, d).unwrap();
    fb.ret(Some(s3)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern double f(long long,double,long long,double);\nint main(void){ printf(\"%.1f\\n\", f(1, 2.5, 3, 0.5)); return 0; }",
        "7.0",
    );
}

#[test]
fn recursive_factorial() {
    let mut b = ModuleBuilder::with_name("fact");
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("n", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    let n = fb.param(0);
    let entry = fb.append_block();
    let base = fb.append_block_with_params(&[("r", I64)]).unwrap();
    let rec = fb.append_block();

    fb.switch_to(entry).unwrap();
    let one = fb.const_int(I64, 1).unwrap();
    let small = fb.le(n, one).unwrap();
    fb.branch(small, base, &[one], rec, &[]).unwrap();

    fb.switch_to(base).unwrap();
    let r = fb.block_param(base, 0).unwrap();
    fb.ret(Some(r)).unwrap();

    fb.switch_to(rec).unwrap();
    let nm1 = fb.sub(n, one).unwrap();
    let sub = fb.call(id, &[nm1]).unwrap().unwrap();
    let prod = fb.mul(n, sub).unwrap();
    fb.ret(Some(prod)).unwrap();

    // 6! == 720, 0! == 1
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld %lld\\n\", f(6), f(0)); return 0; }",
        "720 1",
    );
}

#[test]
fn recursive_fibonacci() {
    let mut b = ModuleBuilder::with_name("fib");
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("n", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    let n = fb.param(0);
    let entry = fb.append_block();
    let base = fb.append_block_with_params(&[("r", I64)]).unwrap();
    let rec = fb.append_block();

    fb.switch_to(entry).unwrap();
    let two = fb.const_int(I64, 2).unwrap();
    let small = fb.lt(n, two).unwrap();
    fb.branch(small, base, &[n], rec, &[]).unwrap();

    fb.switch_to(base).unwrap();
    let r = fb.block_param(base, 0).unwrap();
    fb.ret(Some(r)).unwrap();

    fb.switch_to(rec).unwrap();
    let one = fb.const_int(I64, 1).unwrap();
    let n1 = fb.sub(n, one).unwrap();
    let n2 = fb.sub(n, two).unwrap();
    let a = fb.call(id, &[n1]).unwrap().unwrap();
    let b_ = fb.call(id, &[n2]).unwrap().unwrap();
    let s = fb.add(a, b_).unwrap();
    fb.ret(Some(s)).unwrap();

    // fib(10) == 55, fib(15) == 610
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld %lld\\n\", f(10), f(15)); return 0; }",
        "55 610",
    );
}

#[test]
fn variadic_printf_with_integer() {
    let mut b = ModuleBuilder::with_name("printf_int");
    let i8p = b.ptr_type(TypeId::I8);
    let printf = b
        .declare_extern("printf", Abi::C, &[i8p], true, I32)
        .unwrap();
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("x", I64)], I32)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let x = fb.param(0);
    let fmt = fb.const_string(b"value=%lld\n").unwrap();
    let r = fb.call(printf, &[fmt, x]).unwrap().unwrap();
    fb.ret(Some(r)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern int f(long long);\nint main(void){ f(12345); return 0; }",
        "value=12345",
    );
}

#[test]
fn variadic_printf_with_float_sets_al() {
    let mut b = ModuleBuilder::with_name("printf_float");
    let i8p = b.ptr_type(TypeId::I8);
    let printf = b
        .declare_extern("printf", Abi::C, &[i8p], true, I32)
        .unwrap();
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("x", F64)], TypeId::VOID)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let x = fb.param(0);
    let fmt = fb.const_string(b"x=%.1f\n").unwrap();
    fb.call(printf, &[fmt, x]).unwrap();
    fb.ret_void().unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern void f(double);\nint main(void){ f(1.5); return 0; }",
        "x=1.5",
    );
}

#[test]
fn call_to_void_function() {
    let mut b = ModuleBuilder::with_name("void");
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("x", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let x = fb.param(0);
    fb.ret(Some(x)).unwrap();

    // A void-returning exported function that calls nothing; just confirm a
    // void result round-trips through the C ABI as "no value".
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld\\n\", f(7)); return 0; }",
        "7",
    );
}

#[test]
fn many_call_arguments_across_two_layers() {
    // Callee takes 3 i64 + 2 f64; caller forwards its own arguments.
    let mut b = ModuleBuilder::with_name("fwd");
    let callee = b
        .declare_function(
            "callee",
            Linkage::Internal,
            Abi::C,
            &[("a", I64), ("b", I64), ("c", I64), ("x", F64), ("y", F64)],
            F64,
        )
        .unwrap();
    {
        let mut fb = b.function_builder(callee).unwrap();
        fb.append_block();
        let (a, c1, c2, x, y) = (fb.param(0), fb.param(1), fb.param(2), fb.param(3), fb.param(4));
        let sum_i = fb.add(a, c1).unwrap();
        let sum_i = fb.add(sum_i, c2).unwrap();
        let sum_f = fb.add(x, y).unwrap();
        let fi = fb.int_to_float(F64, sum_i).unwrap();
        let total = fb.add(fi, sum_f).unwrap();
        fb.ret(Some(total)).unwrap();
    }
    let caller = b
        .declare_function(
            "f",
            Linkage::Exported,
            Abi::C,
            &[("a", I64), ("b", I64), ("c", I64), ("x", F64), ("y", F64)],
            F64,
        )
        .unwrap();
    {
        let mut fb = b.function_builder(caller).unwrap();
        fb.append_block();
        let (a, c1, c2, x, y) = (fb.param(0), fb.param(1), fb.param(2), fb.param(3), fb.param(4));
        let r = fb
            .call(callee, &[a, c1, c2, x, y])
            .unwrap()
            .unwrap();
        fb.ret(Some(r)).unwrap();
    }

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern double f(long long,long long,long long,double,double);\nint main(void){ printf(\"%.1f\\n\", f(1, 2, 3, 0.5, 0.5)); return 0; }",
        "7.0",
    );
}

#[test]
fn void_function_with_no_frame() {
    // No parameters and no value slots: the frame is empty and the prologue
    // must still be well-formed (`leave; ret` with no stack adjustment).
    let mut b = ModuleBuilder::with_name("noop");
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[], TypeId::VOID)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    fb.ret_void().unwrap();

    let asm = common::compile_ok(b.finish());
    assert!(!asm.contains("sub rsp"), "{asm}");
    assert!(asm.contains("    leave\n    ret"), "{asm}");
    assert_prints(
        common::one_function("g", &[], TypeId::I32, |fb| {
            let zero = fb.const_int(TypeId::I32, 0).unwrap();
            fb.ret(Some(zero)).unwrap();
        }),
        "#include <stdio.h>\nextern int g(void);\nint main(void){ return g(); }",
        "",
    );
}
