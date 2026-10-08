//! Execution tests for memory (`alloca`/`load`/`store`/`ptr_offset`),
//! aggregates (`construct`/`extract`/`insert`, aggregate constants),
//! pointer casts and string constants.

mod common;

use astronomy::{Abi, ConstantData, Linkage, ModuleBuilder, TypeId};
use common::{assert_prints, one_function};

const I64: TypeId = TypeId::I64;
const I32: TypeId = TypeId::I32;

fn int_data(ty: TypeId, width: u32, bits: u128) -> ConstantData {
    ConstantData::Int { ty, width, bits }
}

#[test]
fn alloca_store_load_i64() {
    let m = one_function("f", &[("x", I64)], I64, |fb| {
        let x = fb.param(0);
        let slot = fb.alloca(I64).unwrap();
        fb.store(slot, x).unwrap();
        let v = fb.load(slot).unwrap();
        fb.ret(Some(v)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long);\nint main(void){ printf(\"%lld\\n\", f(123456789)); return 0; }",
        "123456789",
    );
}

#[test]
fn alloca_store_load_i32_and_f64() {
    let m = one_function("f", &[("x", I32), ("y", TypeId::F64)], TypeId::F64, |fb| {
        let (x, y) = (fb.param(0), fb.param(1));
        let si = fb.alloca(I32).unwrap();
        fb.store(si, x).unwrap();
        let back = fb.load(si).unwrap();
        let fl = fb.int_to_float(TypeId::F64, back).unwrap();
        let sd = fb.alloca(TypeId::F64).unwrap();
        fb.store(sd, y).unwrap();
        let y2 = fb.load(sd).unwrap();
        let s = fb.add(fl, y2).unwrap();
        fb.ret(Some(s)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern double f(int, double);\nint main(void){ printf(\"%.1f\\n\", f(40, 2.5)); return 0; }",
        "42.5",
    );
}

#[test]
fn ptr_offset_reads_neighbouring_slots() {
    let m = one_function("f", &[("a", I64), ("b", I64)], I64, |fb| {
        let (a, b) = (fb.param(0), fb.param(1));
        let base = fb.alloca(I64).unwrap();
        fb.store(base, a).unwrap();
        let one = fb.const_int(I64, 1).unwrap();
        let p1 = fb.ptr_offset(base, one).unwrap();
        fb.store(p1, b).unwrap();
        let minus_one = fb.const_int(I64, -1).unwrap();
        let back = fb.ptr_offset(p1, minus_one).unwrap();
        let first = fb.load(back).unwrap();
        let second = fb.load(p1).unwrap();
        let sum = fb.add(first, second).unwrap();
        fb.ret(Some(sum)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(11, 31)); return 0; }",
        "42",
    );
}

#[test]
fn struct_construct_extract_insert() {
    let mut b = ModuleBuilder::with_name("s");
    let pair = b.struct_type(&[I64, I64]);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("a", I64), ("b", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let (a, bv) = (fb.param(0), fb.param(1));
    let p = fb.construct(pair, &[a, bv]).unwrap();
    let x = fb.extract(p, 0).unwrap();
    let p2 = fb.insert(p, 0, bv).unwrap();
    let y = fb.extract(p2, 0).unwrap();
    let s = fb.add(x, y).unwrap();
    fb.ret(Some(s)).unwrap();

    // a + b
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(19, 23)); return 0; }",
        "42",
    );
}

#[test]
fn nested_struct_extract() {
    let mut b = ModuleBuilder::with_name("n");
    let inner = b.struct_type(&[I64, I64]);
    let outer = b.struct_type(&[I64, inner]);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("a", I64), ("b", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let (a, bv) = (fb.param(0), fb.param(1));
    let i = fb.construct(inner, &[bv, a]).unwrap();
    let o = fb.construct(outer, &[a, i]).unwrap();
    let got_i = fb.extract(o, 1).unwrap();
    let got = fb.extract(got_i, 0).unwrap();
    fb.ret(Some(got)).unwrap();

    // extract outer.1.0 == b
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(1, 99)); return 0; }",
        "99",
    );
}

#[test]
fn array_construct_extract_insert() {
    let mut b = ModuleBuilder::with_name("a");
    let arr = b.array_type(I64, 3);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("a", I64), ("b", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let (a, bv) = (fb.param(0), fb.param(1));
    let v = fb.construct(arr, &[a, bv, a]).unwrap();
    let w = fb.insert(v, 2, bv).unwrap();
    let x = fb.extract(w, 2).unwrap();
    let y = fb.extract(w, 0).unwrap();
    let s = fb.add(x, y).unwrap();
    fb.ret(Some(s)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(40, 2)); return 0; }",
        "42",
    );
}

#[test]
fn string_bytes_via_load() {
    let m = one_function("f", &[], I32, |fb| {
        let s = fb.const_string(b"AB").unwrap();
        let c = fb.load(s).unwrap();
        let w = fb.ext(TypeId::I32, c).unwrap();
        fb.ret(Some(w)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\nextern int f(void);\nint main(void){ printf(\"%d\\n\", f()); return 0; }",
        "65",
    );
}

#[test]
fn ptr_cast_narrows_access_width() {
    let mut b = ModuleBuilder::with_name("p");
    let i32_ptr = b.ptr_type(I32);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("x", I64)], I32)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let x = fb.param(0);
    let slot = fb.alloca(I64).unwrap();
    fb.store(slot, x).unwrap();
    let narrow = fb.ptr_cast(i32_ptr, slot).unwrap();
    let low = fb.load(narrow).unwrap();
    fb.ret(Some(low)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern int f(long long);\nint main(void){ printf(\"%d\\n\", f(4294967298LL)); return 0; }",
        "2",
    );
}

#[test]
fn i1_round_trips_through_memory() {
    let m = one_function("f", &[("b", TypeId::I1)], TypeId::I1, |fb| {
        let b = fb.param(0);
        let slot = fb.alloca(TypeId::I1).unwrap();
        fb.store(slot, b).unwrap();
        let v = fb.load(slot).unwrap();
        fb.ret(Some(v)).unwrap();
    });
    assert_prints(
        m,
        "#include <stdio.h>\n#include <stdbool.h>\nextern bool f(bool);\nint main(void){ printf(\"%d %d\\n\", (int)f(true), (int)f(false)); return 0; }",
        "1 0",
    );
}

#[test]
fn struct_constant_materializes() {
    let mut b = ModuleBuilder::with_name("aggconst");
    let pair = b.struct_type(&[I64, I64]);
    let c0 = b.intern_constant_data(int_data(I64, 64, 7));
    let c1 = b.intern_constant_data(int_data(I64, 64, 9));
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let agg = fb.const_aggregate(pair, &[c0, c1]).unwrap();
    let x = fb.extract(agg, 0).unwrap();
    let y = fb.extract(agg, 1).unwrap();
    let s = fb.add(x, y).unwrap();
    fb.ret(Some(s)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(void);\nint main(void){ printf(\"%lld\\n\", f()); return 0; }",
        "16",
    );
}

#[test]
fn array_constant_materializes() {
    let mut b = ModuleBuilder::with_name("arrconst");
    let arr = b.array_type(I64, 3);
    let c0 = b.intern_constant_data(int_data(I64, 64, 1));
    let c1 = b.intern_constant_data(int_data(I64, 64, 2));
    let c2 = b.intern_constant_data(int_data(I64, 64, 3));
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let agg = fb.const_aggregate(arr, &[c0, c1, c2]).unwrap();
    let x = fb.extract(agg, 0).unwrap();
    let y = fb.extract(agg, 2).unwrap();
    let s = fb.add(x, y).unwrap();
    fb.ret(Some(s)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(void);\nint main(void){ printf(\"%lld\\n\", f()); return 0; }",
        "4",
    );
}

#[test]
fn nested_aggregate_constant_materializes() {
    let mut b = ModuleBuilder::with_name("nestconst");
    let inner = b.struct_type(&[I64, I64]);
    let outer = b.struct_type(&[I64, inner]);
    let c1 = b.intern_constant_data(int_data(I64, 64, 1));
    let c2 = b.intern_constant_data(int_data(I64, 64, 2));
    let c3 = b.intern_constant_data(int_data(I64, 64, 3));
    let inner_c = b.intern_constant_data(ConstantData::Aggregate {
        ty: inner,
        elements: vec![c2, c3],
    });
    let outer_c = b.intern_constant_data(ConstantData::Aggregate {
        ty: outer,
        elements: vec![c1, inner_c],
    });
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    // Re-materialize through constant instructions using the pool entries.
    let agg = fb.const_aggregate(outer, &[c1, inner_c]).unwrap();
    let _ = outer_c;
    let got_inner = fb.extract(agg, 1).unwrap();
    let got = fb.extract(got_inner, 1).unwrap();
    fb.ret(Some(got)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(void);\nint main(void){ printf(\"%lld\\n\", f()); return 0; }",
        "3",
    );
}

#[test]
fn aggregate_round_trips_through_memory() {
    let mut b = ModuleBuilder::with_name("aggmem");
    let pair = b.struct_type(&[I64, I64]);
    let id = b
        .declare_function("f", Linkage::Exported, Abi::C, &[("a", I64), ("b", I64)], I64)
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let (a, bv) = (fb.param(0), fb.param(1));
    let p = fb.construct(pair, &[a, bv]).unwrap();
    let slot = fb.alloca(pair).unwrap();
    fb.store(slot, p).unwrap();
    let q = fb.load(slot).unwrap();
    let x = fb.extract(q, 0).unwrap();
    let y = fb.extract(q, 1).unwrap();
    let s = fb.add(x, y).unwrap();
    fb.ret(Some(s)).unwrap();

    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long, long long);\nint main(void){ printf(\"%lld\\n\", f(17, 25)); return 0; }",
        "42",
    );
}

#[test]
fn struct_pointer_offset_walks_array() {
    let mut b = ModuleBuilder::with_name("aggwalk");
    let pair = b.struct_type(&[I64, I64]);
    let id = b
        .declare_function(
            "f",
            Linkage::Exported,
            Abi::C,
            &[("a", I64), ("b", I64), ("c", I64), ("d", I64)],
            I64,
        )
        .unwrap();
    let mut fb = b.function_builder(id).unwrap();
    fb.append_block();
    let (a, bv, c, d) = (fb.param(0), fb.param(1), fb.param(2), fb.param(3));
    let base = fb.alloca(pair).unwrap();
    let p0 = fb.construct(pair, &[a, bv]).unwrap();
    fb.store(base, p0).unwrap();
    let one = fb.const_int(I64, 1).unwrap();
    let p1 = fb.ptr_offset(base, one).unwrap();
    let p2 = fb.construct(pair, &[c, d]).unwrap();
    fb.store(p1, p2).unwrap();
    let q = fb.load(p1).unwrap();
    let x = fb.extract(q, 0).unwrap();
    let y = fb.extract(q, 1).unwrap();
    let s = fb.add(x, y).unwrap();
    fb.ret(Some(s)).unwrap();

    // f(1,2,10,32) -> second struct (10,32) -> 42
    assert_prints(
        b.finish(),
        "#include <stdio.h>\nextern long long f(long long,long long,long long,long long);\nint main(void){ printf(\"%lld\\n\", f(1,2,10,32)); return 0; }",
        "42",
    );
}
