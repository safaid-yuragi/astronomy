//! External function declarations (§19 Extern): declare `printf` with the
//! C ABI and call it. Note that the Core carries only the signature and
//! ABI — no header paths or other C specifics.

use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = ModuleBuilder::with_name("extern-demo");

    let i8_ptr = builder.ptr_type(TypeId::I8);
    let printf = builder.declare_extern("printf", Abi::C, &[i8_ptr], true, TypeId::I32)?;

    let hello = builder.declare_function(
        "hello",
        Linkage::Exported,
        Abi::Astronomy,
        &[("n", TypeId::I32)],
        TypeId::VOID,
    )?;

    let mut fb = builder.function_builder(hello)?;
    let n = fb.param(0);
    fb.append_block();

    let fmt = fb.const_string(b"n = %d\n")?;
    let _printed = fb.call(printf, &[fmt, n])?;
    fb.ret_void()?;

    let module = builder.finish();
    let verified = Verifier::verify(module)?;
    print!("{}", verified.to_arn());
    Ok(())
}
