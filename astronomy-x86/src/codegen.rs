//! x86-64 (System V) instruction selection for verified Astronomy modules.
//!
//! # Strategy
//!
//! This is a *correctness-first* backend: every SSA value (function
//! parameter, block parameter and instruction result) is assigned a stack
//! slot, and instructions load their operands from those slots into
//! registers, compute, and store the result back. There is no register
//! allocator — that keeps lowering local and easy to verify.
//!
//! * **CFG** — blocks become labels (`fname.bbN`), terminators become
//!   `jmp`/`je`/`leave; ret`/`ud2`.
//! * **Block arguments** — a jump/branch first stages every argument into a
//!   per-block scratch area, then copies scratch slots into the target
//!   block's parameter slots. This is a parallel copy, so the staging pass
//!   is what makes `jump bb(p1, p0)` correct.
//! * **Frame** — all slots are `rbp`-relative; `rsp` is kept fixed at a
//!   16-byte boundary with an outgoing stack-argument area reserved at the
//!   bottom, so calls always see a 16-byte-aligned stack.
//! * **ABI** — the System V AMD64 convention is used for every function
//!   regardless of the IR `Abi` fact (`c`, `astronomy`, `system`, `custom`
//!   all map to it). Integer/pointer arguments go in
//!   `rdi, rsi, rdx, rcx, r8, r9`, float arguments in `xmm0..xmm7`, the
//!   rest on the stack; `%al` is set for variadic calls.
//!
//! The output is a structured [`Program`] of [`Inst`]s, which
//! `astronomy-nasm` prints as NASM text and `astronomy-object` encodes into
//! an ELF object. Unsupported constructs (128-bit integers, aggregates
//! passed/returned by value) produce structured [`BackendError`]s instead
//! of wrong code.

use std::collections::HashMap;

use astronomy::{
    ConstantData, Function, FunctionId, Instruction, InstructionKind, Linkage, Module, Terminator,
    TypeData, TypeId, ValueId, VerifiedModule,
};

use crate::asm::{
    AluOp, Cond, DataId, Fp, FuncCode, FuncSymbol, ImmStyle, Inst, Label, LabelKind, Mem, Program,
    Reg, ShiftOp, SseOp, SymbolKind, Width, Xmm,
};
use crate::error::BackendError;
use crate::layout::{self, align_of, align_up, is_aggregate, is_f32, is_float, size_of};

/// Lowers a verified module to machine instructions.
pub fn lower(module: &VerifiedModule) -> Result<Program, BackendError> {
    let module = module.module();
    let mut rodata = Rodata::default();
    let symbols = module
        .functions()
        .iter()
        .map(|f| FuncSymbol {
            name: module
                .symbol_name(f.symbol)
                .unwrap_or("<invalid>")
                .to_string(),
            kind: if f.is_declaration() {
                SymbolKind::Extern
            } else if matches!(f.linkage, Linkage::Internal) {
                SymbolKind::Local
            } else {
                SymbolKind::Global
            },
        })
        .collect();

    let mut functions = Vec::new();
    for (index, function) in module.functions().iter().enumerate() {
        if function.is_declaration() {
            continue;
        }
        let id = FunctionId::new(index as u32);
        let mut func = FuncGen::new(module, function, &mut rodata);
        func.build()?;
        functions.push(FuncCode {
            id,
            labels: func.labels,
            insts: func.insts,
        });
    }

    Ok(Program {
        symbols,
        functions,
        rodata: rodata.entries,
    })
}

// ---------------------------------------------------------------------------
// Read-only data (strings and float-conversion bounds)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Rodata {
    entries: Vec<Vec<u8>>,
    cache: HashMap<Vec<u8>, DataId>,
}

impl Rodata {
    fn intern(&mut self, bytes: &[u8]) -> DataId {
        if let Some(&id) = self.cache.get(bytes) {
            return id;
        }
        let id = DataId(self.entries.len() as u32);
        self.entries.push(bytes.to_vec());
        self.cache.insert(bytes.to_vec(), id);
        id
    }

    fn intern_f64(&mut self, value: f64) -> DataId {
        self.intern(&value.to_bits().to_le_bytes())
    }
}

// ---------------------------------------------------------------------------
// Per-function code generation
// ---------------------------------------------------------------------------

/// System V integer argument registers, in order.
const GP_ARGS: [Reg; 6] = [Reg::Rdi, Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];

const XMM0: Xmm = Xmm(0);
const XMM1: Xmm = Xmm(1);

/// Where a System V argument is passed.
#[derive(Debug, Clone, Copy)]
enum Loc {
    /// Integer/pointer register (System V order).
    Int(Reg),
    /// SSE register index (`xmmN`).
    Sse(u8),
    /// Stack slot at `[rsp + offset]`.
    Stack(u64),
}

/// Binary arithmetic operator for `add`/`sub`/`mul` over ints and floats.
#[derive(Debug, Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
}

/// Comparison operator (result `i1`).
#[derive(Debug, Clone, Copy)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Width of an integer access covering `bits` bits.
fn width_of_bits(bits: u32) -> Width {
    Width::from_bytes((bits as u64).div_ceil(8).max(1))
}

struct FuncGen<'a> {
    module: &'a Module,
    f: &'a Function,
    fname: String,
    rodata: &'a mut Rodata,
    insts: Vec<Inst>,
    labels: Vec<(String, LabelKind)>,
    /// `disp[value.index()]` is the `rbp` displacement of the value's slot.
    disp: Vec<i32>,
    /// Displacement of each `alloca`'s storage, keyed by result value.
    alloca: HashMap<u32, i32>,
    /// Per-block scratch slots used to implement parallel edge copies.
    scratch: Vec<Vec<i32>>,
    frame_size: u64,
    tmp: usize,
}

impl<'a> FuncGen<'a> {
    fn new(module: &'a Module, f: &'a Function, rodata: &'a mut Rodata) -> Self {
        let fname = module
            .symbol_name(f.symbol)
            .unwrap_or("<invalid>")
            .to_string();
        // Labels 0..n are the blocks, so `Label(i)` is block `i`.
        let labels = (0..f.blocks.len())
            .map(|i| (format!("{fname}.bb{i}"), LabelKind::Block))
            .collect();
        FuncGen {
            module,
            f,
            fname,
            rodata,
            insts: Vec::new(),
            labels,
            disp: Vec::new(),
            alloca: HashMap::new(),
            scratch: Vec::new(),
            frame_size: 0,
            tmp: 0,
        }
    }

