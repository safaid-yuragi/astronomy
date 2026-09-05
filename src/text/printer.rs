//! Canonical `.arn` printer (§32 Canonical Printer).
//!
//! The printer is deterministic: the same module always produces the same
//! text, which makes snapshot tests, diffs and cache keys stable (§46
//! Determinism). Blocks are emitted in reverse post-order (with unreachable
//! blocks appended in index order) so that value definitions precede uses
//! textually wherever possible; anonymous values and blocks are renumbered
//! by emission order, guaranteeing roundtrip idempotence (§33).

use std::collections::HashMap;
use std::fmt::Write;

use crate::constant::ConstantData;
use crate::function::{Abi, Function, Linkage};
use crate::id::{BlockId, TypeId, ValueId};
use crate::instruction::InstructionKind;
use crate::module::Module;
use crate::Terminator;

/// Serializes a module to canonical `.arn` text.
pub fn print(module: &Module) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "::ASTRONOMY::MODULE_START");
    if let Some(name) = module.name() {
        let _ = writeln!(out, "::ASTRONOMY::MODULE_NAME \"{}\"", escape_string(name.as_bytes()));
    }
    let _ = writeln!(
        out,
        "::ASTRONOMY::MODULE_VERSION {}",
        module.version().major
    );
    print_constants(module, &mut out);
    for f in module.functions() {
        print_function(module, f, &mut out);
    }
    let _ = writeln!(out, "::ASTRONOMY::MODULE_END");
    out
}

fn print_constants(module: &Module, out: &mut String) {
    if module.constants().is_empty() {
        return;
    }
    let _ = writeln!(out, "::ASTRONOMY::CONSTANTS_START");
    for i in 0..module.constants().len() {
        let id = crate::id::ConstantId::new(i as u32);
        match module.constants().get(id).unwrap() {
            ConstantData::Int { ty, width, bits } => {
                let signed = matches!(
                    module.types().get(*ty),
                    Some(crate::types::TypeData::Int { signed: true, .. })
                );
                let value = if signed {
                    format!("{}", crate::constant::ConstantStore::sign_extend(*bits, *width))
                } else {
                    format!("{bits}")
                };
                let _ = writeln!(
                    out,
                    "::ASTRONOMY::CONSTANT_INT c{i} {} {}",
                    module.type_name(*ty),
                    value
                );
            }
            ConstantData::Float { ty, bits } => {
                let text = if *ty == TypeId::F32 {
                    format_f32(*bits as u32)
                } else {
                    format_f64(*bits)
                };
                let _ = writeln!(
                    out,
                    "::ASTRONOMY::CONSTANT_FLOAT c{i} {} {text}",
                    module.type_name(*ty)
                );
            }
            ConstantData::Null { pointee } => {
                let _ = writeln!(
                    out,
                    "::ASTRONOMY::CONSTANT_NULL c{i} ptr<{}>",
                    module.type_name(*pointee)
                );
            }
            ConstantData::String { bytes } => {
                let _ = writeln!(
                    out,
                    "::ASTRONOMY::CONSTANT_STRING c{i} \"{}\"",
                    escape_string(bytes)
                );
            }
            ConstantData::Aggregate { ty, elements } => {
                let parts: Vec<String> = elements
                    .iter()
                    .map(|c| format!("c{}", c.as_u32()))
                    .collect();
                let _ = writeln!(
                    out,
                    "::ASTRONOMY::CONSTANT_AGGREGATE c{i} {} {}",
                    module.type_name(*ty),
                    parts.join(" ")
                );
            }
        }
    }
    let _ = writeln!(out, "::ASTRONOMY::CONSTANTS_END");
}

/// Per-function naming context: printed handles for values and labels for
/// blocks. Anonymous values are numbered by emission order; named values
/// keep their names. Value names can never be digit-only, and anonymous
/// block labels (`bbN`) are reserved, so printed names are unambiguous.
struct Names {
    values: HashMap<ValueId, String>,
    blocks: HashMap<BlockId, String>,
}

