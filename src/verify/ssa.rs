//! SSA single-definition checks (§12 SSA, §23).
//!
//! Every SSA value must be defined exactly once, and the `ValueKind`
//! recorded in the arena must agree with the actual definition site.

use std::collections::HashMap;

use crate::error::VerifyError;
use crate::function::Function;
use crate::id::ValueId;
use crate::module::Module;
use crate::value::ValueKind;

/// Where a definition was found during the scan.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DefSite {
    FunctionParam(usize),
    BlockParam { block: usize, index: usize },
    Inst { block: usize, index: usize },
}

impl DefSite {
    fn describe(&self) -> String {
        match self {
            DefSite::FunctionParam(i) => format!("function parameter {i}"),
            DefSite::BlockParam { block, index } => {
                format!("parameter {index} of bb{block}")
            }
            DefSite::Inst { block, index } => format!("instruction {index} of bb{block}"),
        }
    }
}

pub(crate) fn check_definitions(
    _module: &Module,
    f: &Function,
    name: &str,
    errors: &mut Vec<VerifyError>,
) {
    let mut defined: HashMap<u32, DefSite> = HashMap::new();
    let record = |value: u32,
                  site: DefSite,
                  defined: &mut HashMap<u32, DefSite>,
                  errors: &mut Vec<VerifyError>| {
        if let Some(first) = defined.get(&value) {
            errors.push(VerifyError::ValueRedefined {
                function: name.to_string(),
                value,
                first: first.describe(),
                second: site.describe(),
            });
        } else {
            defined.insert(value, site);
        }
    };

    // Function parameters are values 0..n by construction, but direct
    // construction may have tampered with the arena — cross-check both
    // directions.
    if f.values.len() < f.params.len() {
        errors.push(VerifyError::InvalidSignature {
            function: name.to_string(),
            reason: format!(
                "value arena holds {} entries but the function has {} parameters",
                f.values.len(),
                f.params.len()
            ),
        });
    }
    for (i, p) in f.params.iter().enumerate() {
        if p.value.index() != i {
            errors.push(VerifyError::InconsistentDefinition {
                function: name.to_string(),
                value: p.value.as_u32(),
                reason: format!(
                    "function parameter {i} must be value v{i}, found v{}",
                    p.value.as_u32()
                ),
            });
        }
        record(
            p.value.as_u32(),
            DefSite::FunctionParam(i),
            &mut defined,
            errors,
        );
    }

    for (bi, block) in f.blocks.iter().enumerate() {
        for (pi, param) in block.params.iter().enumerate() {
            record(
                param.value.as_u32(),
                DefSite::BlockParam { block: bi, index: pi },
                &mut defined,
                errors,
            );
        }
        for (ii, inst) in block.instructions.iter().enumerate() {
            if let Some(result) = inst.result {
                if result.index() >= f.values.len() {
                    errors.push(VerifyError::UnknownValue {
                        function: name.to_string(),
                        value: result.as_u32(),
                    });
                    continue;
                }
                record(
                    result.as_u32(),
                    DefSite::Inst {
                        block: bi,
                        index: ii,
                    },
                    &mut defined,
                    errors,
                );
            }
        }
    }

    // Every arena entry must be defined exactly once and its `kind` must
    // point at the actual definition site.
    for (vi, value) in f.values.iter().enumerate() {
        let vi = vi as u32;
        match &value.kind {
            ValueKind::Reserved => errors.push(VerifyError::UndefinedValue {
                function: name.to_string(),
                value: vi,
            }),
            ValueKind::Param { index } => {
                if *index as usize >= f.params.len() {
                    errors.push(VerifyError::InconsistentDefinition {
                        function: name.to_string(),
                        value: vi,
                        reason: format!(
                            "claims to be parameter {index} but the function has {} parameters",
                            f.params.len()
                        ),
                    });
                } else {
                    match defined.get(&vi) {
                        Some(DefSite::FunctionParam(i)) if *index as usize == *i => {}
                        _ => errors.push(VerifyError::InconsistentDefinition {
                            function: name.to_string(),
                            value: vi,
                            reason: "claims to be a function parameter but is not registered as one"
                                .to_string(),
                        }),
                    }
                    let declared = f.params[*index as usize];
                    if declared.value != ValueId::new(vi) || declared.ty != value.ty {
                        errors.push(VerifyError::InconsistentDefinition {
                            function: name.to_string(),
                            value: vi,
                            reason: "parameter type disagrees with the declared signature"
                                .to_string(),
                        });
                    }
                }
            }
            ValueKind::BlockParam { block, index } => {
                let matches = f
                    .blocks
                    .get(block.index())
                    .and_then(|b| b.params.get(*index as usize))
                    .map(|p| p.value == ValueId::new(vi))
                    .unwrap_or(false);
                if !matches {
                    errors.push(VerifyError::InconsistentDefinition {
                        function: name.to_string(),
                        value: vi,
                        reason: format!(
                            "claims to be parameter {index} of bb{block} but that slot holds another value"
                        ),
                    });
                } else {
                    let declared = &f.blocks[block.index()].params[*index as usize];
                    if declared.ty != value.ty {
                        errors.push(VerifyError::InconsistentDefinition {
                            function: name.to_string(),
                            value: vi,
                            reason: "block parameter type disagrees with its declaration"
                                .to_string(),
                        });
                    }
                }
            }
            ValueKind::Inst { block, index } => {
                let matches = f
                    .blocks
                    .get(block.index())
                    .and_then(|b| b.instructions.get(*index as usize))
                    .map(|inst| inst.result == Some(ValueId::new(vi)))
                    .unwrap_or(false);
                if !matches {
                    errors.push(VerifyError::InconsistentDefinition {
                        function: name.to_string(),
                        value: vi,
                        reason: format!(
                            "claims to be the result of instruction {index} of bb{block} but that instruction produces another value"
                        ),
                    });
                }
            }
        }
    }
}
