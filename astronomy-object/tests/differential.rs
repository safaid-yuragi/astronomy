//! Deterministic execution comparisons against independent numeric references.

mod common;

use std::fmt::Write;

use astronomy::{Abi, Linkage, Module, ModuleBuilder, TypeId, Verifier};
use common::assert_prints;

const INTEGERS: [(TypeId, &str, u32, bool); 8] = [
    (TypeId::I8, "int8_t", 8, true),
    (TypeId::U8, "uint8_t", 8, false),
    (TypeId::I16, "int16_t", 16, true),
    (TypeId::U16, "uint16_t", 16, false),
    (TypeId::I32, "int32_t", 32, true),
    (TypeId::U32, "uint32_t", 32, false),
    (TypeId::I64, "int64_t", 64, true),
    (TypeId::U64, "uint64_t", 64, false),
];

fn run_roundtrip(builder: ModuleBuilder, driver: &str) {
    let verified = Verifier::verify(builder.finish()).unwrap();
    let module = Module::parse_arn(&verified.to_arn()).unwrap();
    assert_prints(module, driver, "ok");
}

#[test]
fn integer_operations_match_reference_at_all_widths() {
    let mut builder = ModuleBuilder::new();
    let mut driver = String::from(
        r#"
        #include <stdint.h>
        #include <stdbool.h>
        #include <stdio.h>
        static __int128 signed_value(uint64_t x, unsigned bits) {
            return (x & (UINT64_C(1) << (bits - 1)))
                ? (__int128)x - ((__int128)1 << bits) : (__int128)x;
        }
        static uint64_t reference(unsigned op, uint64_t a, uint64_t b,
                                  unsigned bits, bool is_signed, uint64_t mask) {
            __int128 x = is_signed ? signed_value(a, bits) : (__int128)a;
            __int128 y = is_signed ? signed_value(b, bits) : (__int128)b;
            switch (op) {
                case 0: return (a + b) & mask;
                case 1: return (a - b) & mask;
                case 2: return (a * b) & mask;
                case 3: return (uint64_t)(x / y) & mask;
                case 4: return (uint64_t)(x % y) & mask;
                case 5: return a & b;
                case 6: return a | b;
                case 7: return a ^ b;
                case 8: return b >= bits ? 0 : (a << b) & mask;
                case 9:
                    if (b >= bits) return x < 0 ? mask : 0;
                    return (a >> b) | (x < 0 ? mask ^ (mask >> b) : 0);
                case 10: return x == y;
                case 11: return x != y;
                case 12: return x < y;
                case 13: return x <= y;
                case 14: return x > y;
                default: return x >= y;
            }
        }
        static uint64_t random_bits(uint64_t *state) {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            return *state;
        }
    "#,
    );
    let mut main = String::from("int main(void) {\n");
    for (type_index, &(ty, c_ty, bits, signed)) in INTEGERS.iter().enumerate() {
        for op in 0..16 {
            let name = format!("op_{type_index}_{op}");
            let result_ty = if op >= 10 { TypeId::I1 } else { ty };
            let id = builder
                .declare_function(
                    &name,
                    Linkage::Exported,
                    Abi::C,
                    &[("a", ty), ("b", ty)],
                    result_ty,
                )
                .unwrap();
            let mut fb = builder.function_builder(id).unwrap();
            fb.append_block();
            let (a, b) = (fb.param(0), fb.param(1));
            let value = match op {
                0 => fb.add(a, b),
                1 => fb.sub(a, b),
                2 => fb.mul(a, b),
                3 => fb.div(a, b),
                4 => fb.rem(a, b),
                5 => fb.bit_and(a, b),
                6 => fb.bit_or(a, b),
                7 => fb.bit_xor(a, b),
                8 => fb.shl(a, b),
                9 => fb.shr(a, b),
                10 => fb.eq(a, b),
                11 => fb.ne(a, b),
                12 => fb.lt(a, b),
                13 => fb.le(a, b),
                14 => fb.gt(a, b),
                _ => fb.ge(a, b),
            }
            .unwrap();
            fb.ret(Some(value)).unwrap();
            let c_result = if op >= 10 { "bool" } else { c_ty };
            writeln!(driver, "extern {c_result} {name}({c_ty}, {c_ty});").unwrap();
            let mask = u64::MAX >> (64 - bits);
            writeln!(
                main,
                r#"
                {{
                    const uint64_t mask = UINT64_C({mask});
                    const uint64_t edges[] = {{ 0, 1, 2, 3, mask, mask - 1,
                        mask / 2, mask / 2 + 1, 8, 16, 32, 63, 64, 65,
                        127, 128, 255, 256, UINT64_MAX }};
                    const unsigned n = sizeof(edges) / sizeof(edges[0]);
                    uint64_t state = UINT64_C(0x123456789abcdef);
                    for (unsigned i = 0; i < n * n + 256; ++i) {{
                        uint64_t a = (i < n * n ? edges[i / n] : random_bits(&state)) & mask;
                        uint64_t b = (i < n * n ? edges[i % n] : random_bits(&state)) & mask;
                        if (({op} == 3 || {op} == 4) && b == 0) continue;
                        uint64_t actual = (uint64_t){name}(({c_ty})a, ({c_ty})b) & mask;
                        uint64_t expected = reference({op}, a, b, {bits}, {signed}, mask);
                        if (actual != expected) {{
                            printf("{name}(%llu, %llu): got %llu, expected %llu\n",
                                (unsigned long long)a, (unsigned long long)b,
                                (unsigned long long)actual, (unsigned long long)expected);
                            return 1;
                        }}
                    }}
                }}
            "#
            )
            .unwrap();
        }
    }
    main.push_str("puts(\"ok\"); return 0; }\n");
    driver.push_str(&main);
    run_roundtrip(builder, &driver);
}

