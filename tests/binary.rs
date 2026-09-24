//! `.arb` binary format tests (§16): lossless roundtrips, determinism, and
//! structured rejection of malformed input.

use astronomy::binary::{self, FORMAT_MAJOR, MAGIC};
use astronomy::{
    Abi, ArbError, BasicBlock, BlockId, ConstantData, Instruction, InstructionKind, Linkage,
    Module, ModuleBuilder, TypeId, ValueData, ValueKind, Verifier,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn build_block_args() -> Module {
    let mut b = ModuleBuilder::with_name("block-args");
    let pick = b
        .declare_function(
            "pick",
            Linkage::Exported,
            Abi::Astronomy,
            &[("cond", TypeId::I1), ("a", TypeId::I64), ("b", TypeId::I64)],
            TypeId::I64,
        )
        .unwrap();
    let mut fb = b.function_builder(pick).unwrap();
    let (cond, a, b_) = (fb.param(0), fb.param(1), fb.param(2));
    let entry = fb.append_named_block("entry").unwrap();
    let left = fb.append_block_with_params(&[("x", TypeId::I64)]).unwrap();
    let right = fb.append_block_with_params(&[("x", TypeId::I64)]).unwrap();
    let merge = fb
        .append_block_with_params(&[("result", TypeId::I64)])
        .unwrap();
    fb.switch_to(entry).unwrap();
    fb.branch(cond, left, &[a], right, &[b_]).unwrap();
    let x = fb.block_param(left, 0).unwrap();
    fb.switch_to(left).unwrap();
    fb.jump(merge, &[x]).unwrap();
    let x = fb.block_param(right, 0).unwrap();
    fb.switch_to(right).unwrap();
    fb.jump(merge, &[x]).unwrap();
    let result = fb.block_param(merge, 0).unwrap();
    fb.switch_to(merge).unwrap();
    fb.ret(Some(result)).unwrap();
    b.finish()
}

/// Exercises every instruction, terminator, constant and type kind.
fn build_kitchen_sink() -> Module {
    let mut b = ModuleBuilder::with_name("kitchen-sink");
    let pair_ty = b.struct_type(&[TypeId::I64, TypeId::I64]);
    let arr_ty = b.array_type(TypeId::U8, 3);
    let byte_ptr = b.ptr_type(TypeId::I8);
    let _fn_ty = b.fn_type(&[byte_ptr], true, TypeId::I32);
    let custom = b.intern_symbol("fastcall");

    let printf = b
        .declare_extern("printf", Abi::C, &[byte_ptr], true, TypeId::I32)
        .unwrap();
    let helper = b
        .declare_function(
            "helper",
            Linkage::Internal,
            Abi::Custom(custom),
            &[("v", TypeId::U128)],
            TypeId::VOID,
        )
        .unwrap();
    {
        let mut fb = b.function_builder(helper).unwrap();
        fb.append_block();
        fb.ret_void().unwrap();
    }

    let f = b
        .declare_function(
            "sink",
            Linkage::Exported,
            Abi::System,
            &[("a", TypeId::I64), ("b", TypeId::I64), ("f", TypeId::F32)],
            TypeId::I64,
        )
        .unwrap();
    let mut fb = b.function_builder(f).unwrap();
    let (a, b_, fl) = (fb.param(0), fb.param(1), fb.param(2));
    let entry = fb.append_named_block("entry").unwrap();
    let dead = fb.append_named_block("dead").unwrap();
    let exit = fb.append_block_with_params(&[("r", TypeId::I64)]).unwrap();
    fb.switch_to(entry).unwrap();

    let slot = fb.alloca(TypeId::I64).unwrap();
    fb.store(slot, a).unwrap();
    let loaded = fb.load(slot).unwrap();
    let one = fb.const_int(TypeId::I64, 1).unwrap();
    let slot2 = fb.ptr_offset(slot, one).unwrap();
    fb.store(slot2, b_).unwrap();

    let mut acc = loaded;
    for op in 0..10 {
        acc = match op {
            0 => fb.add(acc, b_),
            1 => fb.sub(acc, b_),
            2 => fb.mul(acc, b_),
            3 => fb.div(acc, one),
            4 => fb.rem(acc, b_),
            5 => fb.bit_and(acc, b_),
            6 => fb.bit_or(acc, b_),
            7 => fb.bit_xor(acc, b_),
            8 => fb.shl(acc, one),
            _ => fb.shr(acc, one),
        }
        .unwrap();
    }
    let c1 = fb.eq(acc, b_).unwrap();
    let c2 = fb.ne(acc, b_).unwrap();
    let c3 = fb.lt(acc, b_).unwrap();
    let c4 = fb.le(acc, b_).unwrap();
    let c5 = fb.gt(acc, b_).unwrap();
    let c6 = fb.ge(acc, b_).unwrap();
    let c = fb.bit_and(c1, c2).unwrap();
    let c = fb.bit_or(c, c3).unwrap();
    let c = fb.bit_xor(c, c4).unwrap();
    let c = fb.bit_and(c, c5).unwrap();
    let c = fb.bit_or(c, c6).unwrap();

    let pair = fb.construct(pair_ty, &[acc, b_]).unwrap();
    let x = fb.extract(pair, 1).unwrap();
    let pair = fb.insert(pair, 0, x).unwrap();
    let y = fb.extract(pair, 0).unwrap();

    let wide = fb.ext(TypeId::I128, y).unwrap();
    let narrow = fb.trunc(TypeId::I32, wide).unwrap();
    let as_f = fb.int_to_float(TypeId::F64, narrow).unwrap();
    let back = fb.float_to_int(TypeId::I64, as_f).unwrap();
    let _ = fb.int_to_float(TypeId::F32, back).unwrap();
    let _ = fb.add(fl, fl).unwrap();
    let _ = fb.ptr_cast(byte_ptr, slot).unwrap();
    let _ = fb.const_null(TypeId::I64).unwrap();
    let msg = fb.const_string(b"esc\"aped\n\x00\xff").unwrap();
    let _ = fb.call(printf, &[msg, narrow]).unwrap();
    let big = fb.const_uint(TypeId::U128, u128::MAX).unwrap();
    let _ = fb.call(helper, &[big]).unwrap();
    let _ = fb.const_f64(f64::NAN).unwrap();
    let _ = fb.const_f32(-0.0).unwrap();
    let _ = fb.const_int(TypeId::I8, -128).unwrap();

    fb.branch(c, exit, &[back], dead, &[]).unwrap();
    fb.switch_to(dead).unwrap();
    fb.unreachable().unwrap();
    let r = fb.block_param(exit, 0).unwrap();
    fb.switch_to(exit).unwrap();
    fb.ret(Some(r)).unwrap();

    // An aggregate constant referencing earlier pool entries.
    let mut module = b.finish();
    let elems: Vec<_> = (1u128..=3)
        .map(|v| {
            module.intern_constant(ConstantData::Int {
                ty: TypeId::U8,
                width: 8,
                bits: v,
            })
        })
        .collect();
    module.intern_constant(ConstantData::Aggregate {
        ty: arr_ty,
        elements: elems,
    });
    module
}

fn fixtures() -> Vec<Module> {
    let hello = Module::parse_arn(include_str!("../examples/hello.arn")).unwrap();
    vec![
        Module::new(),
        build_block_args(),
        build_kitchen_sink(),
        strip_spans(hello),
    ]
}

/// Spans point into `.arn` source text and are intentionally not stored.
fn strip_spans(mut module: Module) -> Module {
    for f in module.functions_mut() {
        f.span = None;
        for v in &mut f.values {
            v.span = None;
        }
        for b in &mut f.blocks {
            b.span = None;
            for i in &mut b.instructions {
                i.span = None;
            }
        }
    }
    module
}

// ---------------------------------------------------------------------------
// Roundtrip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_is_lossless() {
    for module in fixtures() {
        let bytes = module.to_arb();
        let decoded = Module::from_arb(&bytes).expect("decoding must succeed");
        assert_eq!(decoded, module, "decoded module must equal the original");
    }
}

