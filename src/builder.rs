//! Builder API for frontends (§29 Builder).
//!
//! The builder is the first-class way to construct Astronomy IR from Rust:
//!
//! ```no_run
//! use astronomy::{Abi, Linkage, ModuleBuilder, TypeId};
//!
//! # fn main() -> Result<(), astronomy::BuildError> {
//! let mut b = ModuleBuilder::new();
//! let add = b.declare_function(
//!     "add",
//!     Linkage::Exported,
//!     Abi::Astronomy,
//!     &[("a", TypeId::I64), ("b", TypeId::I64)],
//!     TypeId::I64,
//! )?;
//! let mut fb = b.function_builder(add)?;
//! let entry = fb.append_block();
//! let (a, b_) = (fb.param(0), fb.param(1));
//! let sum = fb.add(a, b_)?;
//! fb.ret(Some(sum))?;
//! let module = b.finish();
//! # Ok(())
//! # }
//! ```
//!
//! The builder prevents common mistakes (unknown operands, type mismatches,
//! double terminators) but the [`crate::Verifier`] remains the final
//! authority.

use crate::block::{BasicBlock, BlockParam};
use crate::constant::ConstantData;
use crate::error::BuildError;
use crate::function::{Abi, Function, Linkage};
use crate::id::{BlockId, ConstantId, FunctionId, SymbolId, TypeId, ValueId};
use crate::instruction::{Instruction, InstructionKind};
use crate::module::Module;
use crate::types::TypeData;
use crate::value::{ValueData, ValueKind};
use crate::Terminator;

/// Returns true for a valid IR identifier (`[A-Za-z_][A-Za-z0-9_]*`).
pub(crate) fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Returns true for names reserved for anonymous block labels (`bb[0-9]+`).
pub(crate) fn is_reserved_block_name(s: &str) -> bool {
    let rest = s.strip_prefix("bb").unwrap_or("");
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// Fluent module builder (§29).
#[derive(Debug, Clone)]
pub struct ModuleBuilder {
    module: Module,
}

impl ModuleBuilder {
    /// Creates an empty builder.
    pub fn new() -> Self {
        ModuleBuilder {
            module: Module::new(),
        }
    }

    /// Creates a builder with a module name.
    pub fn with_name(name: &str) -> Self {
        let mut b = Self::new();
        b.set_module_name(name);
        b
    }

    /// Sets the module name.
    pub fn set_module_name(&mut self, name: &str) {
        self.module.set_name(name);
    }

    /// Borrows the module under construction.
    pub fn module(&self) -> &Module {
        &self.module
    }

    /// Consumes the builder, returning the module.
    pub fn finish(self) -> Module {
        self.module
    }

    /// Consumes the builder, returning the module.
    pub fn into_module(self) -> Module {
        self.module
    }

    // -- types -----------------------------------------------------------

    /// Mutably borrows the type store, for interning composite types.
    pub fn types(&mut self) -> &mut crate::types::TypeStore {
        &mut self.module.types
    }

    /// Interns `ptr<pointee>` in address space 0.
    pub fn ptr_type(&mut self, pointee: TypeId) -> TypeId {
        self.module.types.ptr(pointee)
    }

    /// Interns `array<element, length>`.
    pub fn array_type(&mut self, element: TypeId, length: u64) -> TypeId {
        self.module.types.array(element, length)
    }

    /// Interns `struct<...fields>`.
    pub fn struct_type(&mut self, fields: &[TypeId]) -> TypeId {
        self.module.types.struct_of(fields)
    }

    /// Interns a function signature type.
    pub fn fn_type(&mut self, params: &[TypeId], variadic: bool, result: TypeId) -> TypeId {
        self.module.types.function(params, variadic, result)
    }

    // -- symbols / constants ----------------------------------------------

    /// Interns a symbol name.
    pub fn intern_symbol(&mut self, name: &str) -> SymbolId {
        self.module.symbols.intern(name)
    }

    /// Interns raw constant data (§30 Direct Construction).
    pub fn intern_constant_data(&mut self, data: ConstantData) -> ConstantId {
        self.module.constants.intern(data)
    }

    // -- functions ---------------------------------------------------------

    /// Declares a function and returns its ID. Parameters are given as
    /// `(name, type)` pairs; an empty name (`""`) leaves the parameter
    /// anonymous. A function declared this way starts without a body; use
    /// [`ModuleBuilder::function_builder`] to fill it in.
    pub fn declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        abi: Abi,
        params: &[(&str, TypeId)],
        result: TypeId,
    ) -> Result<FunctionId, BuildError> {
        self.declare_function_inner(name, linkage, abi, params, result, false)
    }

    /// Declares an external (extern) function. `fixed_params` are the
    /// non-variadic parameters; when `variadic` is true, calls may append
    /// extra first-class arguments.
    pub fn declare_extern(
        &mut self,
        name: &str,
        abi: Abi,
        fixed_params: &[TypeId],
        variadic: bool,
        result: TypeId,
    ) -> Result<FunctionId, BuildError> {
        let params: Vec<(&str, TypeId)> = fixed_params.iter().map(|&t| ("", t)).collect();
        self.declare_function_inner(name, Linkage::External, abi, &params, result, variadic)
    }

    fn declare_function_inner(
        &mut self,
        name: &str,
        linkage: Linkage,
        abi: Abi,
        params: &[(&str, TypeId)],
        result: TypeId,
        variadic: bool,
    ) -> Result<FunctionId, BuildError> {
        if !is_valid_ident(name) {
            return Err(BuildError::InvalidName {
                name: name.to_string(),
                reason: "function names must be identifiers",
            });
        }
        if self.module.function_by_name(name).is_some() {
            return Err(BuildError::DuplicateFunctionName {
                name: name.to_string(),
            });
        }
        // Signatures must be built from valid, first-class types.
        let types = &self.module.types;
        for &(_, ty) in params {
            let ok = types.get(ty).map(|d| d.is_first_class()).unwrap_or(false);
            if !ok {
                return Err(BuildError::InvalidSignature {
                    reason: format!(
                        "parameter type `{}` is not a first-class type",
                        crate::types::type_to_string(types, ty)
                    ),
                });
            }
        }
        let result_ok = types
            .get(result)
            .map(|d| d.is_first_class() || d.is_void())
            .unwrap_or(false);
        if !result_ok {
            return Err(BuildError::InvalidSignature {
                reason: format!(
                    "result type `{}` must be first-class or void",
                    crate::types::type_to_string(types, result)
                ),
            });
        }
        if variadic && !matches!(linkage, Linkage::External) {
            return Err(BuildError::InvalidSignature {
                reason: "only external declarations may be variadic".to_string(),
            });
        }

        let symbol = self.module.symbols.intern(name);
        let params: Vec<(Option<SymbolId>, TypeId)> = params
            .iter()
            .map(|&(n, t)| {
                let sym = if n.is_empty() {
                    None
                } else {
                    Some(self.module.symbols.intern(n))
                };
                (sym, t)
            })
            .collect();
        let function = Function::new(symbol, linkage, abi, params, result, variadic);
        self.module.functions.push(function);
        Ok(FunctionId::new(self.module.functions.len() as u32 - 1))
    }

    /// Declares a function and returns a builder cursor for its body in one
    /// step.
    pub fn build_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        abi: Abi,
        params: &[(&str, TypeId)],
        result: TypeId,
    ) -> Result<FunctionBuilder<'_>, BuildError> {
        let id = self.declare_function(name, linkage, abi, params, result)?;
        self.function_builder(id)
    }

    /// Returns a builder cursor for an already-declared function.
    pub fn function_builder(&mut self, id: FunctionId) -> Result<FunctionBuilder<'_>, BuildError> {
        if self.module.function(id).is_none() {
            return Err(BuildError::UnknownFunction { function: id });
        }
        Ok(FunctionBuilder {
            module: &mut self.module,
            function: id,
            current: None,
        })
    }
}

