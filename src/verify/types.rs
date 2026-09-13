//! Type checking of signatures, instructions, calls and constants (§23).

use crate::constant::ConstantData;
use crate::error::VerifyError;
use crate::function::Function;
use crate::id::TypeId;
use crate::instruction::{Instruction, InstructionKind};
use crate::module::Module;
use crate::types::TypeData;

pub(crate) fn check_constant(
    module: &Module,
    index: usize,
    data: &ConstantData,
    errors: &mut Vec<VerifyError>,
) {
    let invalid = |reason: String| VerifyError::InvalidConstant {
        constant: index as u32,
        reason,
    };
    match data {
        ConstantData::Int { ty, width, .. } => match module.types().get(*ty) {
            Some(TypeData::Int { bits, .. }) => {
                if bits != width {
                    errors.push(invalid(format!(
                        "integer constant width {width} does not match type `{}` ({bits} bits)",
                        module.type_name(*ty)
                    )));
                }
            }
            _ => errors.push(invalid(format!(
                "integer constant typed as non-integer `{}`",
                module.type_name(*ty)
            ))),
        },
        ConstantData::Float { ty, .. } => {
            if !module.types().get(*ty).map(|d| d.is_float()).unwrap_or(false) {
                errors.push(invalid(format!(
                    "float constant typed as non-float `{}`",
                    module.type_name(*ty)
                )));
            }
        }
        ConstantData::Null { pointee } => {
            if !module
                .types()
                .get(*pointee)
                .map(|d| d.is_first_class())
                .unwrap_or(false)
            {
                errors.push(invalid(format!(
                    "null pointer to non-first-class type `{}`",
                    module.type_name(*pointee)
                )));
            }
        }
        ConstantData::String { .. } => {}
        ConstantData::Aggregate { ty, elements } => {
            let fields: Vec<TypeId> = match module.types().get(*ty) {
                Some(TypeData::Array { element, length }) => {
                    if elements.len() as u64 != *length {
                        errors.push(invalid(format!(
                            "aggregate constant needs {length} element(s), found {}",
                            elements.len()
                        )));
                    }
                    vec![*element; elements.len()]
                }
                Some(TypeData::Struct { fields }) => {
                    if fields.len() != elements.len() {
                        errors.push(invalid(format!(
                            "aggregate constant needs {} field(s), found {}",
                            fields.len(),
                            elements.len()
                        )));
                    }
                    fields.clone()
                }
                _ => {
                    errors.push(invalid(format!(
                        "aggregate constant typed as non-aggregate `{}`",
                        module.type_name(*ty)
                    )));
                    return;
                }
            };
            for (i, (&cid, &field)) in elements.iter().zip(fields.iter()).enumerate() {
                // Pool invariants: elements must exist and must be earlier
                // pool entries (prevents cyclic aggregates, §20).
                if cid.index() >= module.constants().len() {
                    errors.push(VerifyError::UnknownConstant {
                        constant: cid.as_u32(),
                    });
                    continue;
                }
                if cid.index() >= index {
                    errors.push(invalid(format!(
                        "element {i} must reference an earlier constant (found self/forward reference)"
                    )));
                    continue;
                }
                let elem = module.constants().get(cid).unwrap();
                if !crate::error::constant_fits_type(module.types(), elem, field) {
                    errors.push(invalid(format!(
                        "element {i} does not match field type `{}`",
                        module.type_name(field)
                    )));
                }
            }
        }
    }
}

/// Structural type equality against a `TypeData` pattern, without interning
/// (the verifier only has `&Module`).
fn type_is(module: &Module, id: TypeId, pattern: &TypeData) -> bool {
    match module.types().get(id) {
        Some(actual) => actual == pattern,
        None => false,
    }
}

fn ptr_pointee(module: &Module, id: TypeId) -> Option<(&TypeId, u32)> {
    match module.types().get(id) {
        Some(TypeData::Pointer {
            pointee,
            address_space,
        }) => Some((pointee, *address_space)),
        _ => None,
    }
}