#[test]
fn roundtrip_preserves_verification_and_text() {
    for module in fixtures() {
        let verified = Verifier::verify(module).expect("fixture must verify");
        let decoded = Module::from_arb(&verified.to_arb()).unwrap();
        let reverified = decoded.verify().expect("decoded module must verify");
        assert_eq!(reverified.to_arn(), verified.to_arn());
    }
}

#[test]
fn encoding_is_deterministic_and_stable() {
    for module in fixtures() {
        let a = binary::write(&module);
        let b = binary::write(&module.clone());
        assert_eq!(a, b);
        let again = binary::write(&binary::read(&a).unwrap());
        assert_eq!(a, again, "write(read(bytes)) must reproduce the bytes");
    }
}

#[test]
fn parsed_arn_roundtrips_through_arb() {
    let src = include_str!("../examples/hello.arn");
    let parsed = Module::parse_arn(src).unwrap();
    let decoded = Module::from_arb(&parsed.to_arb()).unwrap();
    assert_eq!(decoded.to_arn(), parsed.to_arn());
    assert_eq!(decoded.name(), Some("hello"));
    assert!(decoded.verify().is_ok());
}

#[test]
fn unverified_modules_roundtrip_faithfully() {
    // Missing terminator and a reserved value: invalid, but representable.
    let mut module = Module::new();
    let sym = module.symbols_mut().intern("broken");
    let mut f = astronomy::Function::new(
        sym,
        Linkage::Internal,
        Abi::Astronomy,
        vec![],
        TypeId::VOID,
        false,
    );
    f.values
        .push(ValueData::new(TypeId::I64, ValueKind::Reserved));
    f.values.push(ValueData::new(
        TypeId::I64,
        ValueKind::Inst {
            block: BlockId::new(7),
            index: 3,
        },
    ));
    let mut bb = BasicBlock::new();
    bb.instructions.push(Instruction::new(
        InstructionKind::Add {
            lhs: astronomy::ValueId::new(0),
            rhs: astronomy::ValueId::new(99),
        },
        Some(astronomy::ValueId::new(1)),
    ));
    f.blocks.push(bb);
    module.functions_mut().push(f);

    let decoded = Module::from_arb(&module.to_arb()).unwrap();
    assert_eq!(decoded, module);
    let original = Verifier::verify(module).unwrap_err();
    let again = Verifier::verify(decoded).unwrap_err();
    assert_eq!(original, again, "the verifier must see the same problems");
}