impl Default for ModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Classified operand requirements for binary operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinClass {
    /// Arithmetic integers or floats (`add`, `sub`, `mul`).
    ArithOrFloat,
    /// Arithmetic integers or floats (`div`).
    ArithOrFloatDiv,
    /// Arithmetic integers only (`rem`).
    ArithInt,
    /// Any integer width including `i1` (`and`, `or`, `xor`).
    AnyInt,
    /// Arithmetic integers (`shl`, `shr`).
    ArithIntShift,
    /// Integers, floats or pointers (`eq`, `ne`).
    CmpEquality,
    /// Arithmetic integers or floats (`lt`, `le`, `gt`, `ge`).
    CmpOrdered,
}

/// A cursor for building one function body (§29).
///
/// Instructions are appended to the *current block*; terminators complete
/// the current block and release the cursor to be switched elsewhere.
pub struct FunctionBuilder<'m> {
    module: &'m mut Module,
    function: FunctionId,
    current: Option<BlockId>,
}

impl<'m> FunctionBuilder<'m> {
    fn f(&self) -> &Function {
        &self.module.functions[self.function.index()]
    }

    fn f_mut(&mut self) -> &mut Function {
        &mut self.module.functions[self.function.index()]
    }

    fn block(&self, id: BlockId) -> Result<&BasicBlock, BuildError> {
        self.f()
            .blocks
            .get(id.index())
            .ok_or(BuildError::UnknownBlock { block: id })
    }

    fn ty_of(&self, v: ValueId) -> Result<TypeId, BuildError> {
        self.f()
            .value(v)
            .map(|d| d.ty)
            .ok_or(BuildError::UnknownValue {
                value: v,
                context: "operand lookup",
            })
    }

    fn type_name(&self, ty: TypeId) -> String {
        self.module.type_name(ty)
    }

    // -- structure ---------------------------------------------------------

    /// The function being built.
    pub fn function_id(&self) -> FunctionId {
        self.function
    }

    /// The value of function parameter `index`.
    pub fn param(&self, index: usize) -> ValueId {
        self.f().params[index].value
    }

    /// The value of block parameter `index` of `block`.
    pub fn block_param(&self, block: BlockId, index: usize) -> Result<ValueId, BuildError> {
        self.block(block)?
            .params
            .get(index)
            .map(|p| p.value)
            .ok_or(BuildError::UnknownBlock { block })
    }

    /// Appends a new anonymous block and switches to it. The first block
    /// appended is the entry block.
    pub fn append_block(&mut self) -> BlockId {
        let id = BlockId::new(self.f().blocks.len() as u32);
        self.f_mut().blocks.push(BasicBlock::new());
        self.current = Some(id);
        id
    }

    /// Appends a new named block and switches to it.
    pub fn append_named_block(&mut self, name: &str) -> Result<BlockId, BuildError> {
        self.check_block_name(name)?;
        let id = self.append_block();
        self.f_mut().blocks[id.index()].name = Some(self.module.symbols.intern(name));
        Ok(id)
    }

    /// Appends a block with parameters (block arguments, §14) and switches
    /// to it. The entry block must not take parameters.
    pub fn append_block_with_params(
        &mut self,
        params: &[(&str, TypeId)],
    ) -> Result<BlockId, BuildError> {
        let id = self.append_block();
        for (name, ty) in params {
            self.add_block_param_named(id, name, *ty)?;
        }
        Ok(id)
    }

