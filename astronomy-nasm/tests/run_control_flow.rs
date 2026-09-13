//! Execution tests for control flow: branches, loops and block arguments
//! (the phi-free merging mechanism).

mod common;

use astronomy::{TypeId, Verifier};
use common::{assert_prints, compile_ok, one_function};

const I64: TypeId = TypeId::I64;

#[test]
fn abs_via_branch() {
    let m = one_function("f", &[("x", I64)], I64, |fb| {
        let x = fb.param(0);
        let entry = fb.current_block().unwrap();
        let negative = fb.append_block();
        let positive = fb.append_block();
        fb.switch_to(entry).unwrap();
        let zero = fb.const_int(I64, 0).unwrap();
        let cond = fb.lt(x, zero).unwrap();
        fb.branch(cond, negative, &[], positive, &[]).unwrap();
        fb.switch_to(negative).unwrap();
        let neg = fb.sub(zero, x).unwrap();
        fb.ret(Some(neg)).unwrap();
        fb.switch_to(positive).unwrap();
        fb.ret(Some(x)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld %lld\\n\", f(-42), f(7)); return 0; }",
        "42 7",
    );
}

#[test]
fn pick_with_block_arguments() {
    let m = one_function(
        "f",
        &[("c", TypeId::I1), ("a", I64), ("b", I64)],
        I64,
        |fb| {
            let (c, a, b) = (fb.param(0), fb.param(1), fb.param(2));
            let entry = fb.current_block().unwrap();
            let merge = fb.append_block_with_params(&[("r", I64)]).unwrap();
            fb.switch_to(entry).unwrap();
            fb.branch(c, merge, &[a], merge, &[b]).unwrap();
            let r = fb.block_param(merge, 0).unwrap();
            fb.switch_to(merge).unwrap();
            fb.ret(Some(r)).unwrap();
        },
    );
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern long long f(bool, long long, long long);\nint main(void){ printf(\"%lld %lld\\n\", f(true, 111, 222), f(false, 111, 222)); return 0; }",
        "111 222",
    );
}

#[test]
fn sum_loop_with_block_parameters() {
    let m = one_function("f", &[("n", I64)], I64, |fb| {
        let n = fb.param(0);
        let entry = fb.current_block().unwrap();
        let loop_b = fb
            .append_block_with_params(&[("i", I64), ("acc", I64)])
            .unwrap();
        let step_b = fb.append_block();
        let done_b = fb.append_block_with_params(&[("r", I64)]).unwrap();

        fb.switch_to(entry).unwrap();
        let zero = fb.const_int(I64, 0).unwrap();
        let nonpos = fb.le(n, zero).unwrap();
        fb.branch(nonpos, done_b, &[zero], loop_b, &[n, zero])
            .unwrap();

        let i = fb.block_param(loop_b, 0).unwrap();
        let acc = fb.block_param(loop_b, 1).unwrap();
        fb.switch_to(loop_b).unwrap();
        let finish = fb.eq(i, zero).unwrap();
        fb.branch(finish, done_b, &[acc], step_b, &[]).unwrap();

        fb.switch_to(step_b).unwrap();
        let one = fb.const_int(I64, 1).unwrap();
        let next = fb.sub(i, one).unwrap();
        let sum = fb.add(acc, i).unwrap();
        fb.jump(loop_b, &[next, sum]).unwrap();

        let r = fb.block_param(done_b, 0).unwrap();
        fb.switch_to(done_b).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    // sum(1..=100) == 5050, sum(-5) == 0
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld %lld\\n\", f(100), f(-5)); return 0; }",
        "5050 0",
    );
}

#[test]
fn factorial_loop() {
    let m = one_function("f", &[("n", I64)], I64, |fb| {
        let n = fb.param(0);
        let entry = fb.current_block().unwrap();
        let loop_b = fb
            .append_block_with_params(&[("i", I64), ("acc", I64)])
            .unwrap();
        let done_b = fb.append_block_with_params(&[("r", I64)]).unwrap();

        fb.switch_to(entry).unwrap();
        let one = fb.const_int(I64, 1).unwrap();
        let small = fb.le(n, one).unwrap();
        fb.branch(small, done_b, &[one], loop_b, &[n, one]).unwrap();

        let i = fb.block_param(loop_b, 0).unwrap();
        let acc = fb.block_param(loop_b, 1).unwrap();
        fb.switch_to(loop_b).unwrap();
        let done = fb.eq(i, one).unwrap();
        let next = fb.sub(i, one).unwrap();
        let prod = fb.mul(acc, i).unwrap();
        fb.branch(done, done_b, &[acc], loop_b, &[next, prod]).unwrap();

        let r = fb.block_param(done_b, 0).unwrap();
        fb.switch_to(done_b).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    // 5! == 120, 0! == 1
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld %lld\\n\", f(5), f(0)); return 0; }",
        "120 1",
    );
}

#[test]
fn three_way_sign() {
    let m = one_function("f", &[("x", I64)], I64, |fb| {
        let x = fb.param(0);
        let entry = fb.current_block().unwrap();
        let ret1 = fb.append_block();
        let check_neg = fb.append_block();
        let retneg = fb.append_block();
        let retzero = fb.append_block();

        fb.switch_to(entry).unwrap();
        let zero = fb.const_int(I64, 0).unwrap();
        let pos = fb.gt(x, zero).unwrap();
        let neg = fb.lt(x, zero).unwrap();
        fb.branch(pos, ret1, &[], check_neg, &[]).unwrap();

        fb.switch_to(ret1).unwrap();
        let one = fb.const_int(I64, 1).unwrap();
        fb.ret(Some(one)).unwrap();

        fb.switch_to(check_neg).unwrap();
        fb.branch(neg, retneg, &[], retzero, &[]).unwrap();

        fb.switch_to(retneg).unwrap();
        let minus = fb.const_int(I64, -1).unwrap();
        fb.ret(Some(minus)).unwrap();

        fb.switch_to(retzero).unwrap();
        fb.ret(Some(zero)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld %lld %lld\\n\", f(9), f(-9), f(0)); return 0; }",
        "1 -1 0",
    );
}

#[test]
fn loop_variable_via_parallel_edge_copy() {
    // `loop(a, b) -> jump loop(b, a + b)` is a parallel swap+add; the
    // staging area must keep both old values alive.
    let m = one_function("f", &[("n", I64)], I64, |fb| {
        let n = fb.param(0);
        let entry = fb.current_block().unwrap();
        let loop_b = fb
            .append_block_with_params(&[("i", I64), ("a", I64), ("b", I64)])
            .unwrap();
        let done_b = fb.append_block_with_params(&[("r", I64)]).unwrap();

        fb.switch_to(entry).unwrap();
        let zero = fb.const_int(I64, 0).unwrap();
        let one = fb.const_int(I64, 1).unwrap();
        // count down from n, Fibonacci-style
        fb.jump(loop_b, &[n, zero, one]).unwrap();

        let i = fb.block_param(loop_b, 0).unwrap();
        let a = fb.block_param(loop_b, 1).unwrap();
        let b = fb.block_param(loop_b, 2).unwrap();
        fb.switch_to(loop_b).unwrap();
        let finish = fb.eq(i, zero).unwrap();
        let next = fb.sub(i, one).unwrap();
        let sum = fb.add(a, b).unwrap();
        fb.branch(finish, done_b, &[a], loop_b, &[next, b, sum])
            .unwrap();

        let r = fb.block_param(done_b, 0).unwrap();
        fb.switch_to(done_b).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    // fib with fib(0)=0, fib(1)=1: after n iterations `a` is fib(n)
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld %lld %lld\\n\", f(0), f(10), f(20)); return 0; }",
        "0 55 6765",
    );
}

#[test]
fn unreachable_lowers_to_ud2() {
    let m = one_function("f", &[("x", I64)], I64, |fb| {
        let x = fb.param(0);
        let entry = fb.current_block().unwrap();
        let dead = fb.append_block();
        fb.switch_to(entry).unwrap();
        fb.ret(Some(x)).unwrap();
        fb.switch_to(dead).unwrap();
        fb.unreachable().unwrap();
    });
    let asm = compile_ok(m);
    assert!(asm.contains("ud2"), "{asm}");
}

#[test]
fn unreachable_block_does_not_break_verification() {
    let m = one_function("f", &[], I64, |fb| {
        let entry = fb.current_block().unwrap();
        let dead = fb.append_block();
        fb.switch_to(entry).unwrap();
        let zero = fb.const_int(I64, 0).unwrap();
        fb.ret(Some(zero)).unwrap();
        fb.switch_to(dead).unwrap();
        fb.unreachable().unwrap();
    });
    // compile_ok verifies internally; a panic here would fail the test.
    let verified = Verifier::verify(m).unwrap();
    assert!(!verified.to_arn().is_empty());
}

#[test]
fn aggregate_block_arguments() {
    // A block parameter of struct type: edge copies must move the whole
    // aggregate (16 bytes) through the staging area.
    use astronomy::{Abi, Linkage, ModuleBuilder};
    let mut b = ModuleBuilder::with_name("aggphi");
    let pair = b.struct_type(&[I64, I64]);
    let id = b
        .declare_function(
            "f",
            Linkage::Exported,
            Abi::C,
            &[("c", TypeId::I1), ("a", I64), ("b", I64)],
            I64,
        )
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    let (c, a, bv) = (fb.param(0), fb.param(1), fb.param(2));
    let entry = fb.append_block();
    let merge = fb.append_block_with_params(&[("p", pair)]).unwrap();
    fb.switch_to(entry).unwrap();
    let s1 = fb.construct(pair, &[a, bv]).unwrap();
    let s2 = fb.construct(pair, &[bv, a]).unwrap();
    fb.branch(c, merge, &[s1], merge, &[s2]).unwrap();
    let p = fb.block_param(merge, 0).unwrap();
    fb.switch_to(merge).unwrap();
    let x = fb.extract(p, 0).unwrap();
    let y = fb.extract(p, 1).unwrap();
    let d = fb.sub(x, y).unwrap();
    fb.ret(Some(d)).unwrap();

    // f(true, 10, 3) -> (10-3)=7 ; f(false,10,3) -> (3-10)=-7
    common::assert_prints(
        b.finish(),
        "#include <stdio.h>\n#include <stdbool.h>\nextern long long f(bool, long long, long long);\nint main(void){ printf(\"%lld %lld\\n\", f(true, 10, 3), f(false, 10, 3)); return 0; }",
        "7 -7",
    );
}