#[test]
fn shifts_match_reference_across_count_types() {
    let mut builder = ModuleBuilder::new();
    let mut driver =
        String::from("#include <stdio.h>\n#include <stdint.h>\n#include <stdbool.h>\n");
    let mut main = String::from("int main(void) {\n");
    let mut counts = INTEGERS.to_vec();
    counts.push((TypeId::I1, "bool", 1, false));
    for &(ty, c_ty, bits, signed) in &INTEGERS {
        for &(count_ty, c_count, count_bits, _) in &counts {
            for left in [true, false] {
                let name = format!(
                    "shift_{}_{}_{}",
                    ty.as_u32(),
                    count_ty.as_u32(),
                    u8::from(left)
                );
                let id = builder
                    .declare_function(
                        &name,
                        Linkage::Exported,
                        Abi::C,
                        &[("a", ty), ("count", count_ty)],
                        ty,
                    )
                    .unwrap();
                let mut fb = builder.function_builder(id).unwrap();
                fb.append_block();
                let value = if left {
                    fb.shl(fb.param(0), fb.param(1))
                } else {
                    fb.shr(fb.param(0), fb.param(1))
                }
                .unwrap();
                fb.ret(Some(value)).unwrap();
                writeln!(driver, "extern {c_ty} {name}({c_ty}, {c_count});").unwrap();
                let mask = u64::MAX >> (64 - bits);
                let count_mask = u64::MAX >> (64 - count_bits);
                for a in [0, 1, mask, mask / 2, mask / 2 + 1] {
                    for count in [
                        0,
                        1,
                        bits as u64 - 1,
                        bits as u64,
                        63,
                        64,
                        255,
                        256,
                        u64::MAX,
                    ] {
                        let count = count & count_mask;
                        let negative = signed && a & (1 << (bits - 1)) != 0;
                        let expected = if count >= bits as u64 {
                            if !left && negative {
                                mask
                            } else {
                                0
                            }
                        } else if left {
                            (a << count) & mask
                        } else if negative {
                            let signed_a = (a as i128) - (1i128 << bits);
                            ((signed_a >> count) as u64) & mask
                        } else {
                            a >> count
                        };
                        writeln!(main,
                            "if (((uint64_t){name}(({c_ty})UINT64_C({a}), ({c_count})UINT64_C({count})) & UINT64_C({mask})) != UINT64_C({expected})) {{ puts(\"{name}: a={a} count={count}\"); return 1; }}"
                        ).unwrap();
                    }
                }
            }
        }
    }
    main.push_str("puts(\"ok\"); return 0; }\n");
    driver.push_str(&main);
    run_roundtrip(builder, &driver);
}