fn compute_names(module: &Module, f: &Function, order: &[BlockId]) -> Names {
    let mut names = Names {
        values: HashMap::new(),
        blocks: HashMap::new(),
    };
    let mut counter = 0u32;
    let mut block_counter = 0u32;

    // Debug names and block labels are not required to be unique (§52 uses
    // `%x` in two blocks). To keep printed text unambiguous, values/labels
    // with a duplicated name fall back to anonymous numbering — a
    // deterministic choice that preserves canonical roundtrips (§32, §33).
    let mut name_count: HashMap<crate::id::SymbolId, u32> = HashMap::new();
    for v in &f.values {
        if let Some(n) = v.name {
            *name_count.entry(n).or_insert(0) += 1;
        }
    }
    let mut label_count: HashMap<crate::id::SymbolId, u32> = HashMap::new();
    for b in &f.blocks {
        if let Some(n) = b.name {
            *label_count.entry(n).or_insert(0) += 1;
        }
    }

    let mut value_name = |named: Option<crate::id::SymbolId>| -> String {
        match named.filter(|n| name_count.get(n) == Some(&1)) {
            Some(n) => module.symbol_name(n).unwrap_or("?").to_string(),
            None => {
                let s = counter.to_string();
                counter += 1;
                s
            }
        }
    };

    for p in &f.params {
        let name = value_name(f.value(p.value).and_then(|d| d.name));
        names.values.insert(p.value, name);
    }
    for &b in order {
        let block = &f.blocks[b.index()];
        // Labels of the form `bb<digits>` are reserved for the printer's
        // anonymous numbering (§32) and never used for named blocks.
        let label = match block
            .name
            .filter(|n| label_count.get(n) == Some(&1))
            .and_then(|s| module.symbol_name(s))
        {
            Some(n) if !crate::builder::is_reserved_block_name(n) => n.to_string(),
            _ => {
                let l = format!("bb{block_counter}");
                block_counter += 1;
                l
            }
        };
        names.blocks.insert(b, label);
        for param in &block.params {
            let name = value_name(f.value(param.value).and_then(|d| d.name));
            names.values.insert(param.value, name);
        }
        for inst in &block.instructions {
            if let Some(result) = inst.result {
                let name = value_name(f.value(result).and_then(|d| d.name));
                names.values.insert(result, name);
            }
        }
    }
    names
}

/// Block emission order: reverse post-order from the entry block, then
/// unreachable blocks in index order.
fn block_order(f: &Function) -> Vec<BlockId> {
    if f.blocks.is_empty() {
        return Vec::new();
    }
    let info = crate::verify::cfg::CfgInfo::compute(f);
    let mut reachable: Vec<BlockId> = (0..f.blocks.len())
        .map(|i| BlockId::new(i as u32))
        .filter(|b| info.reachable[b.index()])
        .collect();
    reachable.sort_by_key(|b| info.rpo_number[b.index()]);
    let unreachable: Vec<BlockId> = (0..f.blocks.len())
        .map(|i| BlockId::new(i as u32))
        .filter(|b| !info.reachable[b.index()])
        .collect();
    reachable.extend(unreachable);
    reachable
}

fn print_function(module: &Module, f: &Function, out: &mut String) {
    let name = module
        .symbol_name(f.symbol)
        .unwrap_or("<invalid>");
    let order = block_order(f);
    let names = compute_names(module, f, &order);

    // Header.
    let mut params = String::new();
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            params.push_str(", ");
        }
        params.push_str(&module.type_name(p.ty));
        // Declarations omit anonymous parameter handles; definitions keep
        // them so every value has a printed name.
        let named = f
            .value(p.value)
            .and_then(|d| d.name)
            .and_then(|s| module.symbol_name(s));
        match named {
            Some(n) => params.push_str(&format!(" %{n}")),
            None if !f.is_declaration() => {
                params.push_str(&format!(" %{}", names.values[&p.value]));
            }
            _ => {}
        }
    }
    if f.variadic {
        if !f.params.is_empty() {
            params.push_str(", ");
        }
        params.push_str("...");
    }
    let abi = match f.abi {
        Abi::Astronomy => "astronomy".to_string(),
        Abi::C => "c".to_string(),
        Abi::System => "system".to_string(),
        Abi::Custom(sym) => format!(
            "custom {}",
            module.symbol_name(sym).unwrap_or("<invalid>")
        ),
    };
    let linkage = match f.linkage {
        Linkage::Internal => "internal",
        Linkage::External => "external",
        Linkage::Exported => "exported",
    };
    let _ = writeln!(
        out,
        "::ASTRONOMY::FUNCTION_START {name} fn({params}) -> {} linkage={linkage} abi={abi}",
        module.type_name(f.result)
    );

    for b in order {
        let block = &f.blocks[b.index()];
        let label = &names.blocks[&b];
        if block.params.is_empty() {
            let _ = writeln!(out, "::ASTRONOMY::BLOCK_START {label}");
        } else {
            let params: Vec<String> = block
                .params
                .iter()
                .map(|p| {
                    format!(
                        "{} %{}",
                        module.type_name(p.ty),
                        names.values[&p.value]
                    )
                })
                .collect();
            let _ = writeln!(out, "::ASTRONOMY::BLOCK_START {label}({})", params.join(", "));
        }

        for inst in &block.instructions {
            print_instruction(module, f, &names, inst, out);
        }
        if let Some(term) = &block.terminator {
            print_terminator(module, f, &names, term, out);
        }
        let _ = writeln!(out, "::ASTRONOMY::BLOCK_END");
    }
    let _ = writeln!(out, "::ASTRONOMY::FUNCTION_END");
}