    /// Adds an anonymous parameter to `block`. Only allowed while the block
    /// is empty, and never on the entry block.
    pub fn add_block_param(&mut self, block: BlockId, ty: TypeId) -> Result<ValueId, BuildError> {
        self.add_block_param_named(block, "", ty)
    }

    /// Adds a named parameter to `block`.
    pub fn add_block_param_named(
        &mut self,
        block: BlockId,
        name: &str,
        ty: TypeId,
    ) -> Result<ValueId, BuildError> {
        if !name.is_empty() {
            self.check_value_name(name)?;
        }
        if self.f().blocks.is_empty() || block.index() >= self.f().blocks.len() {
            return Err(BuildError::UnknownBlock { block });
        }
        if block.index() == 0 {
            return Err(BuildError::BlockNotEmpty {
                block,
                reason: "the entry block cannot take parameters (use function parameters)",
            });
        }
        {
            let b = self.block(block)?;
            if !b.instructions.is_empty() || b.terminator.is_some() {
                return Err(BuildError::BlockNotEmpty {
                    block,
                    reason: "parameters must be added before instructions",
                });
            }
        }
        if !self
            .module
            .types
            .get(ty)
            .map(|d| d.is_first_class())
            .unwrap_or(false)
        {
            return Err(BuildError::InvalidOperand {
                context: "block parameter",
                reason: format!("`{}` is not a first-class type", self.type_name(ty)),
            });
        }
        let index = self.f().blocks[block.index()].params.len() as u32;
        let value = ValueId::new(self.f().values.len() as u32);
        let name_sym = if name.is_empty() {
            None
        } else {
            Some(self.module.symbols.intern(name))
        };
        self.f_mut().values.push(ValueData {
            ty,
            kind: ValueKind::BlockParam { block, index },
            name: name_sym,
            span: None,
        });
        self.f_mut().blocks[block.index()].params.push(BlockParam {
            value,
            ty,
        });
        Ok(value)
    }

    /// Switches the cursor to `block`.
    pub fn switch_to(&mut self, block: BlockId) -> Result<(), BuildError> {
        self.block(block)?;
        self.current = Some(block);
        Ok(())
    }

    /// The currently selected block.
    pub fn current_block(&self) -> Option<BlockId> {
        self.current
    }

    /// Attaches a debug name to a value (must be a valid identifier).
    pub fn set_value_name(&mut self, v: ValueId, name: &str) -> Result<(), BuildError> {
        self.check_value_name(name)?;
        let sym = self.module.symbols.intern(name);
        let f = self.f_mut();
        if v.index() >= f.values.len() {
            return Err(BuildError::UnknownValue {
                value: v,
                context: "set_value_name",
            });
        }
        f.values[v.index()].name = Some(sym);
        Ok(())
    }

    fn check_block_name(&self, name: &str) -> Result<(), BuildError> {
        if !is_valid_ident(name) {
            return Err(BuildError::InvalidName {
                name: name.to_string(),
                reason: "block names must be identifiers",
            });
        }
        if is_reserved_block_name(name) {
            return Err(BuildError::InvalidName {
                name: name.to_string(),
                reason: "names of the form `bb<digits>` are reserved for anonymous blocks",
            });
        }
        Ok(())
    }

    fn check_value_name(&self, name: &str) -> Result<(), BuildError> {
        if !is_valid_ident(name) {
            return Err(BuildError::InvalidName {
                name: name.to_string(),
                reason: "value names must be identifiers",
            });
        }
        Ok(())
    }

    // -- instruction emission ----------------------------------------------

    fn current(&self) -> Result<BlockId, BuildError> {
        self.current
            .ok_or(BuildError::NoCurrentBlock { action: "emit" })
    }

    fn append(
        &mut self,
        kind: InstructionKind,
        result_ty: Option<TypeId>,
    ) -> Result<Option<ValueId>, BuildError> {
        let block = self.current()?;
        if self.f().blocks[block.index()].terminator.is_some() {
            return Err(BuildError::BlockTerminated { block });
        }
        let index = self.f().blocks[block.index()].instructions.len() as u32;
        let result = result_ty.map(|ty| {
            let v = ValueId::new(self.f().values.len() as u32);
            self.f_mut().values.push(ValueData::new(
                ty,
                ValueKind::Inst { block, index },
            ));
            v
        });
        self.f_mut().blocks[block.index()]
            .instructions
            .push(Instruction::new(kind, result));
        Ok(result)
    }