    // -- small helpers ------------------------------------------------------

    fn emit(&mut self, inst: Inst) {
        self.insts.push(inst);
    }

    fn slot(d: i32) -> Mem {
        Mem::at(Reg::Rbp, d)
    }

    fn tmp_label(&mut self, tag: &str) -> Label {
        self.tmp += 1;
        let label = Label(self.labels.len() as u32);
        self.labels.push((
            format!("{}.{}{}", self.fname, tag, self.tmp),
            LabelKind::Local,
        ));
        label
    }

    fn block_label(block: usize) -> Label {
        Label(block as u32)
    }

    fn ty_of(&self, v: ValueId) -> TypeId {
        self.f.value(v).expect("verified value").ty
    }

    fn int_info(&self, ty: TypeId) -> (u32, bool) {
        match self.module.types().get(ty) {
            Some(TypeData::Int { bits, signed }) => (*bits, *signed && *bits > 1),
            Some(TypeData::Pointer { .. }) => (64, false),
            _ => (64, false),
        }
    }

    fn int_bits(&self, ty: TypeId) -> u32 {
        match self.module.types().get(ty) {
            Some(TypeData::Int { bits, .. }) => *bits,
            Some(TypeData::Pointer { .. }) => 64,
            _ => 64,
        }
    }

    fn size(&self, ty: TypeId) -> Result<u64, BackendError> {
        size_of(self.module.types(), ty)
    }

    // -- moving values ------------------------------------------------------

    /// Loads an integer of `bits` from `mem` into the 64-bit `reg`,
    /// sign- or zero-extending.
    fn load_mem(&mut self, reg: Reg, mem: Mem, bits: u32, signed: bool) {
        let inst = match bits {
            64 => Inst::Load { dst: reg.q(), mem },
            32 if !signed => Inst::Load {
                dst: reg.at(Width::W32),
                mem,
            },
            32 => Inst::LoadExt {
                dst: reg,
                src: Width::W32,
                signed: true,
                mem,
            },
            16 => Inst::LoadExt {
                dst: reg,
                src: Width::W16,
                signed,
                mem,
            },
            _ => Inst::LoadExt {
                dst: reg,
                src: Width::W8,
                signed: signed && bits == 8,
                mem,
            },
        };
        self.emit(inst);
    }

    fn load_int(&mut self, reg: Reg, d: i32, bits: u32, signed: bool) {
        self.load_mem(reg, Self::slot(d), bits, signed);
    }

    fn store_mem(&mut self, reg: Reg, mem: Mem, bits: u32) {
        self.emit(Inst::Store {
            mem,
            src: reg.at(width_of_bits(bits)),
        });
    }

    fn store_int(&mut self, reg: Reg, d: i32, bits: u32) {
        self.store_mem(reg, Self::slot(d), bits);
    }

    fn load_float(&mut self, xmm: Xmm, d: i32, f32: bool) {
        self.emit(Inst::FLoad {
            fp: Fp::of(f32),
            dst: xmm,
            mem: Self::slot(d),
        });
    }

    fn store_float(&mut self, xmm: Xmm, d: i32, f32: bool) {
        self.emit(Inst::FStore {
            fp: Fp::of(f32),
            mem: Self::slot(d),
            src: xmm,
        });
    }

    /// `rcx = count; rep movsb` with `rsi`/`rdi` already set up.
    fn emit_byte_copy(&mut self, count: u64) {
        self.emit(Inst::MovRI {
            dst: Reg::Rcx.q(),
            imm: count,
            style: ImmStyle::Dec,
        });
        self.emit(Inst::Cld);
        self.emit(Inst::RepMovsb);
    }

    /// Copies a whole value between two frame slots (bit-for-bit).
    fn copy_slot(&mut self, src: i32, ty: TypeId, dst: i32) -> Result<(), BackendError> {
        let agg = is_aggregate(self.module.types(), ty);
        let size = self.size(ty)?;
        if agg {
            self.emit(Inst::Lea {
                dst: Reg::Rsi,
                mem: Self::slot(src),
            });
            self.emit(Inst::Lea {
                dst: Reg::Rdi,
                mem: Self::slot(dst),
            });
            self.emit_byte_copy(size);
        } else {
            let bits = (size * 8) as u32;
            self.load_int(Reg::Rax, src, bits, false);
            self.store_int(Reg::Rax, dst, bits);
        }
        Ok(())
    }

    // -- frame layout -------------------------------------------------------

    fn reserve_slot(&self, used: &mut u64, ty: TypeId) -> Result<i32, BackendError> {
        let size = self.size(ty)?;
        let align = align_of(self.module.types(), ty)?;
        let end = used.checked_add(size).and_then(|end| align_up(end, align));
        // Frame references and SUB rsp immediates are signed 32-bit values.
        // Leave room for rounding the frame up to the 16-byte ABI alignment.
        match end.filter(|end| *end <= (i32::MAX as u64 & !15)) {
            Some(end) => {
                *used = end;
                Ok(-(end as i32))
            }
            None => Err(BackendError::UnsupportedType {
                context: format!("stack frame of `{}`", self.fname),
                ty: astronomy::types::type_to_string(self.module.types(), ty),
                reason: "stack frame exceeds the x86-64 signed 32-bit displacement limit".into(),
            }),
        }
    }