fn op(v: ValueId, names: &Names, module: &Module, f: &Function) -> String {
    let ty = f.value(v).map(|d| d.ty).unwrap_or(TypeId::VOID);
    format!(
        "{} %{}",
        module.type_name(ty),
        names.values.get(&v).cloned().unwrap_or_else(|| "?".to_string())
    )
}

fn print_instruction(
    module: &Module,
    f: &Function,
    names: &Names,
    inst: &crate::instruction::Instruction,
    out: &mut String,
) {
    use InstructionKind as K;
    let result_prefix = inst
        .result
        .map(|r| {
            format!(
                "%{} = ",
                names.values.get(&r).cloned().unwrap_or_else(|| "?".into())
            )
        })
        .unwrap_or_default();
    let result_ty = inst
        .result
        .and_then(|r| f.value(r))
        .map(|d| module.type_name(d.ty))
        .unwrap_or_else(|| "void".to_string());

    let line = match &inst.kind {
        K::Const(cid) => format!(
            "{result_prefix}::ASTRONOMY::CONST {result_ty} c{}",
            cid.as_u32()
        ),
        K::Add { lhs, rhs } => bin(&result_prefix, &result_ty, "ADD", *lhs, *rhs, names, module, f),
        K::Sub { lhs, rhs } => bin(&result_prefix, &result_ty, "SUB", *lhs, *rhs, names, module, f),
        K::Mul { lhs, rhs } => bin(&result_prefix, &result_ty, "MUL", *lhs, *rhs, names, module, f),
        K::Div { lhs, rhs } => bin(&result_prefix, &result_ty, "DIV", *lhs, *rhs, names, module, f),
        K::Rem { lhs, rhs } => bin(&result_prefix, &result_ty, "REM", *lhs, *rhs, names, module, f),
        K::And { lhs, rhs } => bin(&result_prefix, &result_ty, "AND", *lhs, *rhs, names, module, f),
        K::Or { lhs, rhs } => bin(&result_prefix, &result_ty, "OR", *lhs, *rhs, names, module, f),
        K::Xor { lhs, rhs } => bin(&result_prefix, &result_ty, "XOR", *lhs, *rhs, names, module, f),
        K::Shl { lhs, rhs } => bin(&result_prefix, &result_ty, "SHL", *lhs, *rhs, names, module, f),
        K::Shr { lhs, rhs } => bin(&result_prefix, &result_ty, "SHR", *lhs, *rhs, names, module, f),
        K::Eq { lhs, rhs } => bin(&result_prefix, &result_ty, "EQ", *lhs, *rhs, names, module, f),
        K::Ne { lhs, rhs } => bin(&result_prefix, &result_ty, "NE", *lhs, *rhs, names, module, f),
        K::Lt { lhs, rhs } => bin(&result_prefix, &result_ty, "LT", *lhs, *rhs, names, module, f),
        K::Le { lhs, rhs } => bin(&result_prefix, &result_ty, "LE", *lhs, *rhs, names, module, f),
        K::Gt { lhs, rhs } => bin(&result_prefix, &result_ty, "GT", *lhs, *rhs, names, module, f),
        K::Ge { lhs, rhs } => bin(&result_prefix, &result_ty, "GE", *lhs, *rhs, names, module, f),
        K::Alloca { pointee } => format!(
            "{result_prefix}::ASTRONOMY::ALLOCA {result_ty} {}",
            module.type_name(*pointee)
        ),
        K::Load { ty, pointer } => format!(
            "{result_prefix}::ASTRONOMY::LOAD {} {}",
            module.type_name(*ty),
            op(*pointer, names, module, f)
        ),
        K::Store { pointer, value } => format!(
            "::ASTRONOMY::STORE {}, {}",
            op(*pointer, names, module, f),
            op(*value, names, module, f)
        ),
        K::PtrOffset { pointer, offset } => format!(
            "{result_prefix}::ASTRONOMY::PTR_OFFSET {result_ty} {}, {}",
            op(*pointer, names, module, f),
            op(*offset, names, module, f)
        ),
        K::Ext { to, value } => format!(
            "{result_prefix}::ASTRONOMY::EXT {} {}",
            module.type_name(*to),
            op(*value, names, module, f)
        ),
        K::Trunc { to, value } => format!(
            "{result_prefix}::ASTRONOMY::TRUNC {} {}",
            module.type_name(*to),
            op(*value, names, module, f)
        ),
        K::IntToFloat { to, value } => format!(
            "{result_prefix}::ASTRONOMY::INT_TO_FLOAT {} {}",
            module.type_name(*to),
            op(*value, names, module, f)
        ),
        K::FloatToInt { to, value } => format!(
            "{result_prefix}::ASTRONOMY::FLOAT_TO_INT {} {}",
            module.type_name(*to),
            op(*value, names, module, f)
        ),
        K::PtrCast { to, pointer } => format!(
            "{result_prefix}::ASTRONOMY::PTR_CAST {} {}",
            module.type_name(*to),
            op(*pointer, names, module, f)
        ),
        K::Call { callee, args } => {
            let rendered: Vec<String> =
                args.iter().map(|&a| op(a, names, module, f)).collect();
            format!(
                "{result_prefix}::ASTRONOMY::CALL {result_ty} @{}({})",
                module.function_name(*callee),
                rendered.join(", ")
            )
        }
        K::Construct { ty, fields } => {
            let rendered: Vec<String> =
                fields.iter().map(|&a| op(a, names, module, f)).collect();
            format!(
                "{result_prefix}::ASTRONOMY::CONSTRUCT {} ({})",
                module.type_name(*ty),
                rendered.join(", ")
            )
        }
        K::Extract { aggregate, index } => format!(
            "{result_prefix}::ASTRONOMY::EXTRACT {result_ty} {}, {index}",
            op(*aggregate, names, module, f)
        ),
        K::Insert {
            aggregate,
            index,
            value,
        } => format!(
            "{result_prefix}::ASTRONOMY::INSERT {result_ty} {}, {index}, {}",
            op(*aggregate, names, module, f),
            op(*value, names, module, f)
        ),
    };
    let _ = writeln!(out, "{line}");
}