pub(crate) fn check_instruction(
    module: &Module,
    f: &Function,
    fname: &str,
    block: usize,
    _index: usize,
    inst: &Instruction,
    errors: &mut Vec<VerifyError>,
) {
    use InstructionKind as K;

    // Operand existence — every referenced value must be in the arena.
    let mut operands: Vec<crate::id::ValueId> = Vec::new();
    inst.kind.visit_values(&mut |v| operands.push(v));
    for v in operands {
        if v.index() >= f.values.len() {
            errors.push(VerifyError::UnknownValue {
                function: fname.to_string(),
                value: v.as_u32(),
            });
        }
    }
    if f.values.len() < f.params.len() {
        // Already reported by the signature check; skip typing to avoid
        // cascading panics.
        return;
    }

    let result_ty = |inst: &Instruction| -> Option<TypeId> {
        inst.result
            .and_then(|v| f.values.get(v.index()))
            .map(|d| d.ty)
    };
    let operand_ty = |v: crate::id::ValueId| -> Option<TypeId> {
        f.values.get(v.index()).map(|d| d.ty)
    };
    let mismatch = |context: String, expected: &str, found: &str| VerifyError::TypeMismatch {
        function: fname.to_string(),
        context,
        expected: expected.to_string(),
        found: found.to_string(),
    };
    let bad_operand = |reason: String| VerifyError::InvalidOperandType {
        function: fname.to_string(),
        reason,
    };
    let bad_result = |expected: String, found: String| VerifyError::InvalidResultType {
        function: fname.to_string(),
        expected,
        found,
    };
    let type_name = |t: TypeId| module.type_name(t);

    macro_rules! same_type_pair {
        ($lhs:expr, $rhs:expr, $op:expr, $class:expr) => {{
            let (lt, rt) = (operand_ty(*$lhs), operand_ty(*$rhs));
            match (lt, rt) {
                (Some(lt), Some(rt)) => {
                    if lt != rt {
                        errors.push(mismatch(
                            format!("{} operands in bb{block}", $op),
                            &type_name(lt),
                            &type_name(rt),
                        ));
                        None
                    } else {
                        let ok = match module.types().get(lt) {
                            Some(d) => $class(d),
                            None => false,
                        };
                        if !ok {
                            errors.push(bad_operand(format!(
                                "{} operands in bb{block} have unsupported type `{}`",
                                $op,
                                type_name(lt)
                            )));
                            None
                        } else {
                            Some(lt)
                        }
                    }
                }
                _ => None,
            }
        }};
    }

    match &inst.kind {
        K::Const(cid) => {
            let data = match module.constants().get(*cid) {
                Some(d) => d,
                None => {
                    errors.push(VerifyError::UnknownConstant {
                        constant: cid.as_u32(),
                    });
                    return;
                }
            };
            match result_ty(inst) {
                Some(ty) => {
                    if !crate::error::constant_fits_type(module.types(), data, ty) {
                        let which = match data {
                            ConstantData::Int { ty, .. } | ConstantData::Float { ty, .. } => {
                                type_name(*ty)
                            }
                            ConstantData::Null { pointee } => {
                                format!("ptr<{}>", type_name(*pointee))
                            }
                            ConstantData::String { .. } => "ptr<i8>".to_string(),
                            ConstantData::Aggregate { ty, .. } => type_name(*ty),
                        };
                        errors.push(bad_result(which, type_name(ty)));
                    }
                }
                None => errors.push(VerifyError::InvalidOperandType {
                    function: fname.to_string(),
                    reason: format!("const in bb{block} is missing its result value"),
                }),
            }
        }
        K::Add { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "add", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            }) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "add");
            }
        }
        K::Sub { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "sub", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            }) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "sub");
            }
        }
        K::Mul { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "mul", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            }) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "mul");
            }
        }
        K::Div { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "div", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            }) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "div");
            }
        }
        K::Rem { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "rem", |d: &TypeData| {
                d.is_arith_integer()
            }) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "rem");
            }
        }
        K::And { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "and", |d: &TypeData| d.is_integer()) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "and");
            }
        }
        K::Or { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "or", |d: &TypeData| d.is_integer()) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "or");
            }
        }
        K::Xor { lhs, rhs } => {
            if let Some(t) = same_type_pair!(lhs, rhs, "xor", |d: &TypeData| d.is_integer()) {
                check_result(inst, result_ty(inst), t, &type_name, errors, fname, "xor");
            }
        }
        K::Shl { lhs, rhs } | K::Shr { lhs, rhs } => {
            let op = if matches!(inst.kind, K::Shl { .. }) {
                "shl"
            } else {
                "shr"
            };
            if let Some(lt) = operand_ty(*lhs) {
                if !module
                    .types()
                    .get(lt)
                    .map(|d| d.is_arith_integer())
                    .unwrap_or(false)
                {
                    errors.push(bad_operand(format!(
                        "{op} left operand in bb{block} must be an integer of at least 8 bits"
                    )));
                }
                check_result(inst, result_ty(inst), lt, &type_name, errors, fname, op);
            }
            if let Some(rt) = operand_ty(*rhs) {
                if !module
                    .types()
                    .get(rt)
                    .map(|d| d.is_integer())
                    .unwrap_or(false)
                {
                    errors.push(bad_operand(format!(
                        "{op} count in bb{block} must be an integer, found `{}`",
                        type_name(rt)
                    )));
                }
            }
        }
        K::Eq { lhs, rhs } => {
            same_type_pair!(lhs, rhs, "eq", |d: &TypeData| {
                d.is_integer() || d.is_float() || d.is_pointer()
            });
            check_result(inst, result_ty(inst), TypeId::I1, &type_name, errors, fname, "eq");
        }
        K::Ne { lhs, rhs } => {
            same_type_pair!(lhs, rhs, "ne", |d: &TypeData| {
                d.is_integer() || d.is_float() || d.is_pointer()
            });
            check_result(inst, result_ty(inst), TypeId::I1, &type_name, errors, fname, "ne");
        }
        K::Lt { lhs, rhs } => {
            same_type_pair!(lhs, rhs, "lt", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            });
            check_result(inst, result_ty(inst), TypeId::I1, &type_name, errors, fname, "lt");
        }
        K::Le { lhs, rhs } => {
            same_type_pair!(lhs, rhs, "le", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            });
            check_result(inst, result_ty(inst), TypeId::I1, &type_name, errors, fname, "le");
        }
        K::Gt { lhs, rhs } => {
            same_type_pair!(lhs, rhs, "gt", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            });
            check_result(inst, result_ty(inst), TypeId::I1, &type_name, errors, fname, "gt");
        }
        K::Ge { lhs, rhs } => {
            same_type_pair!(lhs, rhs, "ge", |d: &TypeData| {
                d.is_arith_integer() || d.is_float()
            });
            check_result(inst, result_ty(inst), TypeId::I1, &type_name, errors, fname, "ge");
        }
        K::Alloca { pointee } => {
            let pointee_ok = module
                .types()
                .get(*pointee)
                .map(|d| d.is_first_class())
                .unwrap_or(false);
            if !pointee_ok {
                errors.push(bad_operand(format!(
                    "alloca pointee `{}` in bb{block} is not first-class",
                    type_name(*pointee)
                )));
            }
            match result_ty(inst) {
                Some(ty) => {
                    if !type_is(
                        module,
                        ty,
                        &TypeData::Pointer {
                            pointee: *pointee,
                            address_space: 0,
                        },
                    ) {
                        errors.push(bad_result(
                            format!("ptr<{}>", type_name(*pointee)),
                            type_name(ty),
                        ));
                    }
                }
                None => {
                    errors.push(bad_result(
                        format!("ptr<{}>", type_name(*pointee)),
                        "void".to_string(),
                    ))
                }
            }
        }
        K::Load { ty, pointer } => match operand_ty(*pointer).and_then(|t| ptr_pointee(module, t)) {
            Some((pointee, _as)) => {
                if !module.types().get(*ty).map(|d| d.is_first_class()).unwrap_or(false) {
                    errors.push(bad_operand(format!(
                        "load type `{}` in bb{block} is not first-class",
                        type_name(*ty)
                    )));
                } else if *pointee != *ty {
                    errors.push(mismatch(
                        format!("load pointer pointee in bb{block}"),
                        &type_name(*ty),
                        &type_name(*pointee),
                    ));
                }
                check_result(inst, result_ty(inst), *ty, &type_name, errors, fname, "load");
            }
            None => errors.push(bad_operand(format!(
                "load pointer operand in bb{block} is not a pointer"
            ))),
        },
        K::Store { pointer, value } => {
            if inst.result.is_some() {
                errors.push(bad_operand(format!(
                    "store in bb{block} must not produce a value"
                )));
            }
            match operand_ty(*pointer).and_then(|t| ptr_pointee(module, t)) {
                Some((pointee, _)) => match operand_ty(*value) {
                    Some(vt) => {
                        if vt != *pointee {
                            errors.push(mismatch(
                                format!("store value in bb{block}"),
                                &type_name(*pointee),
                                &type_name(vt),
                            ));
                        }
                    }
                    None => {}
                },
                None => errors.push(bad_operand(format!(
                    "store pointer operand in bb{block} is not a pointer"
                ))),
            }
        }
        K::PtrOffset { pointer, offset } => {
            let pty = match operand_ty(*pointer) {
                Some(t) if ptr_pointee(module, t).is_some() => t,
                _ => {
                    errors.push(bad_operand(format!(
                        "ptr_offset base in bb{block} is not a pointer"
                    )));
                    return;
                }
            };
            match operand_ty(*offset) {
                Some(ot) => {
                    if !module.types().get(ot).map(|d| d.is_arith_integer()).unwrap_or(false) {
                        errors.push(bad_operand(format!(
                            "ptr_offset offset in bb{block} must be an integer, found `{}`",
                            type_name(ot)
                        )));
                    }
                }
                None => {}
            }
            check_result(
                inst,
                result_ty(inst),
                pty,
                &type_name,
                errors,
                fname,
                "ptr_offset",
            );
        }
        K::Ext { to, value } => {
            let from = operand_ty(*value);
            let from_ok = from
                .and_then(|t| module.types().get(t))
                .map(|d| d.is_integer())
                .unwrap_or(false);
            let to_ok = module
                .types()
                .get(*to)
                .map(|d| d.is_arith_integer())
                .unwrap_or(false);
            if !from_ok || !to_ok {
                errors.push(bad_operand(format!(
                    "ext in bb{block} requires integer operand and integer target"
                )));
            } else {
                let fb = module.types().get(from.unwrap()).and_then(|d| d.int_bits()).unwrap();
                let tb = module.types().get(*to).and_then(|d| d.int_bits()).unwrap();
                if tb <= fb {
                    errors.push(bad_operand(format!(
                        "ext in bb{block} must widen ({} -> {})",
                        type_name(from.unwrap()),
                        type_name(*to)
                    )));
                }
            }
            check_result(inst, result_ty(inst), *to, &type_name, errors, fname, "ext");
        }
        K::Trunc { to, value } => {
            let from = operand_ty(*value);
            let from_ok = from
                .and_then(|t| module.types().get(t))
                .map(|d| d.is_integer())
                .unwrap_or(false);
            let to_ok = module
                .types()
                .get(*to)
                .map(|d| d.is_integer())
                .unwrap_or(false);
            if !from_ok || !to_ok {
                errors.push(bad_operand(format!(
                    "trunc in bb{block} requires integer operand and integer target"
                )));
            } else {
                let fb = module.types().get(from.unwrap()).and_then(|d| d.int_bits()).unwrap();
                let tb = module.types().get(*to).and_then(|d| d.int_bits()).unwrap();
                if tb >= fb {
                    errors.push(bad_operand(format!(
                        "trunc in bb{block} must narrow ({} -> {})",
                        type_name(from.unwrap()),
                        type_name(*to)
                    )));
                }
            }
            check_result(inst, result_ty(inst), *to, &type_name, errors, fname, "trunc");
        }
        K::IntToFloat { to, value } => {
            let from_ok = operand_ty(*value)
                .and_then(|t| module.types().get(t))
                .map(|d| d.is_arith_integer())
                .unwrap_or(false);
            let to_ok = module.types().get(*to).map(|d| d.is_float()).unwrap_or(false);
            if !from_ok || !to_ok {
                errors.push(bad_operand(format!(
                    "int_to_float in bb{block} requires integer operand and float target"
                )));
            }
            check_result(
                inst,
                result_ty(inst),
                *to,
                &type_name,
                errors,
                fname,
                "int_to_float",
            );
        }
        K::FloatToInt { to, value } => {
            let from_ok = operand_ty(*value)
                .and_then(|t| module.types().get(t))
                .map(|d| d.is_float())
                .unwrap_or(false);
            let to_ok = module
                .types()
                .get(*to)
                .map(|d| d.is_arith_integer())
                .unwrap_or(false);
            if !from_ok || !to_ok {
                errors.push(bad_operand(format!(
                    "float_to_int in bb{block} requires float operand and integer target"
                )));
            }
            check_result(
                inst,
                result_ty(inst),
                *to,
                &type_name,
                errors,
                fname,
                "float_to_int",
            );
        }
        K::PtrCast { to, pointer } => {
            let from_ptr = operand_ty(*pointer)
                .and_then(|t| module.types().get(t))
                .map(|d| d.is_pointer())
                .unwrap_or(false);
            let to_ptr = module.types().get(*to).map(|d| d.is_pointer()).unwrap_or(false);
            if !from_ptr || !to_ptr {
                errors.push(bad_operand(format!(
                    "ptr_cast in bb{block} requires pointer operand and pointer target"
                )));
            }
            check_result(inst, result_ty(inst), *to, &type_name, errors, fname, "ptr_cast");
        }
        K::Call { callee, args } => {
            check_call(module, f, fname, block, *callee, args, inst, errors)
        }
        K::Construct { ty, fields } => {
            let expected = aggregate_fields(module, *ty);
            match expected {
                Some(expected) => {
                    if expected.len() != fields.len() {
                        errors.push(bad_operand(format!(
                            "construct of `{}` in bb{block} needs {} field(s), found {}",
                            type_name(*ty),
                            expected.len(),
                            fields.len()
                        )));
                    }
                    for (i, (&field, &want)) in fields.iter().zip(expected.iter()).enumerate() {
                        if let Some(actual) = operand_ty(field) {
                            if actual != want {
                                errors.push(mismatch(
                                    format!("construct field {i} in bb{block}"),
                                    &type_name(want),
                                    &type_name(actual),
                                ));
                            }
                        }
                    }
                }
                None => errors.push(bad_operand(format!(
                    "construct in bb{block} targets non-aggregate type `{}`",
                    type_name(*ty)
                ))),
            }
            check_result(
                inst,
                result_ty(inst),
                *ty,
                &type_name,
                errors,
                fname,
                "construct",
            );
        }
        K::Extract { aggregate, index } => {
            let fields = operand_ty(*aggregate).and_then(|t| aggregate_fields(module, t));
            match fields {
                Some(fields) => match fields.get(*index as usize) {
                    Some(&field) => check_result(
                        inst,
                        result_ty(inst),
                        field,
                        &type_name,
                        errors,
                        fname,
                        "extract",
                    ),
                    None => errors.push(bad_operand(format!(
                        "extract index {index} out of range in bb{block}"
                    ))),
                },
                None => errors.push(bad_operand(format!(
                    "extract in bb{block} requires an aggregate operand"
                ))),
            }
        }
        K::Insert {
            aggregate,
            index,
            value,
        } => {
            let fields = operand_ty(*aggregate).and_then(|t| aggregate_fields(module, t));
            match fields {
                Some(fields) => {
                    let aty = operand_ty(*aggregate).unwrap();
                    match fields.get(*index as usize) {
                        Some(&field) => {
                            if let Some(actual) = operand_ty(*value) {
                                if actual != field {
                                    errors.push(mismatch(
                                        format!("insert field {index} in bb{block}"),
                                        &type_name(field),
                                        &type_name(actual),
                                    ));
                                }
                            }
                            check_result(
                                inst,
                                result_ty(inst),
                                aty,
                                &type_name,
                                errors,
                                fname,
                                "insert",
                            );
                        }
                        None => errors.push(bad_operand(format!(
                            "insert index {index} out of range in bb{block}"
                        ))),
                    }
                }
                None => errors.push(bad_operand(format!(
                    "insert in bb{block} requires an aggregate operand"
                ))),
            }
        }
    }

    // Result presence: every instruction except `store` and void calls
    // produces a value. (Call handled in `check_call`.)
    if !matches!(inst.kind, K::Store { .. } | K::Call { .. }) && inst.result.is_none() {
        errors.push(VerifyError::InvalidOperandType {
            function: fname.to_string(),
            reason: format!("instruction in bb{block} is missing its result value"),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn check_result(
    _inst: &Instruction,
    actual: Option<TypeId>,
    expected: TypeId,
    type_name: &impl Fn(TypeId) -> String,
    errors: &mut Vec<VerifyError>,
    fname: &str,
    op: &str,
) {
    match actual {
        Some(ty) => {
            if ty != expected {
                errors.push(VerifyError::InvalidResultType {
                    function: fname.to_string(),
                    expected: type_name(expected),
                    found: type_name(ty),
                });
            }
        }
        None => errors.push(VerifyError::InvalidResultType {
            function: fname.to_string(),
            expected: type_name(expected),
            found: format!("{op} result value is missing"),
        }),
    }
}

fn aggregate_fields(module: &Module, ty: TypeId) -> Option<Vec<TypeId>> {
    match module.types().get(ty) {
        Some(TypeData::Array { element, length }) => Some(vec![*element; *length as usize]),
        Some(TypeData::Struct { fields }) => Some(fields.clone()),
        _ => None,
    }
}

fn check_call(
    module: &Module,
    f: &Function,
    fname: &str,
    block: usize,
    callee: crate::id::FunctionId,
    args: &[crate::id::ValueId],
    inst: &Instruction,
    errors: &mut Vec<VerifyError>,
) {
    let callee_f = match module.function(callee) {
        Some(f) => f,
        None => {
            errors.push(VerifyError::UnknownFunction {
                function: fname.to_string(),
                callee: callee.as_u32(),
            });
            return;
        }
    };
    let callee_name = module
        .symbol_name(callee_f.symbol)
        .unwrap_or("<invalid>")
        .to_string();
    let fixed = &callee_f.params;

    if callee_f.variadic {
        if args.len() < fixed.len() {
            errors.push(VerifyError::CallArityMismatch {
                function: fname.to_string(),
                callee: callee_name.clone(),
                expected: format!("at least {}", fixed.len()),
                found: args.len(),
            });
        }
    } else if args.len() != fixed.len() {
        errors.push(VerifyError::CallArityMismatch {
            function: fname.to_string(),
            callee: callee_name.clone(),
            expected: format!("{}", fixed.len()),
            found: args.len(),
        });
    }

    for (i, (&arg, &param)) in args.iter().zip(fixed.iter()).enumerate() {
        if let Some(actual) = f.values.get(arg.index()).map(|d| d.ty) {
            if actual != param.ty {
                errors.push(VerifyError::CallArgTypeMismatch {
                    function: fname.to_string(),
                    callee: callee_name.clone(),
                    index: i,
                    expected: module.type_name(param.ty),
                    found: module.type_name(actual),
                });
            }
        }
    }
    for (i, &arg) in args.iter().skip(fixed.len()).enumerate() {
        if let Some(actual) = f.values.get(arg.index()).map(|d| d.ty) {
            if !module
                .types()
                .get(actual)
                .map(|d| d.is_first_class())
                .unwrap_or(false)
            {
                errors.push(VerifyError::CallArgTypeMismatch {
                    function: fname.to_string(),
                    callee: callee_name.clone(),
                    index: fixed.len() + i,
                    expected: "a first-class type".to_string(),
                    found: module.type_name(actual),
                });
            }
        }
    }

    let is_void = module
        .types()
        .get(callee_f.result)
        .map(|d| d.is_void())
        .unwrap_or(false);
    match (&inst.result, is_void) {
        (Some(result), false) => {
            if let Some(actual) = f.values.get(result.index()).map(|d| d.ty) {
                if actual != callee_f.result {
                    errors.push(VerifyError::InvalidResultType {
                        function: fname.to_string(),
                        expected: module.type_name(callee_f.result),
                        found: module.type_name(actual),
                    });
                }
            }
        }
        (Some(_), true) => errors.push(VerifyError::InvalidOperandType {
            function: fname.to_string(),
            reason: format!("call to `{callee_name}` in bb{block} returns void and must not produce a value"),
        }),
        (None, false) => errors.push(VerifyError::InvalidOperandType {
            function: fname.to_string(),
            reason: format!(
                "call to `{callee_name}` in bb{block} returns a value and needs a result"
            ),
        }),
        (None, true) => {}
    }
}