    fn build(&mut self) -> Result<(), BackendError> {
        let store = self.module.types();
        for (i, value) in self.f.values.iter().enumerate() {
            layout::check_supported(store, value.ty, &format!("value v{i} of `{}`", self.fname))?;
        }

        let mut used: u64 = 0;
        self.disp = vec![0; self.f.values.len()];
        for (i, value) in self.f.values.iter().enumerate() {
            self.disp[i] = self.reserve_slot(&mut used, value.ty)?;
        }

        // `alloca` storage.
        for block in &self.f.blocks {
            for inst in &block.instructions {
                if let InstructionKind::Alloca { pointee } = inst.kind {
                    layout::check_supported(
                        store,
                        pointee,
                        &format!("alloca in `{}`", self.fname),
                    )?;
                    let offset = self.reserve_slot(&mut used, pointee)?;
                    if let Some(result) = inst.result {
                        self.alloca.insert(result.as_u32(), offset);
                    }
                }
            }
        }

        // Per-block scratch for block-argument staging.
        self.scratch = Vec::with_capacity(self.f.blocks.len());
        for block in &self.f.blocks {
            let mut slots = Vec::with_capacity(block.params.len());
            for param in &block.params {
                slots.push(self.reserve_slot(&mut used, param.ty)?);
            }
            self.scratch.push(slots);
        }

        // Outgoing stack-argument area: size of the largest call.
        let mut max_stack = 0u64;
        for block in &self.f.blocks {
            for inst in &block.instructions {
                if let InstructionKind::Call { args, .. } = &inst.kind {
                    let types: Vec<TypeId> = args
                        .iter()
                        .map(|a| self.f.value(*a).expect("value").ty)
                        .collect();
                    let (_locs, bytes, _sse) = self.plan_types(&types, "call")?;
                    max_stack = max_stack.max(bytes);
                }
            }
        }

        self.frame_size = align_up(used, 16)
            .and_then(|locals| {
                align_up(max_stack, 16).and_then(|outgoing| locals.checked_add(outgoing))
            })
            .filter(|size| *size <= i32::MAX as u64)
            .ok_or_else(|| BackendError::UnsupportedInstruction {
                function: self.fname.clone(),
                op: "call",
                reason: "outgoing arguments exceed the x86-64 stack frame limit".into(),
            })?;

        self.emit_prologue()?;
        for index in 0..self.f.blocks.len() {
            self.emit_block(index)?;
        }
        Ok(())
    }

    // -- ABI planning -------------------------------------------------------

    /// Assigns System V locations to a list of argument/parameter types.
    /// Returns the locations, the number of stack bytes consumed and the
    /// number of SSE registers used.
    fn plan_types(
        &self,
        types: &[TypeId],
        op: &'static str,
    ) -> Result<(Vec<Loc>, u64, usize), BackendError> {
        let mut gp = 0usize;
        let mut sse = 0usize;
        let mut stack = 0u64;
        let mut locs = Vec::with_capacity(types.len());
        for &ty in types {
            match self.module.types().get(ty) {
                Some(data) if data.is_float() => {
                    if sse < 8 {
                        locs.push(Loc::Sse(sse as u8));
                        sse += 1;
                    } else {
                        locs.push(Loc::Stack(stack));
                        stack += 8;
                    }
                }
                Some(data) if data.is_pointer() || data.is_integer() => {
                    layout::check_supported(
                        self.module.types(),
                        ty,
                        &format!("`{op}` in `{}`", self.fname),
                    )?;
                    if gp < 6 {
                        locs.push(Loc::Int(GP_ARGS[gp]));
                        gp += 1;
                    } else {
                        locs.push(Loc::Stack(stack));
                        stack += 8;
                    }
                }
                _ => {
                    return Err(BackendError::UnsupportedInstruction {
                        function: self.fname.clone(),
                        op,
                        reason: format!(
                            "aggregate by-value {} is not supported (`{}`)",
                            if op == "parameter" {
                                "parameter"
                            } else {
                                "argument"
                            },
                            astronomy::types::type_to_string(self.module.types(), ty)
                        ),
                    });
                }
            }
        }
        Ok((locs, stack, sse))
    }

    /// Converts a stack-argument offset into a displacement.
    fn stack_disp(&self, base: u64, off: u64, op: &'static str) -> Result<i32, BackendError> {
        base.checked_add(off)
            .and_then(|d| i32::try_from(d).ok())
            .ok_or_else(|| BackendError::UnsupportedInstruction {
                function: self.fname.clone(),
                op,
                reason: "stack arguments exceed the x86-64 signed 32-bit displacement limit".into(),
            })
    }

    // -- prologue / blocks --------------------------------------------------

    fn emit_prologue(&mut self) -> Result<(), BackendError> {
        self.emit(Inst::Push(Reg::Rbp));
        self.emit(Inst::MovRR {
            dst: Reg::Rbp.q(),
            src: Reg::Rsp.q(),
        });
        if self.frame_size > 0 {
            self.emit(Inst::AluRI {
                op: AluOp::Sub,
                dst: Reg::Rsp.q(),
                imm: self.frame_size as i32,
            });
        }

        let ptypes: Vec<TypeId> = self.f.params.iter().map(|p| p.ty).collect();
        let (locs, _bytes, _sse) = self.plan_types(&ptypes, "parameter")?;
        for (i, loc) in locs.iter().enumerate() {
            let value = self.f.params[i].value;
            let d = self.disp[value.index()];
            let ty = ptypes[i];
            match *loc {
                Loc::Int(reg) => {
                    let (bits, _) = self.int_info(ty);
                    self.store_int(reg, d, bits);
                }
                Loc::Sse(n) => {
                    let f32 = is_f32(self.module.types(), ty);
                    self.store_float(Xmm(n), d, f32);
                }
                Loc::Stack(off) => {
                    let bytes = self.size(ty)?;
                    let at = self.stack_disp(16, off, "parameter")?;
                    self.emit(Inst::Load {
                        dst: Reg::Rax.q(),
                        mem: Self::slot(at),
                    });
                    self.store_int(Reg::Rax, d, (bytes * 8) as u32);
                }
            }
        }
        Ok(())
    }

    fn emit_block(&mut self, index: usize) -> Result<(), BackendError> {
        let block = &self.f.blocks[index];
        self.emit(Inst::Label(Self::block_label(index)));
        for inst in &block.instructions {
            self.gen_inst(inst)?;
        }
        match block.terminator.as_ref() {
            Some(term) => self.gen_term(term),
            None => Err(BackendError::InvalidModule {
                reason: format!("block bb{index} of `{}` has no terminator", self.fname),
            }),
        }
    }

    fn emit_edge(&mut self, target: usize, args: &[ValueId]) -> Result<(), BackendError> {
        if args.is_empty() {
            return Ok(());
        }
        let f = self.f;
        let params = &f.blocks[target].params;
        // Parallel copy: stage every argument first, then commit.
        for (i, arg) in args.iter().enumerate() {
            let src = self.disp[arg.index()];
            self.copy_slot(src, params[i].ty, self.scratch[target][i])?;
        }
        for (i, param) in params.iter().enumerate().take(args.len()) {
            let dst = self.disp[param.value.index()];
            self.copy_slot(self.scratch[target][i], param.ty, dst)?;
        }
        Ok(())
    }

