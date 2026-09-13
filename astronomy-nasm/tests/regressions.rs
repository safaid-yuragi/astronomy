//! Boundary cases checked through the complete ARN -> NASM -> executable pipeline.

mod common;

use astronomy::{Abi, Linkage, Module, ModuleBuilder, TypeId, Verifier};
use common::{assert_prints, compile_err, one_function};

fn roundtrip(module: Module) -> Module {
    let verified = Verifier::verify(module).unwrap();
    Module::parse_arn(&verified.to_arn()).unwrap()
}

#[test]
fn signed_min_divided_by_minus_one_wraps() {
    let module = one_function(
        "quotient",
        &[("a", TypeId::I64), ("b", TypeId::I64)],
        TypeId::I64,
        |fb| {
            let value = fb.div(fb.param(0), fb.param(1)).unwrap();
            fb.ret(Some(value)).unwrap();
        },
    );
    assert_prints(
        roundtrip(module),
        r#"
        #include <stdio.h>
        #include <stdint.h>
        extern int64_t quotient(int64_t, int64_t);
        int main(void) {
            printf("%lld %lld %lld\n", (long long)quotient(INT64_MIN, -1),
                (long long)quotient(INT64_MIN, 1), (long long)quotient(7, -2));
            return 0;
        }
    "#,
        "-9223372036854775808 -9223372036854775808 -3",
    );
}

#[test]
fn signed_min_remainder_minus_one_is_zero() {
    let module = one_function(
        "remainder",
        &[("a", TypeId::I64), ("b", TypeId::I64)],
        TypeId::I64,
        |fb| {
            let value = fb.rem(fb.param(0), fb.param(1)).unwrap();
            fb.ret(Some(value)).unwrap();
        },
    );
    assert_prints(
        roundtrip(module),
        r#"
        #include <stdio.h>
        #include <stdint.h>
        extern int64_t remainder(int64_t, int64_t);
        int main(void) {
            printf("%lld %lld\n", (long long)remainder(INT64_MIN, -1),
                (long long)remainder(-7, 2));
            return 0;
        }
    "#,
        "0 -1",
    );
}

#[test]
fn shifts_accept_counts_of_a_different_integer_type() {
    let module = one_function(
        "shift",
        &[("a", TypeId::I8), ("count", TypeId::U64)],
        TypeId::I8,
        |fb| {
            let value = fb.shl(fb.param(0), fb.param(1)).unwrap();
            fb.ret(Some(value)).unwrap();
        },
    );
    assert_prints(
        roundtrip(module),
        r#"
        #include <stdio.h>
        #include <stdint.h>
        extern int8_t shift(int8_t, uint64_t);
        int main(void) {
            printf("%d %d %d %d\n", shift(3, 1), shift(3, 256),
                shift(3, UINT64_MAX), shift(3, 8));
            return 0;
        }
    "#,
        "6 0 0 0",
    );
}

#[test]
fn shifts_load_only_the_count_width() {
    let module = one_function(
        "shift",
        &[("a", TypeId::I64), ("count", TypeId::I1)],
        TypeId::I64,
        |fb| {
            let value = fb.shr(fb.param(0), fb.param(1)).unwrap();
            fb.ret(Some(value)).unwrap();
        },
    );
    assert_prints(
        roundtrip(module),
        r#"
        #include <stdio.h>
        #include <stdbool.h>
        extern long long shift(long long, bool);
        int main(void) {
            printf("%lld %lld\n", shift(-8, true), shift(-8, false));
            return 0;
        }
    "#,
        "-4 -8",
    );
}

#[test]
fn pointer_offsets_are_signed_even_for_unsigned_types() {
    for (ty, c_ty, max) in [
        (TypeId::U8, "uint8_t", "UINT8_MAX"),
        (TypeId::U16, "uint16_t", "UINT16_MAX"),
        (TypeId::U32, "uint32_t", "UINT32_MAX"),
        (TypeId::U64, "uint64_t", "UINT64_MAX"),
    ] {
        let mut builder = ModuleBuilder::new();
        let ptr = builder.ptr_type(TypeId::I64);
        let id = builder
            .declare_function(
                "offset",
                Linkage::Exported,
                Abi::C,
                &[("p", ptr), ("n", ty)],
                ptr,
            )
            .unwrap();
        let mut fb = builder.function_builder(id).unwrap();
        fb.append_block();
        let value = fb.ptr_offset(fb.param(0), fb.param(1)).unwrap();
        fb.ret(Some(value)).unwrap();
        let driver = format!(
            r#"
            #include <stdio.h>
            #include <stdint.h>
            extern int64_t *offset(int64_t *, {c_ty});
            int main(void) {{
                int64_t a[4] = {{ 0 }};
                printf("%d %d\n", offset(a + 2, {max}) == a + 1,
                    offset(a + 2, 1) == a + 3);
                return 0;
            }}
        "#
        );
        assert_prints(roundtrip(builder.finish()), &driver, "1 1");
    }
}

