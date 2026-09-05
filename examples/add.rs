//! Acceptance test 1 (§50): `add(a: i64, b: i64) -> i64` built through the
//! Rust API, verified, and serialized to canonical `.arn`.

use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = ModuleBuilder::with_name("add-demo");

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
    print!("{}", verified.to_arn());
    Ok(())
}