    fn binop<F>(
        &mut self,
        class: BinClass,
        opname: &'static str,
        lhs: ValueId,
        rhs: ValueId,
        mk: F,
    ) -> Result<ValueId, BuildError>
    where
        F: FnOnce(ValueId, ValueId) -> InstructionKind,
    {
        let lt = self.ty_of(lhs)?;
        let rt = self.ty_of(rhs)?;
        if lt != rt {
            return Err(BuildError::TypeMismatch {
                context: format!("{opname} operands"),
                expected: self.type_name(lt),
                found: self.type_name(rt),
            });
        }
        let data = self.module.types.get(lt).cloned();
        let ok = match (class, data.as_ref()) {
            (BinClass::ArithOrFloat, Some(d)) => d.is_arith_integer() || d.is_float(),
            (BinClass::ArithOrFloatDiv, Some(d)) => {
                d.is_arith_integer() || d.is_float()
            }
            (BinClass::ArithInt, Some(d)) => d.is_arith_integer(),
            (BinClass::AnyInt, Some(d)) => d.is_integer(),
            (BinClass::ArithIntShift, Some(d)) => d.is_arith_integer(),
            (BinClass::CmpEquality, Some(d)) => {
                d.is_integer() || d.is_float() || d.is_pointer()
            }
            (BinClass::CmpOrdered, Some(d)) => d.is_arith_integer() || d.is_float(),
            (_, None) => false,
        };
        if !ok {
            let found = self
                .module
                .types
                .get(lt)
                .map(|_| self.type_name(lt))
                .unwrap_or_else(|| format!("<invalid {lt:?}>"));
            return Err(BuildError::InvalidOperand {
                context: opname,
                reason: format!("operand type `{found}` is not supported by {opname}"),
            });
        }
        let result_ty = if matches!(
            class,
            BinClass::CmpEquality | BinClass::CmpOrdered
        ) {
            TypeId::I1
        } else {
            lt
        };
        self.append(mk(lhs, rhs), Some(result_ty))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: opname,
                reason: "expected an instruction result".into(),
            })
    }

    /// `const` — materialize an integer constant (value wraps to the type
    /// width).
    pub fn const_int(&mut self, ty: TypeId, value: i128) -> Result<ValueId, BuildError> {
        let width = match self.module.types.get(ty) {
            Some(TypeData::Int { bits, .. }) => *bits,
            _ => {
                return Err(BuildError::InvalidOperand {
                    context: "const_int",
                    reason: format!("`{}` is not an integer type", self.type_name(ty)),
                })
            }
        };
        let bits = crate::constant::ConstantStore::mask_signed(value, width);
        let cid = self.module.intern_constant(ConstantData::Int {
            ty,
            width,
            bits,
        });
        self.append(InstructionKind::Const(cid), Some(ty))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "const_int",
                reason: "expected an instruction result".into(),
            })
    }

    /// `const` — materialize an unsigned integer constant.
    pub fn const_uint(&mut self, ty: TypeId, value: u128) -> Result<ValueId, BuildError> {
        let width = match self.module.types.get(ty) {
            Some(TypeData::Int { bits, .. }) => *bits,
            _ => {
                return Err(BuildError::InvalidOperand {
                    context: "const_uint",
                    reason: format!("`{}` is not an integer type", self.type_name(ty)),
                })
            }
        };
        let bits = crate::constant::ConstantStore::mask_unsigned(value, width);
        let cid = self.module.intern_constant(ConstantData::Int {
            ty,
            width,
            bits,
        });
        self.append(InstructionKind::Const(cid), Some(ty))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "const_uint",
                reason: "expected an instruction result".into(),
            })
    }

    /// `const` — materialize an `f32` constant.
    pub fn const_f32(&mut self, value: f32) -> Result<ValueId, BuildError> {
        let cid = self.module.intern_constant(ConstantData::Float {
            ty: TypeId::F32,
            bits: value.to_bits() as u64,
        });
        self.append(InstructionKind::Const(cid), Some(TypeId::F32))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "const_f32",
                reason: "expected an instruction result".into(),
            })
    }

    /// `const` — materialize an `f64` constant.
    pub fn const_f64(&mut self, value: f64) -> Result<ValueId, BuildError> {
        let cid = self.module.intern_constant(ConstantData::Float {
            ty: TypeId::F64,
            bits: value.to_bits(),
        });
        self.append(InstructionKind::Const(cid), Some(TypeId::F64))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "const_f64",
                reason: "expected an instruction result".into(),
            })
    }

    /// `const` — materialize a null pointer of type `ptr<pointee>`.
    pub fn const_null(&mut self, pointee: TypeId) -> Result<ValueId, BuildError> {
        if !self
            .module
            .types
            .get(pointee)
            .map(|d| d.is_first_class())
            .unwrap_or(false)
        {
            return Err(BuildError::InvalidOperand {
                context: "const_null",
                reason: format!(
                    "`{}` is not a valid pointee type",
                    self.type_name(pointee)
                ),
            });
        }
        let cid = self
            .module
            .intern_constant(ConstantData::Null { pointee });
        let ptr = self.module.types.ptr(pointee);
        self.append(InstructionKind::Const(cid), Some(ptr))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "const_null",
                reason: "expected an instruction result".into(),
            })
    }

    /// `const` — materialize a string constant of type `ptr<i8>`.
    pub fn const_string(&mut self, bytes: &[u8]) -> Result<ValueId, BuildError> {
        let cid = self
            .module
            .intern_constant(ConstantData::String { bytes: bytes.to_vec() });
        let ptr = self.module.types.ptr(TypeId::I8);
        self.append(InstructionKind::Const(cid), Some(ptr))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "const_string",
                reason: "expected an instruction result".into(),
            })
    }

    /// `const` — materialize an aggregate constant from pool elements.
    pub fn const_aggregate(
        &mut self,
        ty: TypeId,
        elements: &[ConstantId],
    ) -> Result<ValueId, BuildError> {
        self.check_aggregate_constant(ty, elements, None)?;
        let cid = self.module.intern_constant(ConstantData::Aggregate {
            ty,
            elements: elements.to_vec(),
        });
        self.append(InstructionKind::Const(cid), Some(ty))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "const_aggregate",
                reason: "expected an instruction result".into(),
            })
    }

    fn check_aggregate_constant(
        &self,
        ty: TypeId,
        elements: &[ConstantId],
        self_index: Option<usize>,
    ) -> Result<(), BuildError> {
        let expected_fields: Vec<TypeId> = match self.module.types.get(ty) {
            Some(TypeData::Array { element, length }) => {
                if elements.len() as u64 != *length {
                    return Err(BuildError::InvalidConstant {
                        reason: format!(
                            "array constant of `{}` needs {length} element(s), found {}",
                            self.type_name(ty),
                            elements.len()
                        ),
                    });
                }
                vec![*element; elements.len()]
            }
            Some(TypeData::Struct { fields }) => {
                if fields.len() != elements.len() {
                    return Err(BuildError::InvalidConstant {
                        reason: format!(
                            "struct constant of `{}` needs {} field(s), found {}",
                            self.type_name(ty),
                            fields.len(),
                            elements.len()
                        ),
                    });
                }
                fields.clone()
            }
            _ => {
                return Err(BuildError::InvalidConstant {
                    reason: format!("`{}` is not an aggregate type", self.type_name(ty)),
                })
            }
        };
        for (i, (&cid, &field)) in elements.iter().zip(expected_fields.iter()).enumerate() {
            let data = self.module.constants.get(cid).ok_or(BuildError::InvalidConstant {
                reason: format!("element {i} references unknown constant c{}", cid.as_u32()),
            })?;
            if let Some(si) = self_index {
                if cid.index() >= si {
                    return Err(BuildError::InvalidConstant {
                        reason: format!(
                            "element {i} must reference an earlier constant (forward or self reference)"
                        ),
                    });
                }
            }
            if !crate::error::constant_fits_type(&self.module.types, data, field) {
                return Err(BuildError::InvalidConstant {
                    reason: format!(
                        "element {i} does not match field type `{}`",
                        self.type_name(field)
                    ),
                });
            }
        }
        Ok(())
    }

    /// Wrapping integer/float addition.
    pub fn add(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::ArithOrFloat, "add", lhs, rhs, |l, r| {
            InstructionKind::Add { lhs: l, rhs: r }
        })
    }
    /// Wrapping integer/float subtraction.
    pub fn sub(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::ArithOrFloat, "sub", lhs, rhs, |l, r| {
            InstructionKind::Sub { lhs: l, rhs: r }
        })
    }
    /// Wrapping integer/float multiplication.
    pub fn mul(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::ArithOrFloat, "mul", lhs, rhs, |l, r| {
            InstructionKind::Mul { lhs: l, rhs: r }
        })
    }
    /// Integer (truncating) or float division.
    pub fn div(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::ArithOrFloatDiv, "div", lhs, rhs, |l, r| {
            InstructionKind::Div { lhs: l, rhs: r }
        })
    }
    /// Integer remainder.
    pub fn rem(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::ArithInt, "rem", lhs, rhs, |l, r| {
            InstructionKind::Rem { lhs: l, rhs: r }
        })
    }
    /// Bitwise AND.
    pub fn bit_and(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::AnyInt, "and", lhs, rhs, |l, r| {
            InstructionKind::And { lhs: l, rhs: r }
        })
    }
    /// Bitwise OR.
    pub fn bit_or(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::AnyInt, "or", lhs, rhs, |l, r| {
            InstructionKind::Or { lhs: l, rhs: r }
        })
    }
    /// Bitwise XOR.
    pub fn bit_xor(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::AnyInt, "xor", lhs, rhs, |l, r| {
            InstructionKind::Xor { lhs: l, rhs: r }
        })
    }
    /// Shift left.
    pub fn shl(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::ArithIntShift, "shl", lhs, rhs, |l, r| {
            InstructionKind::Shl { lhs: l, rhs: r }
        })
    }
    /// Shift right (arithmetic for signed operands, logical otherwise).
    pub fn shr(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::ArithIntShift, "shr", lhs, rhs, |l, r| {
            InstructionKind::Shr { lhs: l, rhs: r }
        })
    }
    /// Equality comparison; result is `i1`.
    pub fn eq(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::CmpEquality, "eq", lhs, rhs, |l, r| {
            InstructionKind::Eq { lhs: l, rhs: r }
        })
    }
    /// Inequality comparison; result is `i1`.
    pub fn ne(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::CmpEquality, "ne", lhs, rhs, |l, r| {
            InstructionKind::Ne { lhs: l, rhs: r }
        })
    }
    /// Signed/unsigned/float less-than; result is `i1`.
    pub fn lt(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::CmpOrdered, "lt", lhs, rhs, |l, r| {
            InstructionKind::Lt { lhs: l, rhs: r }
        })
    }
    /// Less-or-equal; result is `i1`.
    pub fn le(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::CmpOrdered, "le", lhs, rhs, |l, r| {
            InstructionKind::Le { lhs: l, rhs: r }
        })
    }
    /// Greater-than; result is `i1`.
    pub fn gt(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::CmpOrdered, "gt", lhs, rhs, |l, r| {
            InstructionKind::Gt { lhs: l, rhs: r }
        })
    }
    /// Greater-or-equal; result is `i1`.
    pub fn ge(&mut self, lhs: ValueId, rhs: ValueId) -> Result<ValueId, BuildError> {
        self.binop(BinClass::CmpOrdered, "ge", lhs, rhs, |l, r| {
            InstructionKind::Ge { lhs: l, rhs: r }
        })
    }

    // -- memory --------------------------------------------------------------

    /// Allocates a stack slot; yields `ptr<pointee>`.
    pub fn alloca(&mut self, pointee: TypeId) -> Result<ValueId, BuildError> {
        if !self
            .module
            .types
            .get(pointee)
            .map(|d| d.is_first_class())
            .unwrap_or(false)
        {
            return Err(BuildError::InvalidOperand {
                context: "alloca",
                reason: format!(
                    "`{}` is not a valid pointee type",
                    self.type_name(pointee)
                ),
            });
        }
        let ptr = self.module.types.ptr(pointee);
        self.append(InstructionKind::Alloca { pointee }, Some(ptr))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "alloca",
                reason: "expected an instruction result".into(),
            })
    }

    fn pointee_of(&self, pointer: ValueId) -> Result<TypeId, BuildError> {
        let ty = self.ty_of(pointer)?;
        match self.module.types.get(ty) {
            Some(TypeData::Pointer { pointee, .. }) => Ok(*pointee),
            _ => Err(BuildError::InvalidOperand {
                context: "load/store",
                reason: format!(
                    "operand `{pointer}` must be a pointer, found `{}`",
                    self.type_name(ty)
                ),
            }),
        }
    }

    /// Loads a value from `pointer`; the result type is the pointee type.
    pub fn load(&mut self, pointer: ValueId) -> Result<ValueId, BuildError> {
        let ty = self.pointee_of(pointer)?;
        if !self
            .module
            .types
            .get(ty)
            .map(|d| d.is_first_class())
            .unwrap_or(false)
        {
            return Err(BuildError::InvalidOperand {
                context: "load",
                reason: "cannot load a non-first-class pointee".into(),
            });
        }
        self.append(InstructionKind::Load { ty, pointer }, Some(ty))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "load",
                reason: "expected an instruction result".into(),
            })
    }

    /// Stores `value` into `pointer`.
    pub fn store(&mut self, pointer: ValueId, value: ValueId) -> Result<(), BuildError> {
        let pointee = self.pointee_of(pointer)?;
        let vty = self.ty_of(value)?;
        if pointee != vty {
            return Err(BuildError::TypeMismatch {
                context: "store".to_string(),
                expected: self.type_name(pointee),
                found: self.type_name(vty),
            });
        }
        self.append(
            InstructionKind::Store { pointer, value },
            None,
        )?;
        Ok(())
    }

    /// Element-wise pointer offset; `offset` is interpreted as a signed
    /// integer count of pointee-sized elements.
    pub fn ptr_offset(
        &mut self,
        pointer: ValueId,
        offset: ValueId,
    ) -> Result<ValueId, BuildError> {
        let pty = self.ty_of(pointer)?;
        if !matches!(
            self.module.types.get(pty),
            Some(TypeData::Pointer { .. })
        ) {
            return Err(BuildError::InvalidOperand {
                context: "ptr_offset",
                reason: format!(
                    "base must be a pointer, found `{}`",
                    self.type_name(pty)
                ),
            });
        }
        let oty = self.ty_of(offset)?;
        if !self
            .module
            .types
            .get(oty)
            .map(|d| d.is_arith_integer())
            .unwrap_or(false)
        {
            return Err(BuildError::InvalidOperand {
                context: "ptr_offset",
                reason: format!(
                    "offset must be an integer, found `{}`",
                    self.type_name(oty)
                ),
            });
        }
        self.append(
            InstructionKind::PtrOffset { pointer, offset },
            Some(pty),
        )?
        .ok_or_else(|| BuildError::InvalidOperand {
            context: "ptr_offset",
            reason: "expected an instruction result".into(),
        })
    }

    // -- conversions -----------------------------------------------------------

    fn conv<F>(
        &mut self,
        opname: &'static str,
        to: TypeId,
        value: ValueId,
        mk: F,
        to_check: fn(&TypeData) -> bool,
        from_check: fn(&TypeData) -> bool,
        widening: Option<bool>,
    ) -> Result<ValueId, BuildError>
    where
        F: FnOnce(TypeId, ValueId) -> InstructionKind,
    {
        let from = self.ty_of(value)?;
        let to_data = self.module.types.get(to).cloned();
        let from_data = self.module.types.get(from).cloned();
        let (to_ok, from_ok) = match (to_data, from_data) {
            (Some(t), Some(fr)) => (to_check(&t), from_check(&fr)),
            _ => (false, false),
        };
        if !to_ok || !from_ok {
            return Err(BuildError::InvalidOperand {
                context: opname,
                reason: format!(
                    "cannot convert `{}` to `{}` with {opname}",
                    self.type_name(from),
                    self.type_name(to)
                ),
            });
        }
        if let Some(must_widen) = widening {
            let to_bits = self.module.types.get(to).and_then(|d| d.int_bits()).unwrap();
            let from_bits = self.module.types.get(from).and_then(|d| d.int_bits()).unwrap();
            if must_widen && to_bits <= from_bits {
                return Err(BuildError::InvalidOperand {
                    context: opname,
                    reason: format!(
                        "{opname} must widen: `{}` -> `{}`",
                        self.type_name(from),
                        self.type_name(to)
                    ),
                });
            }
            if !must_widen && to_bits >= from_bits {
                return Err(BuildError::InvalidOperand {
                    context: opname,
                    reason: format!(
                        "{opname} must narrow: `{}` -> `{}`",
                        self.type_name(from),
                        self.type_name(to)
                    ),
                });
            }
        }
        self.append(mk(to, value), Some(to))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: opname,
                reason: "expected an instruction result".into(),
            })
    }

    /// Integer extend (sign/zero-extend by source signedness).
    pub fn ext(&mut self, to: TypeId, value: ValueId) -> Result<ValueId, BuildError> {
        self.conv(
            "ext",
            to,
            value,
            |to, v| InstructionKind::Ext { to, value: v },
            |d| d.is_arith_integer(),
            |d| d.is_integer(),
            Some(true),
        )
    }

    /// Integer truncate.
    pub fn trunc(&mut self, to: TypeId, value: ValueId) -> Result<ValueId, BuildError> {
        self.conv(
            "trunc",
            to,
            value,
            |to, v| InstructionKind::Trunc { to, value: v },
            |d| d.is_integer(),
            |d| d.is_integer(),
            Some(false),
        )
    }

    /// Integer to float.
    pub fn int_to_float(&mut self, to: TypeId, value: ValueId) -> Result<ValueId, BuildError> {
        self.conv(
            "int_to_float",
            to,
            value,
            |to, v| InstructionKind::IntToFloat { to, value: v },
            |d| d.is_float(),
            |d| d.is_arith_integer(),
            None,
        )
    }

    /// Float to integer (truncating, saturating).
    pub fn float_to_int(&mut self, to: TypeId, value: ValueId) -> Result<ValueId, BuildError> {
        self.conv(
            "float_to_int",
            to,
            value,
            |to, v| InstructionKind::FloatToInt { to, value: v },
            |d| d.is_arith_integer(),
            |d| d.is_float(),
            None,
        )
    }

    /// Pointer to pointer cast.
    pub fn ptr_cast(&mut self, to: TypeId, pointer: ValueId) -> Result<ValueId, BuildError> {
        let from = self.ty_of(pointer)?;
        let both_ptr = matches!(self.module.types.get(to), Some(TypeData::Pointer { .. }))
            && matches!(self.module.types.get(from), Some(TypeData::Pointer { .. }));
        if !both_ptr {
            return Err(BuildError::InvalidOperand {
                context: "ptr_cast",
                reason: format!(
                    "ptr_cast requires pointer types, found `{}` -> `{}`",
                    self.type_name(from),
                    self.type_name(to)
                ),
            });
        }
        self.append(InstructionKind::PtrCast { to, pointer }, Some(to))?
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "ptr_cast",
                reason: "expected an instruction result".into(),
        })
    }

    // -- calls / aggregates ------------------------------------------------------

    /// Direct call. Returns `Some(value)` unless the callee returns `void`.
    pub fn call(
        &mut self,
        callee: FunctionId,
        args: &[ValueId],
    ) -> Result<Option<ValueId>, BuildError> {
        let (fixed, variadic, result, callee_name) = {
            let f = self
                .module
                .function(callee)
                .ok_or(BuildError::UnknownFunction { function: callee })?;
            let name = self
                .module
                .symbol_name(f.symbol)
                .unwrap_or("<invalid>")
                .to_string();
            (
                f.params.iter().map(|p| p.ty).collect::<Vec<_>>(),
                f.variadic,
                f.result,
                name,
            )
        };
        if variadic {
            if args.len() < fixed.len() {
                return Err(BuildError::InvalidOperand {
                    context: "call",
                    reason: format!(
                        "`{callee_name}` expects at least {} argument(s), found {}",
                        fixed.len(),
                        args.len()
                    ),
                });
            }
        } else if args.len() != fixed.len() {
            return Err(BuildError::InvalidOperand {
                context: "call",
                reason: format!(
                    "`{callee_name}` expects {} argument(s), found {}",
                    fixed.len(),
                    args.len()
                ),
            });
        }
        for (i, (&arg, &expected)) in args.iter().zip(fixed.iter()).enumerate() {
            let actual = self.ty_of(arg)?;
            if actual != expected {
                return Err(BuildError::TypeMismatch {
                    context: format!("call to `{callee_name}` argument {i}"),
                    expected: self.type_name(expected),
                    found: self.type_name(actual),
                });
            }
        }
        for (i, &arg) in args.iter().skip(fixed.len()).enumerate() {
            let actual = self.ty_of(arg)?;
            if !self
                .module
                .types
                .get(actual)
                .map(|d| d.is_first_class())
                .unwrap_or(false)
            {
                return Err(BuildError::InvalidOperand {
                    context: "call",
                    reason: format!(
                        "`{callee_name}` variadic argument {} must be first-class, found `{}`",
                        fixed.len() + i,
                        self.type_name(actual)
                    ),
                });
            }
        }
        let result_ty = if self
            .module
            .types
            .get(result)
            .map(|d| d.is_void())
            .unwrap_or(true)
        {
            None
        } else {
            Some(result)
        };
        self.append(
            InstructionKind::Call {
                callee,
                args: args.to_vec(),
            },
            result_ty,
        )
    }

    /// Constructs an aggregate value of type `ty`.
    pub fn construct(&mut self, ty: TypeId, fields: &[ValueId]) -> Result<ValueId, BuildError> {
        let field_tys = self.aggregate_fields(ty)?;
        if field_tys.len() != fields.len() {
            return Err(BuildError::InvalidOperand {
                context: "construct",
                reason: format!(
                    "`{}` needs {} field(s), found {}",
                    self.type_name(ty),
                    field_tys.len(),
                    fields.len()
                ),
            });
        }
        for (i, (&f, &expected)) in fields.iter().zip(field_tys.iter()).enumerate() {
            let actual = self.ty_of(f)?;
            if actual != expected {
                return Err(BuildError::TypeMismatch {
                    context: format!("construct field {i}"),
                    expected: self.type_name(expected),
                    found: self.type_name(actual),
                });
            }
        }
        self.append(
            InstructionKind::Construct {
                ty,
                fields: fields.to_vec(),
            },
            Some(ty),
        )?
        .ok_or_else(|| BuildError::InvalidOperand {
            context: "construct",
            reason: "expected an instruction result".into(),
        })
    }

    fn aggregate_fields(&self, ty: TypeId) -> Result<Vec<TypeId>, BuildError> {
        match self.module.types.get(ty) {
            Some(TypeData::Array { element, length }) => Ok(vec![*element; *length as usize]),
            Some(TypeData::Struct { fields }) => Ok(fields.clone()),
            _ => Err(BuildError::InvalidOperand {
                context: "aggregate operation",
                reason: format!("`{}` is not an aggregate type", self.type_name(ty)),
            }),
        }
    }

    /// Reads field `index` of an aggregate.
    pub fn extract(&mut self, aggregate: ValueId, index: u32) -> Result<ValueId, BuildError> {
        let aty = self.ty_of(aggregate)?;
        let fields = self.aggregate_fields(aty)?;
        let field = *fields
            .get(index as usize)
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "extract",
                reason: format!(
                    "index {index} out of range for `{}`",
                    self.type_name(aty)
                ),
            })?;
        self.append(
            InstructionKind::Extract { aggregate, index },
            Some(field),
        )?
        .ok_or_else(|| BuildError::InvalidOperand {
            context: "extract",
            reason: "expected an instruction result".into(),
        })
    }

    /// Replaces field `index` of an aggregate, producing a new value.
    pub fn insert(
        &mut self,
        aggregate: ValueId,
        index: u32,
        value: ValueId,
    ) -> Result<ValueId, BuildError> {
        let aty = self.ty_of(aggregate)?;
        let fields = self.aggregate_fields(aty)?;
        let field = *fields
            .get(index as usize)
            .ok_or_else(|| BuildError::InvalidOperand {
                context: "insert",
                reason: format!(
                    "index {index} out of range for `{}`",
                    self.type_name(aty)
                ),
            })?;
        let vty = self.ty_of(value)?;
        if vty != field {
            return Err(BuildError::TypeMismatch {
                context: format!("insert field {index}"),
                expected: self.type_name(field),
                found: self.type_name(vty),
            });
        }
        self.append(
            InstructionKind::Insert {
                aggregate,
                index,
                value,
            },
            Some(aty),
        )?
        .ok_or_else(|| BuildError::InvalidOperand {
            context: "insert",
            reason: "expected an instruction result".into(),
        })
    }

    // -- terminators ---------------------------------------------------------

    fn set_terminator(&mut self, term: Terminator) -> Result<(), BuildError> {
        let block = self.current()?;
        if self.f().blocks[block.index()].terminator.is_some() {
            return Err(BuildError::BlockTerminated { block });
        }
        self.f_mut().blocks[block.index()].terminator = Some(term);
        // The cursor stays on the terminated block so that accidental
        // appends report `BlockTerminated` rather than a confusing
        // "no current block"; `append_block`/`switch_to` move on.
        Ok(())
    }

    fn check_branch_args(
        &self,
        from: BlockId,
        target: BlockId,
        args: &[ValueId],
    ) -> Result<(), BuildError> {
        let target_block = self.block(target)?;
        if target_block.params.len() != args.len() {
            return Err(BuildError::InvalidOperand {
                context: "jump/branch",
                reason: format!(
                    "block `{}` expects {} argument(s), found {}",
                    self.block_label(target),
                    target_block.params.len(),
                    args.len()
                ),
            });
        }
        for (i, (&arg, param)) in args.iter().zip(target_block.params.iter()).enumerate() {
            let actual = self.ty_of(arg)?;
            if actual != param.ty {
                return Err(BuildError::TypeMismatch {
                    context: format!(
                        "argument {i} for block `{}`",
                        self.block_label(target)
                    ),
                    expected: self.type_name(param.ty),
                    found: self.type_name(actual),
                });
            }
        }
        let _ = from;
        Ok(())
    }

    fn block_label(&self, block: BlockId) -> String {
        match self.block(block) {
            Ok(b) => match b.name {
                Some(sym) => self
                    .module
                    .symbol_name(sym)
                    .unwrap_or("bb?")
                    .to_string(),
                None => format!("bb{block}"),
            },
            Err(_) => format!("bb{block}"),
        }
    }

    /// Unconditional jump with block arguments.
    pub fn jump(&mut self, target: BlockId, args: &[ValueId]) -> Result<(), BuildError> {
        let from = self.current()?;
        self.check_branch_args(from, target, args)?;
        self.set_terminator(Terminator::Jump {
            target,
            args: args.to_vec(),
        })
    }

    /// Conditional branch; the condition must be `i1`.
    pub fn branch(
        &mut self,
        condition: ValueId,
        then_block: BlockId,
        then_args: &[ValueId],
        else_block: BlockId,
        else_args: &[ValueId],
    ) -> Result<(), BuildError> {
        let from = self.current()?;
        let cty = self.ty_of(condition)?;
        if cty != TypeId::I1 {
            return Err(BuildError::TypeMismatch {
                context: "branch condition".to_string(),
                expected: "i1".to_string(),
                found: self.type_name(cty),
            });
        }
        self.check_branch_args(from, then_block, then_args)?;
        self.check_branch_args(from, else_block, else_args)?;
        self.set_terminator(Terminator::Branch {
            condition,
            then_block,
            then_args: then_args.to_vec(),
            else_block,
            else_args: else_args.to_vec(),
        })
    }

    /// Return a value (or `None` for `void` functions).
    pub fn ret(&mut self, value: Option<ValueId>) -> Result<(), BuildError> {
        let result = self.f().result;
        if let Some(v) = value {
            let ty = self.ty_of(v)?;
            if ty != result {
                return Err(BuildError::TypeMismatch {
                    context: "return".to_string(),
                    expected: self.type_name(result),
                    found: self.type_name(ty),
                });
            }
        } else if !self
            .module
            .types
            .get(result)
            .map(|d| d.is_void())
            .unwrap_or(false)
        {
            return Err(BuildError::TypeMismatch {
                context: "return".to_string(),
                expected: self.type_name(result),
                found: "void".to_string(),
            });
        }
        self.set_terminator(Terminator::Return { value })
    }

    /// Return from a `void` function.
    pub fn ret_void(&mut self) -> Result<(), BuildError> {
        self.ret(None)
    }

    /// Marks the current block as unreachable.
    pub fn unreachable(&mut self) -> Result<(), BuildError> {
        self.set_terminator(Terminator::Unreachable)
    }
}
