//! Per-width coverage: each integer width/​signedness combination is exercised
//! through arithmetic, comparison and memory so that sub-register handling
//! (`al`/`ax`/`eax`, sign- vs zero-extension) cannot silently regress.

mod common;

use astronomy::TypeId;
use common::{assert_prints, one_function};

fn add_prints(ty: TypeId, c_ty: &str, fmt: &str, args: &str, expected: &str) {
    let m = one_function("f", &[("a", ty), ("b", ty)], ty, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.add(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    let driver = format!(
        "#include <stdio.h>\nextern {c_ty} f({c_ty}, {c_ty});\nint main(void){{ printf(\"{fmt}\\n\", f({args})); return 0; }}"
    );
    assert_prints(m, &driver, expected);
}

#[test]
fn i8_add_wraps() {
    add_prints(TypeId::I8, "signed char", "%d", "(int)100, (int)100", "-56");
}

#[test]
fn u8_add_wraps() {
    add_prints(TypeId::U8, "unsigned char", "%d", "(int)200, (int)100", "44");
}

#[test]
fn i16_add_wraps() {
    add_prints(TypeId::I16, "short", "%d", "(int)30000, (int)30000", "-5536");
}

#[test]
fn u16_add_wraps() {
    add_prints(TypeId::U16, "unsigned short", "%d", "(int)60000, (int)60000", "54464");
}

#[test]
fn i32_add_wraps() {
    add_prints(TypeId::I32, "int", "%d", "2000000000, 2000000000", "-294967296");
}

#[test]
fn u32_add_wraps() {
    add_prints(TypeId::U32, "unsigned", "%u", "4000000000u, 4000000000u", "3705032704");
}

fn mul_prints(ty: TypeId, c_ty: &str, fmt: &str, args: &str, expected: &str) {
    let m = one_function("f", &[("a", ty), ("b", ty)], ty, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let p = fb.mul(a, b).unwrap();
        fb.ret(Some(p)).unwrap();
    });
    let driver = format!(
        "#include <stdio.h>\nextern {c_ty} f({c_ty}, {c_ty});\nint main(void){{ printf(\"{fmt}\\n\", f({args})); return 0; }}"
    );
    assert_prints(m, &driver, expected);
}

#[test]
fn i8_mul_wraps() {
    mul_prints(TypeId::I8, "signed char", "%d", "(int)-100, (int)3", "-44");
}

#[test]
fn u16_mul_wraps() {
    mul_prints(TypeId::U16, "unsigned short", "%d", "(int)65535, (int)2", "65534");
}

fn cmp_prints(ty: TypeId, c_ty: &str, args: &str, expected: &str) {
    let m = one_function("f", &[("a", ty), ("b", ty)], TypeId::I1, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let c = fb.lt(a, b).unwrap();
        fb.ret(Some(c)).unwrap();
    });
    let driver = format!(
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f({c_ty}, {c_ty});\nint main(void){{ printf(\"%d\\n\", (int)f({args})); return 0; }}"
    );
    assert_prints(m, &driver, expected);
}

#[test]
fn i8_signed_comparison() {
    cmp_prints(TypeId::I8, "signed char", "-5, -1", "1");
}

#[test]
fn u8_unsigned_comparison() {
    cmp_prints(TypeId::U8, "unsigned char", "250, 5", "0");
}

#[test]
fn i16_signed_comparison() {
    cmp_prints(TypeId::I16, "short", "-1, 1", "1");
}

#[test]
fn u32_unsigned_comparison() {
    cmp_prints(TypeId::U32, "unsigned", "1u, 4294967295u", "1");
}

#[test]
fn i8_signed_division() {
    let m = one_function("f", &[("a", TypeId::I8), ("b", TypeId::I8)], TypeId::I8, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let q = fb.div(a, b).unwrap();
        fb.ret(Some(q)).unwrap();
    });
    // -100 / 3 == -33 (truncation toward zero)
    assert_prints(
        m,
        "#include <stdio.h>\nextern signed char f(signed char, signed char);\nint main(void){ printf(\"%d\\n\", (int)f(-100, 3)); return 0; }",
        "-33",
    );
}

#[test]
fn u16_unsigned_remainder() {
    let m = one_function("f", &[("a", TypeId::U16), ("b", TypeId::U16)], TypeId::U16, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let r = fb.rem(a, b).unwrap();
        fb.ret(Some(r)).unwrap();
    });
    // 60000 % 7 == 60000 - 7*8571 = 3
    assert_prints(
        m,
        "#include <stdio.h>\nextern unsigned short f(unsigned short, unsigned short);\nint main(void){ printf(\"%d\\n\", (int)f(60000, 7)); return 0; }",
        "3",
    );
}

#[test]
fn i16_shift() {
    let m = one_function("f", &[("a", TypeId::I16), ("b", TypeId::I16)], TypeId::I16, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.shl(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern short f(short, short);\nint main(void){ printf(\"%d\\n\", (int)f(-256, 2)); return 0; }",
        "-1024",
    );
}

#[test]
fn i16_arithmetic_shift_right() {
    let m = one_function("f", &[("a", TypeId::I16), ("b", TypeId::I16)], TypeId::I16, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let s = fb.shr(a, b).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern short f(short, short);\nint main(void){ printf(\"%d\\n\", (int)f(-256, 2)); return 0; }",
        "-64",
    );
}

#[test]
fn i8_memory_round_trip_preserves_sign() {
    let m = one_function("f", &[("x", TypeId::I8)], TypeId::I8, |fb| {
        let x = fb.param(0);
        let slot = fb.alloca(TypeId::I8).unwrap();
        fb.store(slot, x).unwrap();
        let v = fb.load(slot).unwrap();
        fb.ret(Some(v)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern signed char f(signed char);\nint main(void){ printf(\"%d\\n\", (int)f(-123)); return 0; }",
        "-123",
    );
}
