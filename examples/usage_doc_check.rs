//! Temporary verification harness for USAGE.md code samples. Delete after use.

use astronomy::{Abi, Linkage, Module, ModuleBuilder, TypeId, Verifier};

fn quick_start() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = ModuleBuilder::with_name("demo");
    let add = builder.declare_function(
        "add",
        Linkage::Exported,
        Abi::Astronomy,
        &[("a", TypeId::I64), ("b", TypeId::I64)],
        TypeId::I64,
    )?;
    let mut fb = builder.function_builder(add)?;
    fb.append_block();
    let (a, b) = (fb.param(0), fb.param(1));
    let sum = fb.add(a, b)?;
    fb.ret(Some(sum))?;
    let module = builder.finish();
    let verified = Verifier::verify(module)?;
    let text = astronomy::text::print(&verified);
    println!("=== quick start ===\n{text}");
    let expected = "::ASTRONOMY::MODULE_START\n\
                    ::ASTRONOMY::MODULE_NAME \"demo\"\n\
                    ::ASTRONOMY::MODULE_VERSION 1\n\
                    ::ASTRONOMY::FUNCTION_START add fn(i64 %a, i64 %b) -> i64 linkage=exported abi=astronomy\n\
                    ::ASTRONOMY::BLOCK_START bb0\n\
                    %0 = ::ASTRONOMY::ADD i64 i64 %a, i64 %b\n\
                    ::ASTRONOMY::RETURN i64 %0\n\
                    ::ASTRONOMY::BLOCK_END\n\
                    ::ASTRONOMY::FUNCTION_END\n\
                    ::ASTRONOMY::MODULE_END\n";
    assert_eq!(text, expected, "quick start output drifted");
    Ok(())
}

fn block_args() -> Result<(), Box<dyn std::error::Error>> {
    let mut b = ModuleBuilder::new();
    let pick = b.declare_function(
        "pick",
        Linkage::Exported,
        Abi::Astronomy,
        &[("cond", TypeId::I1), ("a", TypeId::I64), ("bb", TypeId::I64)],
        TypeId::I64,
    )?;
    let mut fb = b.function_builder(pick)?;
    let (cond, a, b_) = (fb.param(0), fb.param(1), fb.param(2));

    let entry = fb.append_block();
    let left = fb.append_block_with_params(&[("x", TypeId::I64)])?;
    let right = fb.append_block_with_params(&[("x", TypeId::I64)])?;
    let merge = fb.append_block_with_params(&[("result", TypeId::I64)])?;

    fb.switch_to(entry)?;
    fb.branch(cond, left, &[a], right, &[b_])?;

    let x = fb.block_param(left, 0)?;
    fb.switch_to(left)?;
    fb.jump(merge, &[x])?;

    let x = fb.block_param(right, 0)?;
    fb.switch_to(right)?;
    fb.jump(merge, &[x])?;

    let result = fb.block_param(merge, 0)?;
    fb.switch_to(merge)?;
    fb.ret(Some(result))?;

    let verified = Verifier::verify(b.finish())?;
    println!("=== block args ===\n{}", verified.to_arn());
    Ok(())
}

fn externs_and_errors() -> Result<(), Box<dyn std::error::Error>> {
    let mut b = ModuleBuilder::new();
    let i8_ptr = b.ptr_type(TypeId::I8);
    let printf = b.declare_extern("printf", Abi::C, &[i8_ptr], true, TypeId::I32)?;
    let main = b.declare_function("main", Linkage::Exported, Abi::Astronomy, &[], TypeId::I32)?;
    let mut fb = b.function_builder(main)?;
    fb.append_block();
    let s = fb.const_string(b"hello\n")?;
    let n = fb.call(printf, &[s])?.expect("printf returns i32");
    fb.ret(Some(n))?;
    let verified = Verifier::verify(b.finish())?;
    println!("=== externs ===\n{}", verified.to_arn());

    // Roundtrip: print -> parse -> verify -> print is stable.
    let text = verified.to_arn();
    let reparsed = Module::parse_arn(&text)?;
    let reverified = reparsed.verify()?;
    assert_eq!(reverified.to_arn(), text, "roundtrip is not stable");

    // Programmatic inspection (USAGE.md §8).
    let id = verified.function_by_name("main").expect("main exists");
    assert_eq!(verified.function_name(id), "main");
    assert_eq!(verified.type_name(i8_ptr), "ptr<i8>");
    let f = verified.function(id).unwrap();
    assert!(!f.is_declaration());

    // Structured verify errors (USAGE.md §6): break an operand deliberately.
    let mut bad = ModuleBuilder::new();
    let f2 = bad.declare_function("f", Linkage::Exported, Abi::Astronomy, &[("x", TypeId::I64)], TypeId::I64)?;
    let mut fb2 = bad.function_builder(f2)?;
    fb2.append_block();
    let x = fb2.param(0);
    let other = fb2.const_int(TypeId::I32, 1)?;
    let _ = fb2.add(x, other); // builder rejects this eagerly: type mismatch
    let zero = fb2.const_int(TypeId::I64, 0)?;
    let sum = fb2.add(x, zero)?;
    // Missing terminator -> verify error path.
    let module = bad.finish();
    match module.verify() {
        Ok(_) => panic!("expected verification failure"),
        Err(report) => {
            println!("=== verify errors ===\n{report}");
            assert!(!report.is_empty());
            let codes: Vec<&str> = report.errors().iter().map(|e| e.code()).collect();
            assert!(codes.iter().all(|c| c.starts_with("A-VERIFY-")));
        }
    }
    Ok(())
}

fn main() {
    quick_start().expect("quick start sample failed");
    block_args().expect("block args sample failed");
    externs_and_errors().expect("extern/error sample failed");
    println!("ALL USAGE.md SAMPLES OK");
}