#[allow(clippy::too_many_arguments)]
fn bin(
    result_prefix: &str,
    result_ty: &str,
    opcode: &str,
    lhs: ValueId,
    rhs: ValueId,
    names: &Names,
    module: &Module,
    f: &Function,
) -> String {
    format!(
        "{result_prefix}::ASTRONOMY::{opcode} {result_ty} {}, {}",
        op(lhs, names, module, f),
        op(rhs, names, module, f)
    )
}

fn print_terminator(
    module: &Module,
    f: &Function,
    names: &Names,
    term: &Terminator,
    out: &mut String,
) {
    let line = match term {
        Terminator::Jump { target, args } => {
            let label = names.blocks.get(target).cloned().unwrap_or_else(|| "?".into());
            if args.is_empty() {
                format!("::ASTRONOMY::JUMP {label}")
            } else {
                let rendered: Vec<String> =
                    args.iter().map(|&a| op(a, names, module, f)).collect();
                format!("::ASTRONOMY::JUMP {label}({})", rendered.join(", "))
            }
        }
        Terminator::Branch {
            condition,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let render = |block: &BlockId, args: &[ValueId]| -> String {
                let label = names.blocks.get(block).cloned().unwrap_or_else(|| "?".into());
                if args.is_empty() {
                    label
                } else {
                    let rendered: Vec<String> =
                        args.iter().map(|&a| op(a, names, module, f)).collect();
                    format!("{label}({})", rendered.join(", "))
                }
            };
            format!(
                "::ASTRONOMY::BRANCH {}, {}, {}",
                op(*condition, names, module, f),
                render(then_block, then_args),
                render(else_block, else_args)
            )
        }
        Terminator::Return { value } => match value {
            Some(v) => format!("::ASTRONOMY::RETURN {}", op(*v, names, module, f)),
            None => "::ASTRONOMY::RETURN void".to_string(),
        },
        Terminator::Unreachable => "::ASTRONOMY::UNREACHABLE".to_string(),
    };
    let _ = writeln!(out, "{line}");
}

fn format_f32(bits: u32) -> String {
    let v = f32::from_bits(bits);
    if v.is_nan() {
        "nan".to_string()
    } else if v.is_infinite() {
        if v < 0.0 { "-inf" } else { "inf" }.to_string()
    } else {
        // Exponent form keeps magnitudes like 1e300 compact and, like
        // Display, uses the shortest representation that roundtrips
        // exactly (§32, §33). The sign of zero is preserved.
        format!("{v:e}")
    }
}

fn format_f64(bits: u64) -> String {
    let v = f64::from_bits(bits);
    if v.is_nan() {
        "nan".to_string()
    } else if v.is_infinite() {
        if v < 0.0 { "-inf" } else { "inf" }.to_string()
    } else {
        format!("{v:e}")
    }
}

/// Escapes bytes for a double-quoted ARN string literal.
pub(crate) fn escape_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02X}")),
        }
    }
    out
}