    fn gen_term(&mut self, term: &Terminator) -> Result<(), BackendError> {
        match term {
            Terminator::Jump { target, args } => {
                self.emit_edge(target.index(), args)?;
                self.emit(Inst::Jmp(Self::block_label(target.index())));
            }
            Terminator::Branch {
                condition,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let cdisp = self.disp[condition.index()];
                let else_label = self.tmp_label("else");
                self.emit(Inst::AluMI {
                    op: AluOp::Cmp,
                    width: Width::W8,
                    mem: Self::slot(cdisp),
                    imm: 0,
                });
                self.emit(Inst::Jcc(Cond::E, else_label));
                self.emit_edge(then_block.index(), then_args)?;
                self.emit(Inst::Jmp(Self::block_label(then_block.index())));
                self.emit(Inst::Label(else_label));
                self.emit_edge(else_block.index(), else_args)?;
                self.emit(Inst::Jmp(Self::block_label(else_block.index())));
            }
            Terminator::Return { value } => {
                if let Some(v) = value {
                    let ty = self.ty_of(*v);
                    let d = self.disp[v.index()];
                    if is_aggregate(self.module.types(), ty) {
                        return Err(BackendError::UnsupportedInstruction {
                            function: self.fname.clone(),
                            op: "return",
                            reason: "aggregate return values are not supported".to_string(),
                        });
                    }
                    if is_float(self.module.types(), ty) {
                        let f32 = is_f32(self.module.types(), ty);
                        self.load_float(XMM0, d, f32);
                    } else {
                        let (bits, signed) = self.int_info(ty);
                        self.load_int(Reg::Rax, d, bits, signed);
                    }
                }
                self.emit(Inst::Leave);
                self.emit(Inst::Ret);
            }
            Terminator::Unreachable => self.emit(Inst::Ud2),
        }
        Ok(())
    }

    // -- instructions -------------------------------------------------------

    fn result_disp(&self, inst: &Instruction) -> Result<i32, BackendError> {
        inst.result
            .map(|v| self.disp[v.index()])
            .ok_or_else(|| BackendError::InvalidModule {
                reason: format!("instruction in `{}` is missing its result", self.fname),
            })
    }

    fn gen_inst(&mut self, inst: &Instruction) -> Result<(), BackendError> {
        use InstructionKind as K;
        match &inst.kind {
            K::Const(cid) => {
                let data = self.module.constants().get(*cid).cloned().ok_or_else(|| {
                    BackendError::InvalidModule {
                        reason: format!("unknown constant c{}", cid.as_u32()),
                    }
                })?;
                let dst = self.result_disp(inst)?;
                self.materialize_const(&data, dst)?;
            }
            K::Add { lhs, rhs } => self.gen_arith(ArithOp::Add, *lhs, *rhs, inst)?,
            K::Sub { lhs, rhs } => self.gen_arith(ArithOp::Sub, *lhs, *rhs, inst)?,
            K::Mul { lhs, rhs } => self.gen_arith(ArithOp::Mul, *lhs, *rhs, inst)?,
            K::Div { lhs, rhs } => self.gen_div(*lhs, *rhs, inst, false)?,
            K::Rem { lhs, rhs } => self.gen_div(*lhs, *rhs, inst, true)?,
            K::And { lhs, rhs } => self.gen_bitwise(AluOp::And, *lhs, *rhs, inst)?,
            K::Or { lhs, rhs } => self.gen_bitwise(AluOp::Or, *lhs, *rhs, inst)?,
            K::Xor { lhs, rhs } => self.gen_bitwise(AluOp::Xor, *lhs, *rhs, inst)?,
            K::Shl { lhs, rhs } => self.gen_shift(*lhs, *rhs, inst, true)?,
            K::Shr { lhs, rhs } => self.gen_shift(*lhs, *rhs, inst, false)?,
            K::Eq { lhs, rhs } => self.gen_cmp(CmpOp::Eq, *lhs, *rhs, inst)?,
            K::Ne { lhs, rhs } => self.gen_cmp(CmpOp::Ne, *lhs, *rhs, inst)?,
            K::Lt { lhs, rhs } => self.gen_cmp(CmpOp::Lt, *lhs, *rhs, inst)?,
            K::Le { lhs, rhs } => self.gen_cmp(CmpOp::Le, *lhs, *rhs, inst)?,
            K::Gt { lhs, rhs } => self.gen_cmp(CmpOp::Gt, *lhs, *rhs, inst)?,
            K::Ge { lhs, rhs } => self.gen_cmp(CmpOp::Ge, *lhs, *rhs, inst)?,
            K::Alloca { .. } => {
                let result = inst.result.expect("alloca result");
                let offset = self.alloca.get(&result.as_u32()).copied().ok_or_else(|| {
                    BackendError::InvalidModule {
                        reason: format!("alloca in `{}` has no storage", self.fname),
                    }
                })?;
                self.emit(Inst::Lea {
                    dst: Reg::Rax,
                    mem: Self::slot(offset),
                });
                let dst = self.disp[result.index()];
                self.store_int(Reg::Rax, dst, 64);
            }
            K::Load { ty, pointer } => self.gen_load(*ty, *pointer, inst)?,
            K::Store { pointer, value } => self.gen_store(*pointer, *value)?,
            K::PtrOffset { pointer, offset } => self.gen_ptr_offset(*pointer, *offset, inst)?,
            K::Ext { to, value } => {
                let (from_bits, from_signed) = self.int_info(self.ty_of(*value));
                let to_bits = self.int_bits(*to);
                let src = self.disp[value.index()];
                let dst = self.result_disp(inst)?;
                self.load_int(Reg::Rax, src, from_bits, from_signed);
                self.store_int(Reg::Rax, dst, to_bits);
            }
            K::Trunc { to, value } => {
                let from_bits = self.int_bits(self.ty_of(*value));
                let to_bits = self.int_bits(*to);
                let src = self.disp[value.index()];
                let dst = self.result_disp(inst)?;
                self.load_int(Reg::Rax, src, from_bits, false);
                if to_bits == 1 {
                    self.emit(Inst::AluRI {
                        op: AluOp::And,
                        dst: Reg::Rax.at(Width::W32),
                        imm: 1,
                    });
                }
                self.store_int(Reg::Rax, dst, to_bits);
            }
            K::IntToFloat { to, value } => self.gen_int_to_float(*to, *value, inst)?,
            K::FloatToInt { to, value } => self.gen_float_to_int(*to, *value, inst)?,
            K::PtrCast { pointer, .. } => {
                let src = self.disp[pointer.index()];
                let dst = self.result_disp(inst)?;
                self.load_int(Reg::Rax, src, 64, false);
                self.store_int(Reg::Rax, dst, 64);
            }
            K::Call { callee, args } => self.gen_call(*callee, args, inst)?,
            K::Construct { ty, fields } => self.gen_construct(*ty, fields, inst)?,
            K::Extract { aggregate, index } => self.gen_extract(*aggregate, *index, inst)?,
            K::Insert {
                aggregate,
                index,
                value,
            } => self.gen_insert(*aggregate, *index, *value, inst)?,
        }
        Ok(())
    }

