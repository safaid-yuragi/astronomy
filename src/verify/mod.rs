//! The Astronomy verifier (§23 Verifier).
//!
//! The verifier is one of the core deliverables of the library: a module
//! that has not been verified is untrusted, and backends accept only
//! [`VerifiedModule`] instances (§24).
//!
//! Checks are grouped by concern:
//!
//! * [`ssa`] — every value is defined exactly once, at a consistent site.
//! * [`types`] — signatures, instruction typing, calls, returns, constants.
//! * [`cfg`] — terminators, branch arguments, dominance of uses by defs.

mod types;

pub(crate) mod cfg;
pub(crate) mod ssa;

use crate::error::{VerifyError, VerifyErrorReport};
use crate::function::Function;
use crate::module::{Module, VerifiedModule};

/// Verifies modules, producing [`VerifiedModule`]s.
#[derive(Debug, Clone, Copy, Default)]
pub struct Verifier;

/// Convenience free function equivalent to `Verifier::verify`.
pub fn verify(module: Module) -> Result<VerifiedModule, VerifyErrorReport> {
    Verifier::verify(module)
}

impl Verifier {
    /// Verifies `module`, returning all errors found (not just the first).
    ///
    /// On success the module is wrapped in a [`VerifiedModule`], making the
    /// verification result part of the type system.
    pub fn verify(module: Module) -> Result<VerifiedModule, VerifyErrorReport> {
        let mut errors = Vec::new();
        Verifier::verify_ref(&module, &mut errors);
        if errors.is_empty() {
            Ok(VerifiedModule::new(module))
        } else {
            Err(VerifyErrorReport::new(errors))
        }
    }

    /// Verifies a borrowed module, appending errors to `errors`.
    pub fn verify_ref(module: &Module, errors: &mut Vec<VerifyError>) {
        check_module(module, errors);
        for (i, function) in module.functions().iter().enumerate() {
            check_function(module, i, function, errors);
        }
    }
}

fn fname(module: &Module, f: &Function) -> String {
    module
        .symbol_name(f.symbol)
        .unwrap_or("<invalid>")
        .to_string()
}

fn check_module(module: &Module, errors: &mut Vec<VerifyError>) {
    // Duplicate function symbols (§17: symbols are unique per module).
    for i in 0..module.functions().len() {
        let name = module.function_name(crate::id::FunctionId::new(i as u32));
        for j in (i + 1)..module.functions().len() {
            let other = module.function_name(crate::id::FunctionId::new(j as u32));
            if name == other {
                errors.push(VerifyError::DuplicateFunctionName {
                    name: name.to_string(),
                });
            }
        }
    }
    // Constant pool well-formedness (§20). Checked for every entry so that
    // unused-but-invalid constants are still rejected.
    for i in 0..module.constants().len() {
        let id = crate::id::ConstantId::new(i as u32);
        match module.constants().get(id) {
            None => unreachable!("constant index in range"),
            Some(data) => types::check_constant(module, i, data, errors),
        }
    }
}

fn check_function(
    module: &Module,
    index: usize,
    f: &Function,
    errors: &mut Vec<VerifyError>,
) {
    let name = fname(module, f);
    let _ = index;

    // -- signature and linkage shape (§17, §19) ---------------------------
    if f.is_declaration() {
        if !matches!(f.linkage, crate::function::Linkage::External) {
            errors.push(VerifyError::InvalidDeclaration {
                function: name.clone(),
                reason: "a function without a body is an extern declaration and must use `external` linkage".to_string(),
            });
        }
    } else if f.variadic {
        errors.push(VerifyError::VariadicDefinition {
            function: name.clone(),
        });
    }

    for (i, p) in f.params.iter().enumerate() {
        let ok = module
            .types()
            .get(p.ty)
            .map(|d| d.is_first_class())
            .unwrap_or(false);
        if !ok {
            errors.push(VerifyError::InvalidSignature {
                function: name.clone(),
                reason: format!(
                    "parameter {i} has non-first-class type `{}`",
                    module.type_name(p.ty)
                ),
            });
        }
    }
    let result_ok = module
        .types()
        .get(f.result)
        .map(|d| d.is_first_class() || d.is_void())
        .unwrap_or(false);
    if !result_ok {
        errors.push(VerifyError::InvalidSignature {
            function: name.clone(),
            reason: format!(
                "result type `{}` must be first-class or void",
                module.type_name(f.result)
            ),
        });
    }

    // -- SSA single-definition (§12, §23) ----------------------------------
    ssa::check_definitions(module, f, &name, errors);

    // -- entry block rules (§13, §14) --------------------------------------
    if let Some(entry) = f.blocks.first() {
        if !entry.params.is_empty() {
            errors.push(VerifyError::EntryBlockWithParams {
                function: name.clone(),
                count: entry.params.len(),
            });
        }
    }

    // -- per-block checks ----------------------------------------------------
    for (bi, block) in f.blocks.iter().enumerate() {
        if block.terminator.is_none() {
            errors.push(VerifyError::MissingTerminator {
                function: name.clone(),
                block: bi as u32,
            });
        }
        for (pi, param) in block.params.iter().enumerate() {
            let ok = module
                .types()
                .get(param.ty)
                .map(|d| d.is_first_class())
                .unwrap_or(false);
            if !ok {
                errors.push(VerifyError::InvalidSignature {
                    function: name.clone(),
                    reason: format!(
                        "block bb{bi} parameter {pi} has non-first-class type `{}`",
                        module.type_name(param.ty)
                    ),
                });
            }
        }
        for (ii, inst) in block.instructions.iter().enumerate() {
            types::check_instruction(module, f, &name, bi, ii, inst, errors);
        }
    }

    // -- CFG, branch arguments and dominance (§13, §14) ----------------------
    cfg::check_cfg(module, f, &name, errors);
}