#[test]
fn float_integer_conversions_match_rust_casts() {
    let mut builder = ModuleBuilder::new();
    let mut driver = String::from("#include <stdio.h>\n#include <stdint.h>\n#include <string.h>\n");
    let mut main = String::from("int main(void) {\n");
    let mut inputs = vec![
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.0,
        0.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        -0.99,
        0.99,
        1.99,
        -1.99,
    ];
    for exponent in [7, 8, 15, 16, 31, 32, 63, 64] {
        let boundary = 2f64.powi(exponent);
        for x in [
            boundary,
            f64::from_bits(boundary.to_bits() - 1),
            f64::from_bits(boundary.to_bits() + 1),
            boundary - 0.5,
            boundary + 0.5,
        ] {
            inputs.extend([x, -x]);
        }
    }
    let mut state = 0x123456789abcdefu64;
    for _ in 0..256 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        inputs.push(f64::from_bits(state));
    }
    for float_ty in [TypeId::F32, TypeId::F64] {
        let f32 = float_ty == TypeId::F32;
        let c_float = if f32 { "float" } else { "double" };
        let c_bits = if f32 { "uint32_t" } else { "uint64_t" };
        for &(int_ty, c_int, _, _) in &INTEGERS {
            let name = format!("from_float_{}_{}", float_ty.as_u32(), int_ty.as_u32());
            let id = builder
                .declare_function(&name, Linkage::Exported, Abi::C, &[("x", float_ty)], int_ty)
                .unwrap();
            let mut fb = builder.function_builder(id).unwrap();
            fb.append_block();
            let value = fb.float_to_int(int_ty, fb.param(0)).unwrap();
            fb.ret(Some(value)).unwrap();
            writeln!(driver, "extern {c_int} {name}({c_float});").unwrap();
            writeln!(
                main,
                "{{ const struct {{ {c_bits} bits; uint64_t expected; }} cases[] = {{"
            )
            .unwrap();
            for &x in &inputs {
                let (x, bits) = if f32 {
                    ((x as f32) as f64, (x as f32).to_bits() as u64)
                } else {
                    (x, x.to_bits())
                };
                let expected = match int_ty {
                    TypeId::I8 => x as i8 as u64,
                    TypeId::U8 => x as u8 as u64,
                    TypeId::I16 => x as i16 as u64,
                    TypeId::U16 => x as u16 as u64,
                    TypeId::I32 => x as i32 as u64,
                    TypeId::U32 => x as u32 as u64,
                    TypeId::I64 => x as i64 as u64,
                    TypeId::U64 => x as u64,
                    _ => unreachable!(),
                };
                writeln!(main, "{{ UINT64_C({bits}), UINT64_C({expected}) }},").unwrap();
            }
            writeln!(
                main,
                r#"
                }};
                for (unsigned i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {{
                    {c_float} x;
                    memcpy(&x, &cases[i].bits, sizeof(x));
                    if ((uint64_t){name}(x) != cases[i].expected) {{
                        printf("{name}: case %u\n", i); return 1;
                    }}
                }} }}
            "#
            )
            .unwrap();

            let name = format!("to_float_{}_{}", int_ty.as_u32(), float_ty.as_u32());
            let id = builder
                .declare_function(&name, Linkage::Exported, Abi::C, &[("x", int_ty)], float_ty)
                .unwrap();
            let mut fb = builder.function_builder(id).unwrap();
            fb.append_block();
            let value = fb.int_to_float(float_ty, fb.param(0)).unwrap();
            fb.ret(Some(value)).unwrap();
            writeln!(driver, "extern {c_float} {name}({c_int});").unwrap();
            writeln!(
                main,
                "{{ const struct {{ uint64_t input; {c_bits} expected; }} cases[] = {{"
            )
            .unwrap();
            let mut integers = vec![0, 1, u64::MAX, i64::MAX as u64, 1 << 63];
            for bit in 1..64 {
                integers.extend([(1u64 << bit) - 1, 1u64 << bit, (1u64 << bit) + 1]);
            }
            for &input in &integers {
                macro_rules! bits {
                    ($value:expr) => {
                        if f32 {
                            ($value as f32).to_bits() as u64
                        } else {
                            ($value as f64).to_bits()
                        }
                    };
                }
                let expected = match int_ty {
                    TypeId::I8 => bits!(input as i8),
                    TypeId::U8 => bits!(input as u8),
                    TypeId::I16 => bits!(input as i16),
                    TypeId::U16 => bits!(input as u16),
                    TypeId::I32 => bits!(input as i32),
                    TypeId::U32 => bits!(input as u32),
                    TypeId::I64 => bits!(input as i64),
                    TypeId::U64 => bits!(input),
                    _ => unreachable!(),
                };
                writeln!(main, "{{ UINT64_C({input}), UINT64_C({expected}) }},").unwrap();
            }
            writeln!(
                main,
                r#"
                }};
                for (unsigned i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {{
                    {c_float} x = {name}(({c_int})cases[i].input);
                    {c_bits} actual;
                    memcpy(&actual, &x, sizeof(x));
                    if (actual != cases[i].expected) {{
                        printf("{name}: case %u\n", i); return 1;
                    }}
                }} }}
            "#
            )
            .unwrap();
        }
    }
    main.push_str("puts(\"ok\"); return 0; }\n");
    driver.push_str(&main);
    run_roundtrip(builder, &driver);
}

