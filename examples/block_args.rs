//! Acceptance test 3 (§52): block arguments (`merge` pattern) built
//! through the Rust API and verified.

use astronomy::{Abi, Linkage, ModuleBuilder, TypeId, Verifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = ModuleBuilder::with_name("block-args-demo");

    let pick = builder.declare_function(
        "pick",
        Linkage::Exported,
        Abi::Astronomy,
        &[("cond", TypeId::I1), ("a", TypeId::I64), ("b", TypeId::I64)],
        TypeId::I64,
    )?;

    let mut fb = builder.function_builder(pick)?;
    let (cond, a, b) = (fb.param(0), fb.param(1), fb.param(2));

    // entry: branch %cond, left(%a), right(%b)
    let entry = fb.append_named_block("entry")?;
    let left = fb.append_block_with_params(&[("x", TypeId::I64)])?;
    let right = fb.append_block_with_params(&[("x", TypeId::I64)])?;
    let merge = fb.append_block_with_params(&[("result", TypeId::I64)])?;
    fb.switch_to(entry)?;
    fb.branch(cond, left, &[a], right, &[b])?;

    // left(%x): jump merge(%x)
    let x_left = fb.block_param(left, 0)?;
    fb.switch_to(left)?;
    fb.jump(merge, &[x_left])?;

    // right(%x): jump merge(%x)
    let x_right = fb.block_param(right, 0)?;
    fb.switch_to(right)?;
    fb.jump(merge, &[x_right])?;

    // merge(%result): return %result
    let result = fb.block_param(merge, 0)?;
    fb.switch_to(merge)?;
    fb.ret(Some(result))?;

    let module = builder.finish();
    let verified = Verifier::verify(module)?;
    print!("{}", verified.to_arn());
    Ok(())
}