    fn materialize_const(&mut self, data: &ConstantData, dst: i32) -> Result<(), BackendError> {
        match data {
            ConstantData::Int { bits, width, .. } => {
                self.emit(Inst::MovRI {
                    dst: Reg::Rax.q(),
                    imm: *bits as u64,
                    style: ImmStyle::Hex,
                });
                self.store_int(Reg::Rax, dst, *width);
            }
            ConstantData::Float { ty: float_ty, bits } => {
                if is_f32(self.module.types(), *float_ty) {
                    self.emit(Inst::MovRI {
                        dst: Reg::Rax.at(Width::W32),
                        imm: *bits as u32 as u64,
                        style: ImmStyle::Hex,
                    });
                    self.store_int(Reg::Rax, dst, 32);
                } else {
                    self.emit(Inst::MovRI {
                        dst: Reg::Rax.q(),
                        imm: *bits,
                        style: ImmStyle::Hex,
                    });
                    self.store_int(Reg::Rax, dst, 64);
                }
            }
            ConstantData::Null { .. } => {
                self.zero(Reg::Rax);
                self.store_int(Reg::Rax, dst, 64);
            }
            ConstantData::String { bytes } => {
                let mut owned = bytes.clone();
                owned.push(0);
                let id = self.rodata.intern(&owned);
                self.emit(Inst::Lea {
                    dst: Reg::Rax,
                    mem: Mem::Data(id),
                });
                self.store_int(Reg::Rax, dst, 64);
            }
            ConstantData::Aggregate {
                ty: agg_ty,
                elements,
            } => {
                for (i, &element) in elements.iter().enumerate() {
                    let elem = self
                        .module
                        .constants()
                        .get(element)
                        .cloned()
                        .ok_or_else(|| BackendError::InvalidModule {
                            reason: format!("unknown aggregate element c{}", element.as_u32()),
                        })?;
                    layout::field_type(self.module.types(), *agg_ty, i as u64)?;
                    let off = layout::field_offset(self.module.types(), *agg_ty, i as u64)?;
                    self.materialize_const(&elem, dst + off as i32)?;
                }
            }
        }
        Ok(())
    }

    /// `xor r32, r32` — zeroes the full 64-bit register.
    fn zero(&mut self, reg: Reg) {
        let r = reg.at(Width::W32);
        self.emit(Inst::AluRR {
            op: AluOp::Xor,
            dst: r,
            src: r,
        });
    }

