//! Execution tests for floating point: arithmetic, comparisons (including
//! NaN ordering), and the conversion instructions `int_to_float` /
//! `float_to_int` with their saturating semantics.

mod common;

use astronomy::TypeId;
use common::{assert_prints, one_function};

const F64: TypeId = TypeId::F64;
const F32: TypeId = TypeId::F32;

#[test]
fn f64_arithmetic() {
    let m = one_function("f", &[("a", F64), ("b", F64)], F64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let sum = fb.add(a, b).unwrap();
        let diff = fb.sub(a, b).unwrap();
        let prod = fb.mul(sum, diff).unwrap();
        let quot = fb.div(prod, a).unwrap();
        fb.ret(Some(quot)).unwrap();
    });
    // ((a+b)*(a-b))/a with a=3, b=1 -> (4*2)/3 = 2.6666666666666665
    assert_prints(
        m,
        "#include <stdio.h>\nextern double f(double, double);\nint main(void){ printf(\"%.16g\\n\", f(3.0, 1.0)); return 0; }",
        "2.666666666666667",
    );
}

#[test]
fn f32_arithmetic() {
    let m = one_function("f", &[("a", F32), ("b", F32)], F32, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.add(a, b).unwrap();
        let p = fb.mul(s, b).unwrap();
        fb.ret(Some(p)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern float f(float, float);\nint main(void){ printf(\"%.4f\\n\", (double)f(1.5f, 2.25f)); return 0; }",
        "8.4375",
    );
}

#[test]
fn f64_ordering_comparisons() {
    let m = one_function("f", &[("a", F64), ("b", F64)], TypeId::I1, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let lt = fb.lt(a, b).unwrap();
        let eq = fb.eq(a, b).unwrap();
        // a <= b  ==  (a < b) | (a == b)
        let le = fb.bit_or(lt, eq).unwrap();
        fb.ret(Some(le)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(double, double);\nint main(void){ printf(\"%d %d %d\\n\", (int)f(1.0,2.0), (int)f(2.0,2.0), (int)f(3.0,2.0)); return 0; }",
        "1 1 0",
    );
}

#[test]
fn nan_mixed_predicates_execute() {
    let m = one_function("f", &[], TypeId::I32, |fb| {
        let nan = fb.const_f64(f64::NAN).unwrap();
        let one = fb.const_f64(1.0).unwrap();
        let eq = fb.eq(nan, nan).unwrap(); // 0
        let ne = fb.ne(nan, nan).unwrap(); // 1
        let lt = fb.lt(nan, one).unwrap(); // 0
        let gt = fb.gt(nan, one).unwrap(); // 0
        let e = fb.ext(TypeId::I32, eq).unwrap();
        let n = fb.ext(TypeId::I32, ne).unwrap();
        let l = fb.ext(TypeId::I32, lt).unwrap();
        let g = fb.ext(TypeId::I32, gt).unwrap();
        let a = fb.add(e, n).unwrap();
        let b = fb.add(l, g).unwrap();
        let c = fb.add(a, b).unwrap();
        fb.ret(Some(c)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(void);\nint main(void){ printf(\"%d\\n\", f()); return 0; }",
        "1",
    );
}

#[test]
fn nan_is_not_equal_to_itself() {
    let m = one_function("f", &[], TypeId::I1, |fb| {
        let nan = fb.const_f64(f64::NAN).unwrap();
        let r = fb.eq(nan, nan).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(void);\nint main(void){ printf(\"%d\\n\", (int)f()); return 0; }",
        "0",
    );
}

#[test]
fn nan_ne_is_true() {
    let m = one_function("f", &[], TypeId::I1, |fb| {
        let nan = fb.const_f64(f64::NAN).unwrap();
        let r = fb.ne(nan, nan).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(void);\nint main(void){ printf(\"%d\\n\", (int)f()); return 0; }",
        "1",
    );
}

#[test]
fn nan_ordered_comparisons_are_false() {
    let m = one_function("f", &[], TypeId::I32, |fb| {
        let nan = fb.const_f64(f64::NAN).unwrap();
        let one = fb.const_f64(1.0).unwrap();
        let lt = fb.lt(nan, one).unwrap();
        let le = fb.le(nan, one).unwrap();
        let gt = fb.gt(nan, one).unwrap();
        let ge = fb.ge(nan, one).unwrap();
        let l = fb.ext(TypeId::I32, lt).unwrap();
        let lew = fb.ext(TypeId::I32, le).unwrap();
        let g = fb.ext(TypeId::I32, gt).unwrap();
        let gew = fb.ext(TypeId::I32, ge).unwrap();
        let a = fb.add(l, lew).unwrap();
        let b = fb.add(g, gew).unwrap();
        let c = fb.add(a, b).unwrap();
        fb.ret(Some(c)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(void);\nint main(void){ printf(\"%d\\n\", f()); return 0; }",
        "0",
    );
}

#[test]
fn int_to_float_signed_and_unsigned() {
    let signed = one_function("f", &[("x", TypeId::I32)], F64, |fb| {
        let x = fb.param(0);
        let d = fb.int_to_float(F64, x).unwrap();
        fb.ret(Some(d)).unwrap();
    });
    assert_prints(
        signed,
        "#include <stdio.h>\nextern double f(int);\nint main(void){ printf(\"%.4f\\n\", f(-7)); return 0; }",
        "-7.0000",
    );

    let unsigned = one_function("g", &[("x", TypeId::U8)], F64, |fb| {
        let x = fb.param(0);
        let d = fb.int_to_float(F64, x).unwrap();
        fb.ret(Some(d)).unwrap();
    });
    assert_prints(
        unsigned,
        "#include <stdio.h>\nextern double g(unsigned char);\nint main(void){ printf(\"%.4f\\n\", g(200)); return 0; }",
        "200.0000",
    );
}

#[test]
fn int_to_float_unsigned_64_uses_large_path() {
    let m = one_function("f", &[("x", TypeId::U64)], F64, |fb| {
        let x = fb.param(0);
        let d = fb.int_to_float(F64, x).unwrap();
        fb.ret(Some(d)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern double f(unsigned long long);\nint main(void){ printf(\"%.17g\\n\", f(18446744073709551615ULL)); return 0; }",
        "1.8446744073709552e+19",
    );
}

#[test]
fn float_to_int_truncates_toward_zero() {
    let m = one_function("f", &[("x", F64)], TypeId::I64, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::I64, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(double);\nint main(void){ printf(\"%lld %lld\\n\", f(2.9), f(-2.9)); return 0; }",
        "2 -2",
    );
}

#[test]
fn float_to_int_saturates_i64() {
    let m = one_function("f", &[("x", F64)], TypeId::I64, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::I64, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(double);\nint main(void){ printf(\"%lld %lld\\n\", f(1e300), f(-1e300)); return 0; }",
        "9223372036854775807 -9223372036854775808",
    );
}

#[test]
fn float_to_int_saturates_i32() {
    let m = one_function("f", &[("x", F64)], TypeId::I32, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::I32, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(double);\nint main(void){ printf(\"%d %d\\n\", f(1e30), f(-1e30)); return 0; }",
        "2147483647 -2147483648",
    );
}

#[test]
fn float_to_int_nan_is_zero() {
    let m = one_function("f", &[], TypeId::I64, |fb| {
        let nan = fb.const_f64(f64::NAN).unwrap();
        let i = fb.float_to_int(TypeId::I64, nan).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(void);\nint main(void){ printf(\"%lld\\n\", f()); return 0; }",
        "0",
    );
}

#[test]
fn float_to_int_unsigned_saturates_and_clamps_negatives() {
    let m = one_function("f", &[("x", F64)], TypeId::U32, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::U32, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned f(double);\nint main(void){ printf(\"%u %u %u\\n\", f(1e30), f(-5.0), f(7.9)); return 0; }",
        "4294967295 0 7",
    );
}

#[test]
fn float_to_int_unsigned_64_handles_large_values() {
    let m = one_function("f", &[("x", F64)], TypeId::U64, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::U64, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned long long f(double);\nint main(void){ printf(\"%llu %llu\\n\", f(1e19), f(1e30)); return 0; }",
        "10000000000000000000 18446744073709551615",
    );
}

#[test]
fn f32_float_to_int() {
    let m = one_function("f", &[("x", F32)], TypeId::I32, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::I32, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(float);\nint main(void){ printf(\"%d\\n\", f(-2.9f)); return 0; }",
        "-2",
    );
}

#[test]
fn int_float_round_trip() {
    let m = one_function("f", &[("x", TypeId::I32)], TypeId::I32, |fb| {
        let x = fb.param(0);
        let d = fb.int_to_float(F64, x).unwrap();
        let half = fb.const_f64(0.5).unwrap();
        let scaled = fb.mul(d, half).unwrap();
        let i = fb.float_to_int(TypeId::I32, scaled).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(int);\nint main(void){ printf(\"%d\\n\", f(41)); return 0; }",
        "20",
    );
}

#[test]
fn float_to_i8_saturates() {
    let m = one_function("f", &[("x", F64)], TypeId::I8, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::I8, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern signed char f(double);\nint main(void){ printf(\"%d %d %d\\n\", (int)f(1e30), (int)f(-1e30), (int)f(42.9)); return 0; }",
        "127 -128 42",
    );
}

#[test]
fn float_to_u8_saturates_and_clamps() {
    let m = one_function("f", &[("x", F64)], TypeId::U8, |fb| {
        let x = fb.param(0);
        let i = fb.float_to_int(TypeId::U8, x).unwrap();
        fb.ret(Some(i)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned char f(double);\nint main(void){ printf(\"%d %d %d\\n\", (int)f(1e30), (int)f(-3.2), (int)f(200.5)); return 0; }",
        "255 0 200",
    );
}