#[test]
fn pointer_offsets_support_strides_larger_than_signed_immediates() {
    let mut builder = ModuleBuilder::new();
    let array = builder.array_type(TypeId::U8, 1 << 31);
    let ptr = builder.ptr_type(array);
    let id = builder
        .declare_function(
            "offset",
            Linkage::Exported,
            Abi::C,
            &[("n", TypeId::I64)],
            ptr,
        )
        .unwrap();
    let mut fb = builder.function_builder(id).unwrap();
    fb.append_block();
    let null = fb.const_null(array).unwrap();
    let value = fb.ptr_offset(null, fb.param(0)).unwrap();
    fb.ret(Some(value)).unwrap();
    assert_prints(
        roundtrip(builder.finish()),
        r#"
        #include <stdio.h>
        #include <stdint.h>
        extern void *offset(int64_t);
        int main(void) {
            printf("%d %d\n", (uintptr_t)offset(1) == UINT64_C(2147483648),
                (uintptr_t)offset(-1) == UINT64_MAX - UINT64_C(2147483647));
            return 0;
        }
    "#,
        "1 1",
    );
}

#[test]
fn empty_aggregate_can_be_stored_loaded_and_copied() {
    let mut builder = ModuleBuilder::new();
    let empty = builder.struct_type(&[]);
    let id = builder
        .declare_function("empty", Linkage::Exported, Abi::C, &[], TypeId::VOID)
        .unwrap();
    let mut fb = builder.function_builder(id).unwrap();
    fb.append_block();
    let value = fb.construct(empty, &[]).unwrap();
    let ptr = fb.alloca(empty).unwrap();
    fb.store(ptr, value).unwrap();
    let loaded = fb.load(ptr).unwrap();
    let entry = fb.current_block().unwrap();
    let next = fb.append_block_with_params(&[("v", empty)]).unwrap();
    fb.switch_to(entry).unwrap();
    fb.jump(next, &[loaded]).unwrap();
    fb.switch_to(next).unwrap();
    fb.ret_void().unwrap();
    assert_prints(
        roundtrip(builder.finish()),
        r#"
        #include <stdio.h>
        extern void empty(void);
        int main(void) { empty(); puts("ok"); return 0; }
    "#,
        "ok",
    );
}

#[test]
fn function_names_can_be_nasm_keywords() {
    let mut builder = ModuleBuilder::new();
    let helper = builder
        .declare_function("rax", Linkage::Internal, Abi::C, &[], TypeId::I64)
        .unwrap();
    let external = builder
        .declare_extern("xmm0", Abi::C, &[TypeId::I64], false, TypeId::I64)
        .unwrap();
    let mut fb = builder.function_builder(helper).unwrap();
    fb.append_block();
    let value = fb.const_int(TypeId::I64, 41).unwrap();
    fb.ret(Some(value)).unwrap();
    let id = builder
        .declare_function("byte", Linkage::Exported, Abi::C, &[], TypeId::I64)
        .unwrap();
    let mut fb = builder.function_builder(id).unwrap();
    fb.append_block();
    let a = fb.call(helper, &[]).unwrap().unwrap();
    let b = fb.call(external, &[a]).unwrap().unwrap();
    fb.ret(Some(b)).unwrap();
    assert_prints(
        roundtrip(builder.finish()),
        r#"
        #include <stdio.h>
        long long xmm0(long long x) { return x + 1; }
        extern long long byte(void);
        int main(void) { printf("%lld\n", byte()); return 0; }
    "#,
        "42",
    );
}

#[test]
fn oversized_stack_frames_fail_with_a_structured_error() {
    for length in [1 << 31, u64::MAX] {
        let mut builder = ModuleBuilder::new();
        let array = builder.array_type(TypeId::U8, length);
        let id = builder
            .declare_function("huge", Linkage::Exported, Abi::C, &[], TypeId::VOID)
            .unwrap();
        let mut fb = builder.function_builder(id).unwrap();
        fb.append_block();
        fb.alloca(array).unwrap();
        fb.ret_void().unwrap();
        let error = compile_err(roundtrip(builder.finish()));
        assert_eq!(error.code(), "A-NASM-001");
    }
}

#[test]
fn overflowing_struct_layout_fails_with_a_structured_error() {
    let mut builder = ModuleBuilder::new();
    let huge = builder.array_type(TypeId::U8, u64::MAX);
    let aggregate = builder.struct_type(&[huge, TypeId::U64]);
    let id = builder
        .declare_function("huge", Linkage::Exported, Abi::C, &[], TypeId::VOID)
        .unwrap();
    let mut fb = builder.function_builder(id).unwrap();
    fb.append_block();
    fb.alloca(aggregate).unwrap();
    fb.ret_void().unwrap();
    let error = compile_err(roundtrip(builder.finish()));
    assert_eq!(error.code(), "A-NASM-001");
}