#[test]
fn file_starts_with_magic() {
    let bytes = Module::new().to_arb();
    assert!(bytes.starts_with(&MAGIC));
    assert!(binary::is_arb(&bytes));
    assert!(!binary::is_arb(b"::ASTRONOMY::MODULE_START"));
}

// ---------------------------------------------------------------------------
// Malformed input
// ---------------------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Recomputes the trailing checksum after a deliberate mutation.
fn reseal(bytes: &mut [u8]) {
    let end = bytes.len() - 4;
    let crc = crc32(&bytes[..end]);
    bytes[end..].copy_from_slice(&crc.to_le_bytes());
}

/// Assembles a file from raw section bodies.
fn assemble(sections: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&FORMAT_MAJOR.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for (tag, body) in sections {
        out.extend_from_slice(&tag[..]);
        out.extend_from_slice(&(body.len() as u64).to_le_bytes());
        out.extend_from_slice(body);
    }
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

fn le(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Section bodies of an empty module: (MODL, SYMB, TYPE, CNST, FUNC).
fn empty_sections() -> Vec<(&'static [u8; 4], Vec<u8>)> {
    let bytes = Module::new().to_arb();
    let mut pos = 16;
    let mut out = Vec::new();
    for tag in [b"MODL", b"SYMB", b"TYPE", b"CNST", b"FUNC"] {
        assert_eq!(&bytes[pos..pos + 4], tag);
        let len = u64::from_le_bytes(bytes[pos + 4..pos + 12].try_into().unwrap()) as usize;
        out.push((tag, bytes[pos + 12..pos + 12 + len].to_vec()));
        pos += 12 + len;
    }
    out
}

/// A type table with the 14 scalars followed by `extra` raw entries.
fn type_table(extra: &[&[u8]]) -> Vec<u8> {
    let sections = empty_sections();
    let mut body = sections[2].1.clone();
    let count = 14 + extra.len() as u32;
    body[..4].copy_from_slice(&count.to_le_bytes());
    for e in extra {
        body.extend_from_slice(e);
    }
    body
}

fn with_section(index: usize, body: Vec<u8>) -> Vec<u8> {
    let mut sections = empty_sections();
    sections[index].1 = body;
    assemble(&sections)
}

#[test]
fn assemble_reproduces_writer_output() {
    assert_eq!(assemble(&empty_sections()), Module::new().to_arb());
}

#[test]
fn rejects_bad_magic() {
    assert_eq!(Module::from_arb(b"").unwrap_err(), ArbError::BadMagic);
    let err = Module::from_arb(b"::ASTRONOMY::MODULE_START\n").unwrap_err();
    assert_eq!(err.code(), "A-ARB-001");
}

#[test]
fn rejects_unknown_format_major() {
    let mut bytes = Module::new().to_arb();
    bytes[8..10].copy_from_slice(&(FORMAT_MAJOR + 1).to_le_bytes());
    let err = Module::from_arb(&bytes).unwrap_err();
    assert_eq!(err.code(), "A-ARB-002", "{err}");
}

#[test]
fn rejects_unknown_module_major() {
    let mut sections = empty_sections();
    sections[0].1[..4].copy_from_slice(&2u32.to_le_bytes());
    let err = Module::from_arb(&assemble(&sections)).unwrap_err();
    assert_eq!(err.code(), "A-ARB-003", "{err}");
}

#[test]
fn rejects_checksum_mismatch() {
    let mut bytes = build_kitchen_sink().to_arb();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x40;
    let err = Module::from_arb(&bytes).unwrap_err();
    assert_eq!(err.code(), "A-ARB-005", "{err}");
}

#[test]
fn rejects_every_truncation() {
    let bytes = build_kitchen_sink().to_arb();
    for len in 0..bytes.len() {
        let err = Module::from_arb(&bytes[..len]).expect_err("truncated input must fail");
        let _ = err.to_string();
    }
    // Truncation with a valid checksum reaches the structural checks.
    for len in 20..bytes.len() - 4 {
        let mut cut = bytes[..len].to_vec();
        cut.extend_from_slice(&[0; 4]);
        reseal(&mut cut);
        assert!(Module::from_arb(&cut).is_err(), "cut at {len} must fail");
    }
}

#[test]
fn rejects_trailing_bytes() {
    let mut bytes = Module::new().to_arb();
    let end = bytes.len() - 4;
    bytes.splice(end..end, [0u8; 3]);
    reseal(&mut bytes);
    let err = Module::from_arb(&bytes).unwrap_err();
    assert_eq!(err.code(), "A-ARB-012", "{err}");
}

#[test]
fn rejects_sections_out_of_order() {
    let mut sections = empty_sections();
    sections.swap(1, 2);
    let err = Module::from_arb(&assemble(&sections)).unwrap_err();
    assert_eq!(err.code(), "A-ARB-006", "{err}");
}

#[test]
fn rejects_unused_section_bytes() {
    let mut body = empty_sections()[4].1.clone();
    body.push(0);
    let err = Module::from_arb(&with_section(4, body)).unwrap_err();
    assert_eq!(err.code(), "A-ARB-007", "{err}");
}

#[test]
fn rejects_hostile_counts_without_allocating() {
    // Claims u32::MAX symbols in a 4-byte section.
    let err = Module::from_arb(&with_section(1, le(&[u32::MAX]))).unwrap_err();
    assert_eq!(err.code(), "A-ARB-009", "{err}");
}

#[test]
fn rejects_duplicate_symbols() {
    let mut body = le(&[2, 1]);
    body.push(b'f');
    body.extend(le(&[1]));
    body.push(b'f');
    let err = Module::from_arb(&with_section(1, body)).unwrap_err();
    assert!(
        matches!(
            err,
            ArbError::DuplicateEntry {
                table: "symbol",
                index: 1,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn rejects_invalid_utf8_symbol() {
    let mut body = le(&[1, 1]);
    body.push(0xFF);
    let err = Module::from_arb(&with_section(1, body)).unwrap_err();
    assert_eq!(err.code(), "A-ARB-009", "{err}");
}

#[test]
fn rejects_self_referential_type() {
    // type 14 = ptr<type 14>: would make type printing recurse forever.
    let mut entry = vec![3u8];
    entry.extend(le(&[14, 0]));
    let err = Module::from_arb(&with_section(2, type_table(&[&entry]))).unwrap_err();
    assert!(
        matches!(
            err,
            ArbError::ForwardTypeReference {
                index: 14,
                referenced: 14,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn rejects_duplicate_types() {
    let mut entry = vec![3u8];
    entry.extend(le(&[5, 0]));
    let body = type_table(&[&entry, &entry]);
    let err = Module::from_arb(&with_section(2, body)).unwrap_err();
    assert!(
        matches!(
            err,
            ArbError::DuplicateEntry {
                table: "type",
                index: 15,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn rejects_corrupted_scalar_prefix() {
    let mut body = type_table(&[]);
    // Entry 0 must be `void` (tag 0); make it `f32`.
    body[4] = 2;
    body.insert(5, 0);
    let err = Module::from_arb(&with_section(2, body)).unwrap_err();
    assert_eq!(err.code(), "A-ARB-009", "{err}");
}

#[test]
fn rejects_invalid_integer_types() {
    for (bits, signed) in [(0u32, 0u8), (129, 0), (1, 1)] {
        let mut entry = vec![1u8];
        entry.extend(le(&[bits]));
        entry.push(signed);
        let err = Module::from_arb(&with_section(2, type_table(&[&entry]))).unwrap_err();
        assert_eq!(err.code(), "A-ARB-009", "{err}");
    }
}

#[test]
fn rejects_unmasked_integer_constant() {
    let mut body = le(&[1]);
    body.push(0); // int
    body.extend(le(&[TypeId::U8.as_u32(), 8]));
    body.extend(0x100u128.to_le_bytes());
    let err = Module::from_arb(&with_section(3, body)).unwrap_err();
    assert_eq!(err.code(), "A-ARB-009", "{err}");
}

#[test]
fn rejects_unknown_tags() {
    let err = Module::from_arb(&with_section(2, type_table(&[&[9u8]]))).unwrap_err();
    assert!(
        matches!(
            err,
            ArbError::InvalidTag {
                what: "type",
                tag: 9,
                ..
            }
        ),
        "{err}"
    );

    let mut body = le(&[1]);
    body.extend([7, 0, 0, 0, 0]);
    let err = Module::from_arb(&with_section(3, body)).unwrap_err();
    assert!(
        matches!(
            err,
            ArbError::InvalidTag {
                what: "constant",
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn rejects_unknown_opcode() {
    // A function whose entry block holds a single `const`; corrupt its
    // opcode byte.
    let mut b = ModuleBuilder::new();
    let f = b
        .declare_function("k", Linkage::Internal, Abi::Astronomy, &[], TypeId::I64)
        .unwrap();
    let mut fb = b.function_builder(f).unwrap();
    fb.append_block();
    let c = fb.const_int(TypeId::I64, 0x5A5A_5A5A).unwrap();
    fb.ret(Some(c)).unwrap();

    let mut bytes = b.finish().to_arb();
    let func = bytes.windows(4).rposition(|w| w == b"FUNC").unwrap();
    // Layout after the FUNC header: count, symbol, linkage, abi, variadic,
    // result, params, values (1 entry: type, kind tag, block, index, name
    // flag), blocks count, block name flag, block params count, instruction
    // count, opcode.
    let body = func + 12;
    let opcode_at = body + 4 + 4 + 1 + 1 + 1 + 4 + 4 + (4 + (4 + 1 + 8 + 1)) + 4 + 1 + 4 + 4;
    assert_eq!(bytes[opcode_at], 0x00, "expected the const opcode");
    bytes[opcode_at] = 0xEE;
    reseal(&mut bytes);
    let err = Module::from_arb(&bytes).unwrap_err();
    assert!(
        matches!(
            err,
            ArbError::InvalidTag {
                what: "opcode",
                tag: 0xEE,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn rejects_out_of_range_symbol_references() {
    let mut sections = empty_sections();
    // MODL: version 1.0, name = symbol 0 — but the symbol table is empty.
    sections[0].1 = le(&[1, 0]);
    sections[0].1.push(1);
    sections[0].1.extend(le(&[0]));
    let err = Module::from_arb(&assemble(&sections)).unwrap_err();
    assert_eq!(err.code(), "A-ARB-009", "{err}");
}

#[test]
fn errors_carry_codes_and_offsets() {
    let err = Module::from_arb(&with_section(1, le(&[u32::MAX]))).unwrap_err();
    assert!(err.offset().is_some());
    assert!(err.to_string().starts_with("[A-ARB-009]"), "{err}");
    let boxed: Box<dyn std::error::Error> = Box::new(err);
    let _ = boxed.to_string();
}

/// Single-byte mutations (with a fixed-up checksum) must never panic the
/// reader, the verifier or the printer.
#[test]
fn mutations_never_panic() {
    let original = build_kitchen_sink().to_arb();
    for i in 16..original.len() - 4 {
        for delta in [0x01u8, 0x80, 0xFF] {
            let mut bytes = original.clone();
            bytes[i] = bytes[i].wrapping_add(delta);
            reseal(&mut bytes);
            if let Ok(module) = Module::from_arb(&bytes) {
                let _ = module.to_arn();
                if let Ok(verified) = module.verify() {
                    let _ = verified.to_arb();
                }
            }
        }
    }
}
