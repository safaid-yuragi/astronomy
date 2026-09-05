//! Acceptance test 2 (§51): a conditional branch (`abs`-style CFG) built
//! through the Rust API and verified.

use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = ModuleBuilder::with_name("cfg-demo");

    let abs = builder.declare_function(
        "abs",
        Linkage::Exported,
        Abi::Astronomy,
        &[("x", TypeId::I32)],
        TypeId::I32,
    )?;

    let mut fb = builder.function_builder(abs)?;
    let x = fb.param(0);

    let entry = fb.append_named_block("entry")?;
    let negative = fb.append_named_block("negative")?;
    let positive = fb.append_named_block("positive")?;

    fb.switch_to(entry)?;
    let zero = fb.const_int(TypeId::I32, 0)?;
    let cond = fb.lt(x, zero)?;
    fb.branch(cond, negative, &[], positive, &[])?;

    fb.switch_to(negative)?;
    let negated = fb.sub(zero, x)?;
    fb.ret(Some(negated))?;

    fb.switch_to(positive)?;
    fb.ret(Some(x))?;

    let module = builder.finish();
    let verified = Verifier::verify(module)?;
    print!("{}", verified.to_arn());
    Ok(())
}