#[test]
fn interleaved_integer_and_float_stack_arguments_follow_system_v() {
    let mut builder = ModuleBuilder::new();
    // Both register classes overflow, with f32/f64 and narrow integers on the stack.
    let types = [
        (TypeId::I8, "int8_t", "-101"),
        (TypeId::F32, "float", "1.25"),
        (TypeId::U32, "uint32_t", "4000000000u"),
        (TypeId::F64, "double", "-2.5"),
        (TypeId::I64, "int64_t", "-5000000000LL"),
        (TypeId::F32, "float", "3.75"),
        (TypeId::U16, "uint16_t", "60000"),
        (TypeId::F64, "double", "-4.5"),
        (TypeId::I16, "int16_t", "-30000"),
        (TypeId::F32, "float", "5.25"),
        (TypeId::U64, "uint64_t", "9000000000ULL"),
        (TypeId::F64, "double", "-6.75"),
        (TypeId::I32, "int32_t", "-2000000000"),
        (TypeId::F32, "float", "7.5"),
        (TypeId::I8, "int8_t", "-123"),
        (TypeId::F64, "double", "-8.25"),
        (TypeId::I64, "int64_t", "-10000000000LL"),
        (TypeId::F32, "float", "9.75"),
        (TypeId::U8, "uint8_t", "251"),
        (TypeId::F64, "double", "-10.5"),
    ];
    let ir_types: Vec<_> = types.iter().map(|t| t.0).collect();
    let external = builder
        .declare_extern("check", Abi::C, &ir_types, false, TypeId::I32)
        .unwrap();
    let names: Vec<_> = (0..types.len()).map(|i| format!("p{i}")).collect();
    let params: Vec<_> = names
        .iter()
        .zip(&ir_types)
        .map(|(name, ty)| (name.as_str(), *ty))
        .collect();
    let id = builder
        .declare_function("forward", Linkage::Exported, Abi::C, &params, TypeId::I32)
        .unwrap();
    let mut fb = builder.function_builder(id).unwrap();
    fb.append_block();
    // Call C twice, checking that arguments and locals survive a call's
    // register clobbers.
    let args: Vec<_> = (0..types.len()).map(|i| fb.param(i)).collect();
    let a = fb.call(external, &args).unwrap().unwrap();
    let b = fb.call(external, &args).unwrap().unwrap();
    let result = fb.add(a, b).unwrap();
    fb.ret(Some(result)).unwrap();
    let c_params = types
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{} p{i}", t.1))
        .collect::<Vec<_>>()
        .join(", ");
    let values = types.iter().map(|t| t.2).collect::<Vec<_>>().join(", ");
    let checks = types
        .iter()
        .enumerate()
        .map(|(i, t)| format!("p{i} == ({})({})", t.1, t.2))
        .collect::<Vec<_>>()
        .join(" && ");
    let driver = format!(
        r#"
        #include <stdint.h>
        #include <stdio.h>
        int32_t check({c_params}) {{ return {checks}; }}
        extern int32_t forward({c_params});
        int main(void) {{
            if (forward({values}) != 2) {{ puts("argument mismatch"); return 1; }}
            puts("ok"); return 0;
        }}
    "#
    );
    run_roundtrip(builder, &driver);
}
