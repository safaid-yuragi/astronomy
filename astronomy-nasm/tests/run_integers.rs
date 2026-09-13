//! Execution tests for integer arithmetic, bitwise ops, shifts,
//! comparisons and integer conversions. Each test assembles, links and runs
//! a compiled module against a C driver.

mod common;

use astronomy::TypeId;
use common::{assert_prints, one_function};

#[test]
fn add_i64() {
    let m = one_function("f", &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.add(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(20,22)); return 0; }",
        "42",
    );
}

#[test]
fn i32_add_wraps() {
    let m = one_function("f", &[("a", TypeId::I32), ("b", TypeId::I32)], TypeId::I32, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.add(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(int, int);\nint main(void){ printf(\"%d\\n\", f(2147483647, 1)); return 0; }",
        "-2147483648",
    );
}

#[test]
fn u8_add_wraps() {
    let m = one_function("f", &[("a", TypeId::U8), ("b", TypeId::U8)], TypeId::U8, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.add(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned char f(unsigned char, unsigned char);\nint main(void){ printf(\"%d\\n\", (int)f(255, 2)); return 0; }",
        "1",
    );
}

#[test]
fn sub_and_mul_i16() {
    let m = one_function("f", &[("a", TypeId::I16), ("b", TypeId::I16)], TypeId::I16, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let d = fb.sub(a, b).unwrap();
        let p = fb.mul(d, b).unwrap();
        fb.ret(Some(p)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern short f(short, short);\nint main(void){ printf(\"%d\\n\", (int)f(10, 4)); return 0; }",
        "24",
    );
}

#[test]
fn signed_div_truncates_toward_zero() {
    let m = one_function("f", &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let q = fb.div(a, b).unwrap();
        fb.ret(Some(q)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(-7, 2)); return 0; }",
        "-3",
    );
}

#[test]
fn signed_rem_follows_dividend_sign() {
    let m = one_function("f", &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let r = fb.rem(a, b).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(7, -2)); return 0; }",
        "1",
    );
}

#[test]
fn unsigned_div_and_rem_are_modular() {
    let m = one_function("f", &[("a", TypeId::U64), ("b", TypeId::U64)], TypeId::U64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let q = fb.div(a, b).unwrap();
        let r = fb.rem(a, b).unwrap();
        let s = fb.add(q, r).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned long long f(unsigned long long, unsigned long long);\nint main(void){ printf(\"%llu\\n\", f(18446744073709551615ULL, 10)); return 0; }",
        "1844674407370955166",
    );
}

#[test]
fn bitwise_ops_i64() {
    let m = one_function("f", &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let and = fb.bit_and(a, b).unwrap();
        let or = fb.bit_or(a, b).unwrap();
        let xor = fb.bit_xor(and, or).unwrap();
        fb.ret(Some(xor)).unwrap();
    });
    // (a & b) ^ (a | b) == a ^ b == 0xF0F0
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(0xFF00, 0x0FF0)); return 0; }",
        "61680",
    );
}

#[test]
fn i1_not_and_xor() {
    let m = one_function("f", &[("a", TypeId::I1), ("b", TypeId::I1)], TypeId::I1, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let x = fb.bit_xor(a, b).unwrap();
        fb.ret(Some(x)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(bool, bool);\nint main(void){ printf(\"%d %d\\n\", (int)f(true,true), (int)f(true,false)); return 0; }",
        "0 1",
    );
}

#[test]
fn shl_in_range() {
    let m = one_function("f", &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.shl(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(1, 20)); return 0; }",
        "1048576",
    );
}

#[test]
fn shl_out_of_range_is_zero() {
    let m = one_function("f", &[("a", TypeId::I32), ("b", TypeId::I32)], TypeId::I32, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.shl(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(int, int);\nint main(void){ printf(\"%d\\n\", f(1, 33)); return 0; }",
        "0",
    );
}

#[test]
fn arithmetic_shr_sign_extends() {
    let m = one_function("f", &[("a", TypeId::I32), ("b", TypeId::I32)], TypeId::I32, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.shr(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(int, int);\nint main(void){ printf(\"%d %d\\n\", f(-8, 1), f(-1, 40)); return 0; }",
        "-4 -1",
    );
}

#[test]
fn logical_shr_is_unsigned() {
    let m = one_function("f", &[("a", TypeId::U32), ("b", TypeId::U32)], TypeId::U32, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.shr(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned f(unsigned, unsigned);\nint main(void){ printf(\"%u %u\\n\", f(0x80000000u, 1), f(1u, 40)); return 0; }",
        "1073741824 0",
    );
}

#[test]
fn signed_and_unsigned_comparisons() {
    let signed = one_function("f", &[("a", TypeId::I32), ("b", TypeId::I32)], TypeId::I1, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let c = fb.lt(a, b).unwrap();
        fb.ret(Some(c)).unwrap();
    });
    assert_prints(
        signed,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(int, int);\nint main(void){ printf(\"%d\\n\", (int)f(-1, 0)); return 0; }",
        "1",
    );

    let unsigned = one_function("g", &[("a", TypeId::U32), ("b", TypeId::U32)], TypeId::I1, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let c = fb.lt(a, b).unwrap();
        fb.ret(Some(c)).unwrap();
    });
    assert_prints(
        unsigned,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool g(unsigned, unsigned);\nint main(void){ printf(\"%d %d\\n\", (int)g(4294967295u, 0u), (int)g(0u, 4294967295u)); return 0; }",
        "0 1",
    );
}

#[test]
fn eq_ne_and_ordering() {
    let m = one_function("f", &[("a", TypeId::I64), ("b", TypeId::I64)], TypeId::I1, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let e = fb.eq(a, b).unwrap();
        let g = fb.ge(a, b).unwrap();
        let r = fb.bit_or(e, g).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(long long, long long);\nint main(void){ printf(\"%d %d %d\\n\", (int)f(5,5), (int)f(6,5), (int)f(4,5)); return 0; }",
        "1 1 0",
    );
}

#[test]
fn ext_sign_and_zero_extension() {
    let signed = one_function("f", &[("x", TypeId::I8)], TypeId::I64, |fb| {
        let x = fb.param(0);
        let w = fb.ext(TypeId::I64, x).unwrap();
        fb.ret(Some(w)).unwrap();
    });
    assert_prints(
        signed,
        "#include <stdio.h>\nextern long long f(signed char);\nint main(void){ printf(\"%lld\\n\", f(-5)); return 0; }",
        "-5",
    );

    let unsigned = one_function("g", &[("x", TypeId::U8)], TypeId::U32, |fb| {
        let x = fb.param(0);
        let w = fb.ext(TypeId::U32, x).unwrap();
        fb.ret(Some(w)).unwrap();
    });
    assert_prints(
        unsigned,
        "#include <stdio.h>\nextern unsigned g(unsigned char);\nint main(void){ printf(\"%u\\n\", g(200)); return 0; }",
        "200",
    );
}

#[test]
fn trunc_drops_high_bits() {
    let m = one_function("f", &[("x", TypeId::I64)], TypeId::I32, |fb| {
        let x = fb.param(0);
        let t = fb.trunc(TypeId::I32, x).unwrap();
        fb.ret(Some(t)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(long long);\nint main(void){ printf(\"%d\\n\", f(4294967297LL)); return 0; }",
        "1",
    );
}

#[test]
fn trunc_to_i1_masks_one_bit() {
    let m = one_function("f", &[("x", TypeId::I32)], TypeId::I1, |fb| {
        let x = fb.param(0);
        let t = fb.trunc(TypeId::I1, x).unwrap();
        fb.ret(Some(t)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(int);\nint main(void){ printf(\"%d %d\\n\", (int)f(3), (int)f(2)); return 0; }",
        "1 0",
    );
}

#[test]
fn negative_and_extreme_constants() {
    let m = one_function("f", &[], TypeId::I64, |fb| {
        let a = fb.const_int(TypeId::I64, -1).unwrap();
        let b = fb.const_int(TypeId::I64, i64::MIN as i128).unwrap();
        let s = fb.sub(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(void);\nint main(void){ printf(\"%lld\\n\", f()); return 0; }",
        "9223372036854775807",
    );
}

#[test]
fn unsigned_full_range_constant() {
    let m = one_function("f", &[], TypeId::U64, |fb| {
        let a = fb.const_uint(TypeId::U64, u64::MAX as u128).unwrap();
        fb.ret(Some(a)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned long long f(void);\nint main(void){ printf(\"%llu\\n\", f()); return 0; }",
        "18446744073709551615",
    );
}