    /// Loads both operands into `rax`/`rcx` (or `xmm0`/`xmm1`).
    fn load_pair(&mut self, lhs: ValueId, rhs: ValueId, ty: TypeId) {
        let (lsrc, rsrc) = (self.disp[lhs.index()], self.disp[rhs.index()]);
        if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            self.load_float(XMM0, lsrc, f32);
            self.load_float(XMM1, rsrc, f32);
        } else {
            let (bits, signed) = self.int_info(ty);
            self.load_int(Reg::Rax, lsrc, bits, signed);
            self.load_int(Reg::Rcx, rsrc, bits, signed);
        }
    }

    fn gen_arith(
        &mut self,
        op: ArithOp,
        lhs: ValueId,
        rhs: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let ty = self.ty_of(lhs);
        let dst = self.result_disp(inst)?;
        self.load_pair(lhs, rhs, ty);
        if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            let op = match op {
                ArithOp::Add => SseOp::Add,
                ArithOp::Sub => SseOp::Sub,
                ArithOp::Mul => SseOp::Mul,
            };
            self.emit(Inst::FArith {
                op,
                fp: Fp::of(f32),
                dst: XMM0,
                src: XMM1,
            });
            self.store_float(XMM0, dst, f32);
        } else {
            let (bits, _) = self.int_info(ty);
            let (rax, rcx) = (Reg::Rax.q(), Reg::Rcx.q());
            self.emit(match op {
                ArithOp::Add => Inst::AluRR {
                    op: AluOp::Add,
                    dst: rax,
                    src: rcx,
                },
                ArithOp::Sub => Inst::AluRR {
                    op: AluOp::Sub,
                    dst: rax,
                    src: rcx,
                },
                ArithOp::Mul => Inst::ImulRR {
                    dst: Reg::Rax,
                    src: Reg::Rcx,
                },
            });
            self.store_int(Reg::Rax, dst, bits);
        }
        Ok(())
    }

    fn gen_bitwise(
        &mut self,
        op: AluOp,
        lhs: ValueId,
        rhs: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let ty = self.ty_of(lhs);
        let (bits, _) = self.int_info(ty);
        let dst = self.result_disp(inst)?;
        self.load_pair(lhs, rhs, ty);
        self.emit(Inst::AluRR {
            op,
            dst: Reg::Rax.q(),
            src: Reg::Rcx.q(),
        });
        self.store_int(Reg::Rax, dst, bits);
        Ok(())
    }

    fn gen_div(
        &mut self,
        lhs: ValueId,
        rhs: ValueId,
        inst: &Instruction,
        is_rem: bool,
    ) -> Result<(), BackendError> {
        let ty = self.ty_of(lhs);
        let dst = self.result_disp(inst)?;
        self.load_pair(lhs, rhs, ty);
        if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            self.emit(Inst::FArith {
                op: SseOp::Div,
                fp: Fp::of(f32),
                dst: XMM0,
                src: XMM1,
            });
            self.store_float(XMM0, dst, f32);
            return Ok(());
        }
        let (bits, signed) = self.int_info(ty);
        if signed {
            // x86 IDIV traps on INT64_MIN / -1. Astronomy instead wraps
            // the quotient and defines the corresponding remainder as zero.
            let done = self.tmp_label("divdone");
            if bits == 64 {
                let normal = self.tmp_label("divnormal");
                self.emit(Inst::AluRI {
                    op: AluOp::Cmp,
                    dst: Reg::Rcx.q(),
                    imm: -1,
                });
                self.emit(Inst::Jcc(Cond::Ne, normal));
                self.emit(Inst::MovRI {
                    dst: Reg::Rdx.q(),
                    imm: 0x8000_0000_0000_0000,
                    style: ImmStyle::Hex,
                });
                self.emit(Inst::AluRR {
                    op: AluOp::Cmp,
                    dst: Reg::Rax.q(),
                    src: Reg::Rdx.q(),
                });
                self.emit(Inst::Jcc(Cond::Ne, normal));
                self.zero(Reg::Rdx);
                self.emit(Inst::Jmp(done));
                self.emit(Inst::Label(normal));
            }
            self.emit(Inst::Cqo);
            self.emit(Inst::Div {
                signed: true,
                src: Reg::Rcx,
            });
            if bits == 64 {
                self.emit(Inst::Label(done));
            }
        } else {
            self.zero(Reg::Rdx);
            self.emit(Inst::Div {
                signed: false,
                src: Reg::Rcx,
            });
        }
        if is_rem {
            self.emit(Inst::MovRR {
                dst: Reg::Rax.q(),
                src: Reg::Rdx.q(),
            });
        }
        self.store_int(Reg::Rax, dst, bits);
        Ok(())
    }

    fn gen_shift(
        &mut self,
        lhs: ValueId,
        rhs: ValueId,
        inst: &Instruction,
        is_left: bool,
    ) -> Result<(), BackendError> {
        let ty = self.ty_of(lhs);
        let (bits, signed) = self.int_info(ty);
        let (lsrc, rsrc) = (self.disp[lhs.index()], self.disp[rhs.index()]);
        let dst = self.result_disp(inst)?;
        self.load_int(Reg::Rax, lsrc, bits, signed);
        let count_bits = self.int_bits(self.ty_of(rhs));
        self.load_int(Reg::Rcx, rsrc, count_bits, false);
        let special = self.tmp_label("shovf");
        let done = self.tmp_label("shdone");
        self.emit(Inst::AluRI {
            op: AluOp::Cmp,
            dst: Reg::Rcx.q(),
            imm: bits as i32,
        });
        self.emit(Inst::Jcc(Cond::Ae, special));
        let op = if is_left {
            ShiftOp::Shl
        } else if signed {
            ShiftOp::Sar
        } else {
            ShiftOp::Shr
        };
        self.emit(Inst::ShiftCl { op, dst: Reg::Rax });
        self.emit(Inst::Jmp(done));
        self.emit(Inst::Label(special));
        if is_left || !signed {
            self.zero(Reg::Rax);
        } else {
            self.emit(Inst::ShiftRI {
                op: ShiftOp::Sar,
                dst: Reg::Rax,
                imm: 63,
            });
        }
        self.emit(Inst::Label(done));
        self.store_int(Reg::Rax, dst, bits);
        Ok(())
    }

    fn gen_cmp(
        &mut self,
        op: CmpOp,
        lhs: ValueId,
        rhs: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let ty = self.ty_of(lhs);
        let dst = self.result_disp(inst)?;
        self.load_pair(lhs, rhs, ty);
        let setcc = |cond, dst| Inst::Setcc { cond, dst };
        let (al, cl) = (Reg::Rax.at(Width::W8), Reg::Rcx.at(Width::W8));
        if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            self.emit(Inst::Ucomi {
                fp: Fp::of(f32),
                a: XMM0,
                b: XMM1,
            });
            // ucomis* reports NaN operands as "unordered" through PF.
            let (cond, parity) = match op {
                CmpOp::Eq => (Cond::E, Some((Cond::Np, AluOp::And))),
                CmpOp::Ne => (Cond::Ne, Some((Cond::P, AluOp::Or))),
                CmpOp::Lt => (Cond::B, Some((Cond::Np, AluOp::And))),
                CmpOp::Le => (Cond::Be, Some((Cond::Np, AluOp::And))),
                CmpOp::Gt => (Cond::A, None),
                CmpOp::Ge => (Cond::Ae, None),
            };
            self.emit(setcc(cond, Reg::Rax));
            if let Some((pcond, combine)) = parity {
                self.emit(setcc(pcond, Reg::Rcx));
                self.emit(Inst::AluRR {
                    op: combine,
                    dst: al,
                    src: cl,
                });
            }
        } else {
            let (_, signed) = self.int_info(ty);
            self.emit(Inst::AluRR {
                op: AluOp::Cmp,
                dst: Reg::Rax.q(),
                src: Reg::Rcx.q(),
            });
            let cond = match (op, signed) {
                (CmpOp::Eq, _) => Cond::E,
                (CmpOp::Ne, _) => Cond::Ne,
                (CmpOp::Lt, true) => Cond::L,
                (CmpOp::Lt, false) => Cond::B,
                (CmpOp::Le, true) => Cond::Le,
                (CmpOp::Le, false) => Cond::Be,
                (CmpOp::Gt, true) => Cond::G,
                (CmpOp::Gt, false) => Cond::A,
                (CmpOp::Ge, true) => Cond::Ge,
                (CmpOp::Ge, false) => Cond::Ae,
            };
            self.emit(setcc(cond, Reg::Rax));
        }
        self.store_int(Reg::Rax, dst, 8);
        Ok(())
    }

    fn gen_load(
        &mut self,
        ty: TypeId,
        pointer: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let psrc = self.disp[pointer.index()];
        let dst = self.result_disp(inst)?;
        self.load_int(Reg::Rax, psrc, 64, false);
        let at_rax = Mem::at(Reg::Rax, 0);
        if is_aggregate(self.module.types(), ty) {
            let size = self.size(ty)?;
            self.emit(Inst::MovRR {
                dst: Reg::Rsi.q(),
                src: Reg::Rax.q(),
            });
            self.emit(Inst::Lea {
                dst: Reg::Rdi,
                mem: Self::slot(dst),
            });
            self.emit_byte_copy(size);
        } else if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            self.emit(Inst::FLoad {
                fp: Fp::of(f32),
                dst: XMM0,
                mem: at_rax,
            });
            self.store_float(XMM0, dst, f32);
        } else {
            let (bits, signed) = self.int_info(ty);
            self.load_mem(Reg::Rdx, at_rax, bits, signed);
            if bits == 1 {
                self.emit(Inst::AluRI {
                    op: AluOp::And,
                    dst: Reg::Rdx.at(Width::W32),
                    imm: 1,
                });
            }
            self.store_int(Reg::Rdx, dst, bits);
        }
        Ok(())
    }

    fn gen_store(&mut self, pointer: ValueId, value: ValueId) -> Result<(), BackendError> {
        let psrc = self.disp[pointer.index()];
        let vsrc = self.disp[value.index()];
        let ty = self.ty_of(value);
        self.load_int(Reg::Rax, psrc, 64, false);
        let at_rax = Mem::at(Reg::Rax, 0);
        if is_aggregate(self.module.types(), ty) {
            let size = self.size(ty)?;
            self.emit(Inst::MovRR {
                dst: Reg::Rdi.q(),
                src: Reg::Rax.q(),
            });
            self.emit(Inst::Lea {
                dst: Reg::Rsi,
                mem: Self::slot(vsrc),
            });
            self.emit_byte_copy(size);
        } else if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            self.load_float(XMM0, vsrc, f32);
            self.emit(Inst::FStore {
                fp: Fp::of(f32),
                mem: at_rax,
                src: XMM0,
            });
        } else {
            let (bits, signed) = self.int_info(ty);
            self.load_int(Reg::Rdx, vsrc, bits, signed);
            self.store_mem(Reg::Rdx, at_rax, bits);
        }
        Ok(())
    }

    fn gen_ptr_offset(
        &mut self,
        pointer: ValueId,
        offset: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let pointee = match self.module.types().get(self.ty_of(pointer)) {
            Some(TypeData::Pointer { pointee, .. }) => *pointee,
            _ => {
                return Err(BackendError::InvalidModule {
                    reason: "ptr_offset base is not a pointer".to_string(),
                })
            }
        };
        let elem_size = self.size(pointee)?;
        let obits = self.int_bits(self.ty_of(offset));
        let (psrc, osrc) = (self.disp[pointer.index()], self.disp[offset.index()]);
        let dst = self.result_disp(inst)?;
        self.load_int(Reg::Rax, psrc, 64, false);
        // Offsets are signed bit patterns regardless of the IR type's signedness.
        self.load_int(Reg::Rcx, osrc, obits, true);
        if elem_size <= i32::MAX as u64 {
            self.emit(Inst::ImulRI {
                dst: Reg::Rcx,
                imm: elem_size as i32,
            });
        } else {
            // IMUL's immediate is only a sign-extended 32-bit value.
            self.emit(Inst::MovRI {
                dst: Reg::Rdx.q(),
                imm: elem_size,
                style: ImmStyle::Hex,
            });
            self.emit(Inst::ImulRR {
                dst: Reg::Rcx,
                src: Reg::Rdx,
            });
        }
        self.emit(Inst::AluRR {
            op: AluOp::Add,
            dst: Reg::Rax.q(),
            src: Reg::Rcx.q(),
        });
        self.store_int(Reg::Rax, dst, 64);
        Ok(())
    }

    fn gen_int_to_float(
        &mut self,
        to: TypeId,
        value: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let f32 = is_f32(self.module.types(), to);
        let fp = Fp::of(f32);
        let (from_bits, from_signed) = self.int_info(self.ty_of(value));
        let src = self.disp[value.index()];
        let dst = self.result_disp(inst)?;
        let cvt = |src| Inst::CvtSi2F { fp, dst: XMM0, src };
        if from_signed || from_bits < 64 {
            self.load_int(Reg::Rax, src, from_bits, from_signed);
            self.emit(cvt(Reg::Rax));
        } else {
            // Unsigned 64-bit cannot use cvtsi2* directly: halve (keeping
            // the low bit for correct rounding), convert, then double.
            let big = self.tmp_label("u2f");
            let done = self.tmp_label("u2fd");
            self.load_int(Reg::Rax, src, 64, false);
            self.emit(Inst::Test {
                a: Reg::Rax.q(),
                b: Reg::Rax.q(),
            });
            self.emit(Inst::Jcc(Cond::S, big));
            self.emit(cvt(Reg::Rax));
            self.emit(Inst::Jmp(done));
            self.emit(Inst::Label(big));
            self.emit(Inst::MovRR {
                dst: Reg::Rcx.q(),
                src: Reg::Rax.q(),
            });
            self.emit(Inst::ShiftRI {
                op: ShiftOp::Shr,
                dst: Reg::Rcx,
                imm: 1,
            });
            self.emit(Inst::AluRI {
                op: AluOp::And,
                dst: Reg::Rax.at(Width::W32),
                imm: 1,
            });
            self.emit(Inst::AluRR {
                op: AluOp::Or,
                dst: Reg::Rcx.q(),
                src: Reg::Rax.q(),
            });
            self.emit(cvt(Reg::Rcx));
            self.emit(Inst::FArith {
                op: SseOp::Add,
                fp,
                dst: XMM0,
                src: XMM0,
            });
            self.emit(Inst::Label(done));
        }
        self.store_float(XMM0, dst, f32);
        Ok(())
    }

    /// `ucomisd xmm0, xmm1` against a read-only `f64` bound.
    fn compare_with_bound(&mut self, bound: f64) {
        let id = self.rodata.intern_f64(bound);
        self.compare_with_data(id);
    }

    fn compare_with_data(&mut self, id: DataId) {
        self.emit(Inst::FLoad {
            fp: Fp::Double,
            dst: XMM1,
            mem: Mem::Data(id),
        });
        self.emit(Inst::Ucomi {
            fp: Fp::Double,
            a: XMM0,
            b: XMM1,
        });
    }

    fn mov_rax_hex(&mut self, imm: u64) {
        self.emit(Inst::MovRI {
            dst: Reg::Rax.q(),
            imm,
            style: ImmStyle::Hex,
        });
    }

    fn gen_float_to_int(
        &mut self,
        to: TypeId,
        value: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let from_f32 = is_f32(self.module.types(), self.ty_of(value));
        let (to_bits, to_signed) = self.int_info(to);
        let src = self.disp[value.index()];
        let dst = self.result_disp(inst)?;
        let truncate = Inst::Cvttsd2si {
            dst: Reg::Rax,
            src: XMM0,
        };

        self.load_float(XMM0, src, from_f32);
        if from_f32 {
            self.emit(Inst::Cvtss2sd {
                dst: XMM0,
                src: XMM0,
            });
        }

        let zero = self.tmp_label("ftiz");
        let done = self.tmp_label("ftid");
        let sat_max = self.tmp_label("ftimax");

        self.emit(Inst::Ucomi {
            fp: Fp::Double,
            a: XMM0,
            b: XMM0,
        });
        self.emit(Inst::Jcc(Cond::P, zero));

        if to_signed {
            let sat_min = self.tmp_label("ftimin");
            let bound = 2f64.powi((to_bits - 1) as i32);
            let min_id = self.rodata.intern_f64(-bound);
            let max_id = self.rodata.intern_f64(bound);
            self.compare_with_data(max_id);
            self.emit(Inst::Jcc(Cond::Ae, sat_max));
            self.compare_with_data(min_id);
            self.emit(Inst::Jcc(Cond::B, sat_min));
            self.emit(truncate.clone());
            self.emit(Inst::Jmp(done));
            self.emit(Inst::Label(sat_max));
            let max_value: u64 = if to_bits == 64 {
                i64::MAX as u64
            } else {
                (1u64 << (to_bits - 1)) - 1
            };
            self.mov_rax_hex(max_value);
            self.emit(Inst::Jmp(done));
            self.emit(Inst::Label(sat_min));
            self.mov_rax_hex(1u64 << (to_bits - 1));
            self.emit(Inst::Jmp(done));
        } else {
            // Reject negatives (`-0.5` truncates to `0`, and any x < 0
            // saturates to 0, so mapping every negative to 0 is correct).
            self.emit(Inst::Xorpd {
                dst: XMM1,
                src: XMM1,
            });
            self.emit(Inst::Ucomi {
                fp: Fp::Double,
                a: XMM0,
                b: XMM1,
            });
            self.emit(Inst::Jcc(Cond::B, zero));
            self.compare_with_bound(2f64.powi(to_bits as i32));
            self.emit(Inst::Jcc(Cond::Ae, sat_max));
            if to_bits == 64 {
                let small = self.tmp_label("ftismall");
                self.compare_with_bound(2f64.powi(63));
                self.emit(Inst::Jcc(Cond::B, small));
                self.emit(Inst::FArith {
                    op: SseOp::Sub,
                    fp: Fp::Double,
                    dst: XMM0,
                    src: XMM1,
                });
                self.emit(truncate.clone());
                self.emit(Inst::MovRI {
                    dst: Reg::Rcx.q(),
                    imm: 0x8000_0000_0000_0000,
                    style: ImmStyle::Hex,
                });
                self.emit(Inst::AluRR {
                    op: AluOp::Add,
                    dst: Reg::Rax.q(),
                    src: Reg::Rcx.q(),
                });
                self.emit(Inst::Jmp(done));
                self.emit(Inst::Label(small));
            }
            self.emit(truncate);
            self.emit(Inst::Jmp(done));
            self.emit(Inst::Label(sat_max));
            let max_value: u64 = if to_bits == 64 {
                u64::MAX
            } else {
                (1u64 << to_bits) - 1
            };
            self.mov_rax_hex(max_value);
            self.emit(Inst::Jmp(done));
        }

        self.emit(Inst::Label(zero));
        self.zero(Reg::Rax);
        self.emit(Inst::Label(done));
        self.store_int(Reg::Rax, dst, to_bits);
        Ok(())
    }

    fn gen_call(
        &mut self,
        callee: FunctionId,
        args: &[ValueId],
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let arg_types: Vec<TypeId> = args.iter().map(|a| self.ty_of(*a)).collect();
        let (locs, _bytes, sse_used) = self.plan_types(&arg_types, "call")?;

        for (arg, loc) in args.iter().zip(locs.iter()) {
            let src = self.disp[arg.index()];
            let ty = self.ty_of(*arg);
            match *loc {
                Loc::Int(reg) => {
                    let (bits, signed) = self.int_info(ty);
                    self.load_int(reg, src, bits, signed);
                }
                Loc::Sse(n) => {
                    let f32 = is_f32(self.module.types(), ty);
                    self.load_float(Xmm(n), src, f32);
                }
                Loc::Stack(off) => {
                    let bits = (self.size(ty)? * 8) as u32;
                    self.load_int(Reg::Rax, src, bits, false);
                    let at = self.stack_disp(0, off, "call")?;
                    self.emit(Inst::Store {
                        mem: Mem::at(Reg::Rsp, at),
                        src: Reg::Rax.q(),
                    });
                }
            }
        }

        let callee_fn =
            self.module
                .function(callee)
                .ok_or_else(|| BackendError::InvalidModule {
                    reason: format!("unknown callee f{}", callee.as_u32()),
                })?;
        if callee_fn.variadic {
            self.emit(Inst::MovRI {
                dst: Reg::Rax.at(Width::W8),
                imm: sse_used as u64,
                style: ImmStyle::Dec,
            });
        }
        self.emit(Inst::Call(callee));

        if let Some(result) = inst.result {
            let rty = self.ty_of(result);
            let dst = self.disp[result.index()];
            if is_aggregate(self.module.types(), rty) {
                return Err(BackendError::UnsupportedInstruction {
                    function: self.fname.clone(),
                    op: "call",
                    reason: "aggregate return values are not supported".to_string(),
                });
            }
            if is_float(self.module.types(), rty) {
                let f32 = is_f32(self.module.types(), rty);
                self.store_float(XMM0, dst, f32);
            } else {
                let (bits, _) = self.int_info(rty);
                self.store_int(Reg::Rax, dst, bits);
            }
        }
        Ok(())
    }

    fn gen_construct(
        &mut self,
        ty: TypeId,
        fields: &[ValueId],
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let dst = self.result_disp(inst)?;
        for (i, field) in fields.iter().enumerate() {
            let fty = layout::field_type(self.module.types(), ty, i as u64)?;
            let off = layout::field_offset(self.module.types(), ty, i as u64)?;
            let src = self.disp[field.index()];
            self.copy_slot(src, fty, dst + off as i32)?;
        }
        Ok(())
    }

    fn gen_extract(
        &mut self,
        aggregate: ValueId,
        index: u32,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let aty = self.ty_of(aggregate);
        let fty = layout::field_type(self.module.types(), aty, index as u64)?;
        let off = layout::field_offset(self.module.types(), aty, index as u64)?;
        let src = self.disp[aggregate.index()] + off as i32;
        let dst = self.result_disp(inst)?;
        self.copy_slot(src, fty, dst)
    }

    fn gen_insert(
        &mut self,
        aggregate: ValueId,
        index: u32,
        value: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let aty = self.ty_of(aggregate);
        let fty = layout::field_type(self.module.types(), aty, index as u64)?;
        let off = layout::field_offset(self.module.types(), aty, index as u64)?;
        let adv = self.disp[aggregate.index()];
        let vd = self.disp[value.index()];
        let dst = self.result_disp(inst)?;
        // Copy the whole aggregate, then overwrite the one field.
        self.copy_slot(adv, aty, dst)?;
        self.copy_slot(vd, fty, dst + off as i32)
    }
}
