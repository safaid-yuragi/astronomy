//! NASM (x86-64 System V) code generation for verified Astronomy modules.
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
//! Unsupported constructs (128-bit integers, aggregates passed/returned by
//! value) produce structured [`BackendError`]s instead of wrong code.

use std::collections::HashMap;
use std::fmt::Write as _;

use astronomy::{
    ConstantData, Function, Instruction, InstructionKind, Linkage, Module, Terminator, TypeData,
    TypeId, ValueId, VerifiedModule,
};

use crate::error::BackendError;
use crate::layout::{self, align_up, align_of, is_aggregate, is_f32, is_float, size_of};

/// Lowers a verified module to NASM source text.
pub fn compile(module: &VerifiedModule) -> Result<String, BackendError> {
    let mut generator = Generator {
        module: module.module(),
        out: String::new(),
        rodata: Rodata::default(),
    };
    generator.run()?;
    Ok(generator.finish())
}

// ---------------------------------------------------------------------------
// Read-only data (strings and float-conversion bounds)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Rodata {
    entries: Vec<(String, Vec<u8>)>,
    cache: HashMap<Vec<u8>, String>,
    counter: usize,
}

impl Rodata {
    fn intern(&mut self, bytes: &[u8]) -> String {
        if let Some(label) = self.cache.get(bytes) {
            return label.clone();
        }
        let label = format!("arn.data.{}", self.counter);
        self.counter += 1;
        self.entries.push((label.clone(), bytes.to_vec()));
        self.cache.insert(bytes.to_vec(), label.clone());
        label
    }

    fn intern_f64(&mut self, value: f64) -> String {
        self.intern(&value.to_bits().to_le_bytes())
    }

    fn emit(&self, out: &mut String) {
        out.push_str("section .rodata\n");
        for (label, bytes) in &self.entries {
            let _ = writeln!(out, "{label}:");
            if bytes.is_empty() {
                out.push_str("    db 0\n");
                continue;
            }
            for chunk in bytes.chunks(16) {
                let parts: Vec<String> = chunk.iter().map(|b| format!("0x{b:02x}")).collect();
                let _ = writeln!(out, "    db {}", parts.join(", "));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Module-level driver
// ---------------------------------------------------------------------------

struct Generator<'a> {
    module: &'a Module,
    out: String,
    rodata: Rodata,
}

impl Generator<'_> {
    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn run(&mut self) -> Result<(), BackendError> {
        self.line("bits 64");
        self.line("default rel");
        self.line("");

        let module = self.module;
        for function in module.functions() {
            let name = module
                .symbol_name(function.symbol)
                .unwrap_or("<invalid>")
                .to_string();
            if function.is_declaration() {
                self.line(&format!("extern ${name}"));
            } else if !matches!(function.linkage, Linkage::Internal) {
                self.line(&format!("global ${name}"));
            }
        }
        self.line("");
        self.line("section .text");

        for function in module.functions() {
            if function.is_declaration() {
                continue;
            }
            let mut func = FuncGen::new(module, function, &mut self.rodata);
            func.build()?;
            self.out.push_str(&func.out);
        }

        if !self.rodata.entries.is_empty() {
            self.line("");
            self.rodata.emit(&mut self.out);
        }
        Ok(())
    }

    fn finish(self) -> String {
        self.out
    }
}

// ---------------------------------------------------------------------------
// Per-function code generation
// ---------------------------------------------------------------------------

/// Where a System V argument is passed.
#[derive(Debug, Clone, Copy)]
enum Loc {
    /// Integer/pointer register (System V order).
    Int(&'static str),
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

struct FuncGen<'a> {
    module: &'a Module,
    f: &'a Function,
    fname: String,
    rodata: &'a mut Rodata,
    out: String,
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
        FuncGen {
            module,
            f,
            fname,
            rodata,
            out: String::new(),
            disp: Vec::new(),
            alloca: HashMap::new(),
            scratch: Vec::new(),
            frame_size: 0,
            tmp: 0,
        }
    }

    // -- small helpers ------------------------------------------------------

    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn mem(&self, d: i32) -> String {
        format!("[rbp{d:+}]")
    }

    fn tmp_label(&mut self, tag: &str) -> String {
        self.tmp += 1;
        format!("{}.{}{}", self.fname, tag, self.tmp)
    }

    fn block_label(&self, block: usize) -> String {
        format!("{}.bb{}", self.fname, block)
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

    // -- emitting values ----------------------------------------------------

    fn load_int(&mut self, reg: &str, d: i32, bits: u32, signed: bool) {
        let m = self.mem(d);
        if bits == 64 {
            self.line(&format!("    mov {reg}, qword {m}"));
        } else if bits == 32 {
            if signed {
                self.line(&format!("    movsxd {reg}, dword {m}"));
            } else {
                let r = subreg(reg, 4);
                self.line(&format!("    mov {r}, dword {m}"));
            }
        } else if bits == 16 {
            if signed {
                self.line(&format!("    movsx {reg}, word {m}"));
            } else {
                self.line(&format!("    movzx {reg}, word {m}"));
            }
        } else if signed && bits == 8 {
            self.line(&format!("    movsx {reg}, byte {m}"));
        } else {
            self.line(&format!("    movzx {reg}, byte {m}"));
        }
    }

    fn store_int(&mut self, reg: &str, d: i32, bits: u32) {
        let bytes = (bits as u64).div_ceil(8).max(1);
        let r = subreg(reg, bytes);
        let m = self.mem(d);
        self.line(&format!("    mov {} {m}, {r}", size_kw(bytes)));
    }

    fn load_float(&mut self, xmm: &str, d: i32, f32: bool) {
        let m = self.mem(d);
        if f32 {
            self.line(&format!("    movss {xmm}, dword {m}"));
        } else {
            self.line(&format!("    movsd {xmm}, qword {m}"));
        }
    }

    fn store_float(&mut self, xmm: &str, d: i32, f32: bool) {
        let m = self.mem(d);
        if f32 {
            self.line(&format!("    movss dword {m}, {xmm}"));
        } else {
            self.line(&format!("    movsd qword {m}, {xmm}"));
        }
    }

    fn load_from_addr(&mut self, dst: &str, addr: &str, bits: u32, signed: bool) {
        if bits == 64 {
            self.line(&format!("    mov {dst}, qword [{addr}]"));
        } else if bits == 32 {
            if signed {
                self.line(&format!("    movsxd {dst}, dword [{addr}]"));
            } else {
                let r = subreg(dst, 4);
                self.line(&format!("    mov {r}, dword [{addr}]"));
            }
        } else if bits == 16 {
            if signed {
                self.line(&format!("    movsx {dst}, word [{addr}]"));
            } else {
                self.line(&format!("    movzx {dst}, word [{addr}]"));
            }
        } else if signed && bits == 8 {
            self.line(&format!("    movsx {dst}, byte [{addr}]"));
        } else {
            self.line(&format!("    movzx {dst}, byte [{addr}]"));
        }
    }

    fn store_to_addr(&mut self, addr: &str, value: &str, bits: u32) {
        let bytes = (bits as u64).div_ceil(8).max(1);
        let r = subreg(value, bytes);
        self.line(&format!("    mov {} [{addr}], {r}", size_kw(bytes)));
    }

    /// Copies a whole value between two frame slots (bit-for-bit).
    fn copy_slot(&mut self, src: i32, ty: TypeId, dst: i32) -> Result<(), BackendError> {
        let agg = is_aggregate(self.module.types(), ty);
        let size = self.size(ty)?;
        if agg {
            let (sm, dm) = (self.mem(src), self.mem(dst));
            self.line(&format!("    lea rsi, {sm}"));
            self.line(&format!("    lea rdi, {dm}"));
            self.line(&format!("    mov rcx, {size}"));
            self.line("    cld");
            self.line("    rep movsb");
        } else {
            let bits = (size * 8) as u32;
            self.load_int("rax", src, bits, false);
            self.store_int("rax", dst, bits);
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
            layout::check_supported(
                store,
                value.ty,
                &format!("value v{i} of `{}`", self.fname),
            )?;
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
                    let types: Vec<TypeId> =
                        args.iter().map(|a| self.f.value(*a).expect("value").ty).collect();
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
        const GP: [&str; 6] = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];
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
                        locs.push(Loc::Int(GP[gp]));
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
                            if op == "parameter" { "parameter" } else { "argument" },
                            astronomy::types::type_to_string(self.module.types(), ty)
                        ),
                    });
                }
            }
        }
        Ok((locs, stack, sse))
    }

    // -- prologue / blocks --------------------------------------------------

    fn emit_prologue(&mut self) -> Result<(), BackendError> {
        let label = self.fname.clone();
        // `$` forces NASM to treat even register/directive names as symbols.
        self.line(&format!("${label}:"));
        self.line("    push rbp");
        self.line("    mov rbp, rsp");
        if self.frame_size > 0 {
            self.line(&format!("    sub rsp, {}", self.frame_size));
        }

        let ptypes: Vec<TypeId> = self.f.params.iter().map(|p| p.ty).collect();
        let (locs, _bytes, _sse) = self.plan_types(&ptypes, "parameter")?;
        for (i, loc) in locs.iter().enumerate() {
            let value = self.f.params[i].value;
            let d = self.disp[value.index()];
            let ty = ptypes[i];
            match loc {
                Loc::Int(reg) => {
                    let (bits, _) = self.int_info(ty);
                    self.store_int(reg, d, bits);
                }
                Loc::Sse(n) => {
                    let f32 = is_f32(self.module.types(), ty);
                    self.store_float(&format!("xmm{n}"), d, f32);
                }
                Loc::Stack(off) => {
                    let bytes = self.size(ty)?;
                    self.line(&format!("mov rax, qword [rbp+{}]", 16 + off));
                    self.store_int("rax", d, (bytes * 8) as u32);
                }
            }
        }
        Ok(())
    }

    fn emit_block(&mut self, index: usize) -> Result<(), BackendError> {
        let block = self.f.blocks[index].clone();
        let label = self.block_label(index);
        self.line("");
        self.line(&format!("{label}:"));
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
        let ptypes: Vec<TypeId> = self.f.blocks[target]
            .params
            .iter()
            .map(|p| p.ty)
            .collect();
        let pdisps: Vec<i32> = self.f.blocks[target]
            .params
            .iter()
            .map(|p| self.disp[p.value.index()])
            .collect();
        let scratch = self.scratch[target].clone();

        // Parallel copy: stage every argument first, then commit.
        for (i, arg) in args.iter().enumerate() {
            let src = self.disp[arg.index()];
            self.copy_slot(src, ptypes[i], scratch[i])?;
        }
        for i in 0..args.len() {
            self.copy_slot(scratch[i], ptypes[i], pdisps[i])?;
        }
        Ok(())
    }

    fn gen_term(&mut self, term: &Terminator) -> Result<(), BackendError> {
        match term {
            Terminator::Jump { target, args } => {
                self.emit_edge(target.index(), args)?;
                let label = self.block_label(target.index());
                self.line(&format!("    jmp {label}"));
            }
            Terminator::Branch {
                condition,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let cdisp = self.disp[condition.index()];
                let m = self.mem(cdisp);
                let else_label = self.tmp_label("else");
                self.line(&format!("    cmp byte {m}, 0"));
                self.line(&format!("    je {else_label}"));
                self.emit_edge(then_block.index(), then_args)?;
                let then_label = self.block_label(then_block.index());
                self.line(&format!("    jmp {then_label}"));
                self.line(&format!("{else_label}:"));
                self.emit_edge(else_block.index(), else_args)?;
                let else_label2 = self.block_label(else_block.index());
                self.line(&format!("    jmp {else_label2}"));
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
                        self.load_float("xmm0", d, f32);
                    } else {
                        let (bits, signed) = self.int_info(ty);
                        self.load_int("rax", d, bits, signed);
                    }
                }
                self.line("    leave");
                self.line("    ret");
            }
            Terminator::Unreachable => {
                self.line("    ud2");
            }
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
                let data = self
                    .module
                    .constants()
                    .get(*cid)
                    .cloned()
                    .ok_or_else(|| BackendError::InvalidModule {
                        reason: format!("unknown constant c{}", cid.as_u32()),
                    })?;
                let ty = self.ty_of(inst.result.expect("const result"));
                let dst = self.result_disp(inst)?;
                self.materialize_const(&data, ty, dst)?;
            }
            K::Add { lhs, rhs } => self.gen_arith(ArithOp::Add, *lhs, *rhs, inst)?,
            K::Sub { lhs, rhs } => self.gen_arith(ArithOp::Sub, *lhs, *rhs, inst)?,
            K::Mul { lhs, rhs } => self.gen_arith(ArithOp::Mul, *lhs, *rhs, inst)?,
            K::Div { lhs, rhs } => self.gen_div(*lhs, *rhs, inst, false)?,
            K::Rem { lhs, rhs } => self.gen_div(*lhs, *rhs, inst, true)?,
            K::And { lhs, rhs } => self.gen_bitwise("and", *lhs, *rhs, inst)?,
            K::Or { lhs, rhs } => self.gen_bitwise("or", *lhs, *rhs, inst)?,
            K::Xor { lhs, rhs } => self.gen_bitwise("xor", *lhs, *rhs, inst)?,
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
                let offset = self
                    .alloca
                    .get(&result.as_u32())
                    .copied()
                    .ok_or_else(|| BackendError::InvalidModule {
                        reason: format!("alloca in `{}` has no storage", self.fname),
                    })?;
                let m = self.mem(offset);
                self.line(&format!("    lea rax, {m}"));
                let dst = self.disp[result.index()];
                self.store_int("rax", dst, 64);
            }
            K::Load { ty, pointer } => self.gen_load(*ty, *pointer, inst)?,
            K::Store { pointer, value } => self.gen_store(*pointer, *value)?,
            K::PtrOffset { pointer, offset } => self.gen_ptr_offset(*pointer, *offset, inst)?,
            K::Ext { to, value } => {
                let (from_bits, from_signed) = self.int_info(self.ty_of(*value));
                let to_bits = self.int_bits(*to);
                let src = self.disp[value.index()];
                let dst = self.result_disp(inst)?;
                self.load_int("rax", src, from_bits, from_signed);
                self.store_int("rax", dst, to_bits);
            }
            K::Trunc { to, value } => {
                let from_bits = self.int_bits(self.ty_of(*value));
                let to_bits = self.int_bits(*to);
                let src = self.disp[value.index()];
                let dst = self.result_disp(inst)?;
                self.load_int("rax", src, from_bits, false);
                if to_bits == 1 {
                    self.line("    and eax, 1");
                }
                self.store_int("rax", dst, to_bits);
            }
            K::IntToFloat { to, value } => self.gen_int_to_float(*to, *value, inst)?,
            K::FloatToInt { to, value } => self.gen_float_to_int(*to, *value, inst)?,
            K::PtrCast { pointer, .. } => {
                let src = self.disp[pointer.index()];
                let dst = self.result_disp(inst)?;
                self.load_int("rax", src, 64, false);
                self.store_int("rax", dst, 64);
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

    fn materialize_const(
        &mut self,
        data: &ConstantData,
        _ty: TypeId,
        dst: i32,
    ) -> Result<(), BackendError> {
        match data {
            ConstantData::Int { bits, width, .. } => {
                self.line(&format!("    mov rax, 0x{:x}", *bits as u64));
                self.store_int("rax", dst, *width);
            }
            ConstantData::Float { ty: float_ty, bits } => {
                if is_f32(self.module.types(), *float_ty) {
                    self.line(&format!("    mov eax, 0x{:x}", *bits as u32));
                    self.store_int("rax", dst, 32);
                } else {
                    self.line(&format!("    mov rax, 0x{:x}", bits));
                    self.store_int("rax", dst, 64);
                }
            }
            ConstantData::Null { .. } => {
                self.line("    xor eax, eax");
                self.store_int("rax", dst, 64);
            }
            ConstantData::String { bytes } => {
                let mut owned = bytes.clone();
                owned.push(0);
                let label = self.rodata.intern(&owned);
                self.line(&format!("    lea rax, [rel {label}]"));
                self.store_int("rax", dst, 64);
            }
            ConstantData::Aggregate { ty: agg_ty, elements } => {
                for (i, &element) in elements.iter().enumerate() {
                    let elem = self
                        .module
                        .constants()
                        .get(element)
                        .cloned()
                        .ok_or_else(|| BackendError::InvalidModule {
                            reason: format!("unknown aggregate element c{}", element.as_u32()),
                        })?;
                    let fty = layout::field_type(self.module.types(), *agg_ty, i as u64)?;
                    let off = layout::field_offset(self.module.types(), *agg_ty, i as u64)?;
                    self.materialize_const(&elem, fty, dst + off as i32)?;
                }
            }
        }
        Ok(())
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
        let (lsrc, rsrc) = (self.disp[lhs.index()], self.disp[rhs.index()]);
        if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            let mnemonic = match op {
                ArithOp::Add => "add",
                ArithOp::Sub => "sub",
                ArithOp::Mul => "mul",
            };
            let suffix = if f32 { "ss" } else { "sd" };
            self.load_float("xmm0", lsrc, f32);
            self.load_float("xmm1", rsrc, f32);
            self.line(&format!("    {mnemonic}{suffix} xmm0, xmm1"));
            self.store_float("xmm0", dst, f32);
        } else {
            let (bits, signed) = self.int_info(ty);
            let mnemonic = match op {
                ArithOp::Add => "add",
                ArithOp::Sub => "sub",
                ArithOp::Mul => "imul",
            };
            self.load_int("rax", lsrc, bits, signed);
            self.load_int("rcx", rsrc, bits, signed);
            self.line(&format!("    {mnemonic} rax, rcx"));
            self.store_int("rax", dst, bits);
        }
        Ok(())
    }

    fn gen_bitwise(
        &mut self,
        mnemonic: &str,
        lhs: ValueId,
        rhs: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let ty = self.ty_of(lhs);
        let (bits, signed) = self.int_info(ty);
        let (lsrc, rsrc) = (self.disp[lhs.index()], self.disp[rhs.index()]);
        let dst = self.result_disp(inst)?;
        self.load_int("rax", lsrc, bits, signed);
        self.load_int("rcx", rsrc, bits, signed);
        self.line(&format!("    {mnemonic} rax, rcx"));
        self.store_int("rax", dst, bits);
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
        let (lsrc, rsrc) = (self.disp[lhs.index()], self.disp[rhs.index()]);
        if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            let suffix = if f32 { "ss" } else { "sd" };
            self.load_float("xmm0", lsrc, f32);
            self.load_float("xmm1", rsrc, f32);
            self.line(&format!("    div{suffix} xmm0, xmm1"));
            self.store_float("xmm0", dst, f32);
            return Ok(());
        }
        let (bits, signed) = self.int_info(ty);
        self.load_int("rax", lsrc, bits, signed);
        self.load_int("rcx", rsrc, bits, signed);
        if signed {
            // x86 IDIV traps on INT64_MIN / -1. Astronomy instead wraps
            // the quotient and defines the corresponding remainder as zero.
            let done = self.tmp_label("divdone");
            if bits == 64 {
                let normal = self.tmp_label("divnormal");
                self.line("    cmp rcx, -1");
                self.line(&format!("    jne {normal}"));
                self.line("    mov rdx, 0x8000000000000000");
                self.line("    cmp rax, rdx");
                self.line(&format!("    jne {normal}"));
                self.line("    xor edx, edx");
                self.line(&format!("    jmp {done}"));
                self.line(&format!("{normal}:"));
            }
            self.line("    cqo");
            self.line("    idiv rcx");
            if bits == 64 {
                self.line(&format!("{done}:"));
            }
        } else {
            self.line("    xor edx, edx");
            self.line("    div rcx");
        }
        if is_rem {
            self.line("    mov rax, rdx");
        }
        self.store_int("rax", dst, bits);
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
        self.load_int("rax", lsrc, bits, signed);
        let count_bits = self.int_bits(self.ty_of(rhs));
        self.load_int("rcx", rsrc, count_bits, false);
        let special = self.tmp_label("shovf");
        let done = self.tmp_label("shdone");
        self.line(&format!("    cmp rcx, {bits}"));
        self.line(&format!("    jae {special}"));
        if is_left {
            self.line("    shl rax, cl");
        } else if signed {
            self.line("    sar rax, cl");
        } else {
            self.line("    shr rax, cl");
        }
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{special}:"));
        if is_left || !signed {
            self.line("    xor eax, eax");
        } else {
            self.line("    sar rax, 63");
        }
        self.line(&format!("{done}:"));
        self.store_int("rax", dst, bits);
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
        let (lsrc, rsrc) = (self.disp[lhs.index()], self.disp[rhs.index()]);
        let dst = self.result_disp(inst)?;
        if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            let suffix = if f32 { "ss" } else { "sd" };
            self.load_float("xmm0", lsrc, f32);
            self.load_float("xmm1", rsrc, f32);
            self.line(&format!("    ucomi{suffix} xmm0, xmm1"));
            match op {
                CmpOp::Eq => {
                    self.line("    sete al");
                    self.line("    setnp cl");
                    self.line("    and al, cl");
                }
                CmpOp::Ne => {
                    self.line("    setne al");
                    self.line("    setp cl");
                    self.line("    or al, cl");
                }
                CmpOp::Lt => {
                    self.line("    setb al");
                    self.line("    setnp cl");
                    self.line("    and al, cl");
                }
                CmpOp::Le => {
                    self.line("    setbe al");
                    self.line("    setnp cl");
                    self.line("    and al, cl");
                }
                CmpOp::Gt => self.line("    seta al"),
                CmpOp::Ge => self.line("    setae al"),
            }
        } else {
            let (bits, signed) = self.int_info(ty);
            self.load_int("rax", lsrc, bits, signed);
            self.load_int("rcx", rsrc, bits, signed);
            self.line("    cmp rax, rcx");
            let cc = match op {
                CmpOp::Eq => "sete",
                CmpOp::Ne => "setne",
                CmpOp::Lt => {
                    if signed {
                        "setl"
                    } else {
                        "setb"
                    }
                }
                CmpOp::Le => {
                    if signed {
                        "setle"
                    } else {
                        "setbe"
                    }
                }
                CmpOp::Gt => {
                    if signed {
                        "setg"
                    } else {
                        "seta"
                    }
                }
                CmpOp::Ge => {
                    if signed {
                        "setge"
                    } else {
                        "setae"
                    }
                }
            };
            self.line(&format!("    {cc} al"));
        }
        self.store_int("rax", dst, 8);
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
        self.load_int("rax", psrc, 64, false);
        if is_aggregate(self.module.types(), ty) {
            let size = self.size(ty)?;
            self.line("    mov rsi, rax");
            let dm = self.mem(dst);
            self.line(&format!("    lea rdi, {dm}"));
            self.line(&format!("    mov rcx, {size}"));
            self.line("    cld");
            self.line("    rep movsb");
        } else if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            if f32 {
                self.line("    movss xmm0, dword [rax]");
            } else {
                self.line("    movsd xmm0, qword [rax]");
            }
            self.store_float("xmm0", dst, f32);
        } else {
            let (bits, signed) = self.int_info(ty);
            self.load_from_addr("rdx", "rax", bits, signed);
            if bits == 1 {
                self.line("    and edx, 1");
            }
            self.store_int("rdx", dst, bits);
        }
        Ok(())
    }

    fn gen_store(&mut self, pointer: ValueId, value: ValueId) -> Result<(), BackendError> {
        let psrc = self.disp[pointer.index()];
        let vsrc = self.disp[value.index()];
        let ty = self.ty_of(value);
        self.load_int("rax", psrc, 64, false);
        if is_aggregate(self.module.types(), ty) {
            let size = self.size(ty)?;
            self.line("    mov rdi, rax");
            let sm = self.mem(vsrc);
            self.line(&format!("    lea rsi, {sm}"));
            self.line(&format!("    mov rcx, {size}"));
            self.line("    cld");
            self.line("    rep movsb");
        } else if is_float(self.module.types(), ty) {
            let f32 = is_f32(self.module.types(), ty);
            self.load_float("xmm0", vsrc, f32);
            if f32 {
                self.line("    movss dword [rax], xmm0");
            } else {
                self.line("    movsd qword [rax], xmm0");
            }
        } else {
            let (bits, signed) = self.int_info(ty);
            self.load_int("rdx", vsrc, bits, signed);
            self.store_to_addr("rax", "rdx", bits);
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
        self.load_int("rax", psrc, 64, false);
        // Offsets are signed bit patterns regardless of the IR type's signedness.
        self.load_int("rcx", osrc, obits, true);
        if elem_size <= i32::MAX as u64 {
            self.line(&format!("    imul rcx, {elem_size}"));
        } else {
            // IMUL's immediate is only a sign-extended 32-bit value.
            self.line(&format!("    mov rdx, 0x{elem_size:x}"));
            self.line("    imul rcx, rdx");
        }
        self.line("    add rax, rcx");
        self.store_int("rax", dst, 64);
        Ok(())
    }

    fn gen_int_to_float(
        &mut self,
        to: TypeId,
        value: ValueId,
        inst: &Instruction,
    ) -> Result<(), BackendError> {
        let f32 = is_f32(self.module.types(), to);
        let suffix = if f32 { "ss" } else { "sd" };
        let (from_bits, from_signed) = self.int_info(self.ty_of(value));
        let src = self.disp[value.index()];
        let dst = self.result_disp(inst)?;
        if from_signed {
            self.load_int("rax", src, from_bits, true);
            self.line(&format!("    cvtsi2{suffix} xmm0, rax"));
        } else if from_bits < 64 {
            self.load_int("rax", src, from_bits, false);
            self.line(&format!("    cvtsi2{suffix} xmm0, rax"));
        } else {
            // Unsigned 64-bit cannot use cvtsi2* directly.
            let big = self.tmp_label("u2f");
            let done = self.tmp_label("u2fd");
            self.load_int("rax", src, 64, false);
            self.line("    test rax, rax");
            self.line(&format!("    js {big}"));
            self.line(&format!("    cvtsi2{suffix} xmm0, rax"));
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{big}:"));
            self.line("    mov rcx, rax");
            self.line("    shr rcx, 1");
            self.line("    and eax, 1");
            self.line("    or rcx, rax");
            self.line(&format!("    cvtsi2{suffix} xmm0, rcx"));
            self.line(&format!("    add{suffix} xmm0, xmm0"));
            self.line(&format!("{done}:"));
        }
        self.store_float("xmm0", dst, f32);
        Ok(())
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

        self.load_float("xmm0", src, from_f32);
        if from_f32 {
            self.line("    cvtss2sd xmm0, xmm0");
        }

        let zero = self.tmp_label("ftiz");
        let done = self.tmp_label("ftid");
        let sat_max = self.tmp_label("ftimax");

        self.line("    ucomisd xmm0, xmm0");
        self.line(&format!("    jp {zero}"));

        if to_signed {
            let sat_min = self.tmp_label("ftimin");
            let max_bound = (to_bits - 1) as i32;
            let bound = 2f64.powi(max_bound);
            let min_label = self.rodata.intern_f64(-bound);
            let max_label = self.rodata.intern_f64(bound);
            self.line(&format!("    movsd xmm1, qword [rel {max_label}]"));
            self.line("    ucomisd xmm0, xmm1");
            self.line(&format!("    jae {sat_max}"));
            self.line(&format!("    movsd xmm1, qword [rel {min_label}]"));
            self.line("    ucomisd xmm0, xmm1");
            self.line(&format!("    jb {sat_min}"));
            self.line("    cvttsd2si rax, xmm0");
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{sat_max}:"));
            let max_value: u64 = if to_bits == 64 {
                i64::MAX as u64
            } else {
                (1u64 << (to_bits - 1)) - 1
            };
            self.line(&format!("    mov rax, 0x{max_value:x}"));
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{sat_min}:"));
            let min_value: u64 = if to_bits == 64 {
                1u64 << 63
            } else {
                1u64 << (to_bits - 1)
            };
            self.line(&format!("    mov rax, 0x{min_value:x}"));
            self.line(&format!("    jmp {done}"));
        } else {
            // Reject negatives (`-0.5` truncates to `0`, and any x < 0
            // saturates to 0, so mapping every negative to 0 is correct).
            self.line("    xorpd xmm1, xmm1");
            self.line("    ucomisd xmm0, xmm1");
            self.line(&format!("    jb {zero}"));
            let bound = 2f64.powi(to_bits as i32);
            let bound_label = self.rodata.intern_f64(bound);
            self.line(&format!("    movsd xmm1, qword [rel {bound_label}]"));
            self.line("    ucomisd xmm0, xmm1");
            self.line(&format!("    jae {sat_max}"));
            if to_bits == 64 {
                let small = self.tmp_label("ftismall");
                let half = self.rodata.intern_f64(2f64.powi(63));
                self.line(&format!("    movsd xmm1, qword [rel {half}]"));
                self.line("    ucomisd xmm0, xmm1");
                self.line(&format!("    jb {small}"));
                self.line("    subsd xmm0, xmm1");
                self.line("    cvttsd2si rax, xmm0");
                self.line("    mov rcx, 0x8000000000000000");
                self.line("    add rax, rcx");
                self.line(&format!("    jmp {done}"));
                self.line(&format!("{small}:"));
                self.line("    cvttsd2si rax, xmm0");
                self.line(&format!("    jmp {done}"));
            } else {
                self.line("    cvttsd2si rax, xmm0");
                self.line(&format!("    jmp {done}"));
            }
            self.line(&format!("{sat_max}:"));
            let max_value: u64 = if to_bits == 64 {
                u64::MAX
            } else {
                (1u64 << to_bits) - 1
            };
            self.line(&format!("    mov rax, 0x{max_value:x}"));
            self.line(&format!("    jmp {done}"));
        }

        self.line(&format!("{zero}:"));
        self.line("    xor eax, eax");
        self.line(&format!("{done}:"));
        self.store_int("rax", dst, to_bits);
        Ok(())
    }

    fn gen_call(
        &mut self,
        callee: astronomy::FunctionId,
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
                    self.load_float(&format!("xmm{n}"), src, f32);
                }
                Loc::Stack(off) => {
                    let bits = (self.size(ty)? * 8) as u32;
                    self.load_int("rax", src, bits, false);
                    self.line(&format!("    mov qword [rsp+{off}], rax"));
                }
            }
        }

        let (callee_name, variadic) = {
            let callee_fn =
                self.module
                    .function(callee)
                    .ok_or_else(|| BackendError::InvalidModule {
                        reason: format!("unknown callee f{}", callee.as_u32()),
                    })?;
            (
                self.module
                    .symbol_name(callee_fn.symbol)
                    .unwrap_or("<invalid>")
                    .to_string(),
                callee_fn.variadic,
            )
        };

        if variadic {
            self.line(&format!("    mov al, {sse_used}"));
        }
        self.line(&format!("    call ${callee_name}"));

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
                self.store_float("xmm0", dst, f32);
            } else {
                let (bits, _) = self.int_info(rty);
                self.store_int("rax", dst, bits);
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

// ---------------------------------------------------------------------------
// Small textual helpers
// ---------------------------------------------------------------------------

fn size_kw(bytes: u64) -> &'static str {
    match bytes {
        1 => "byte",
        2 => "word",
        4 => "dword",
        _ => "qword",
    }
}

/// Maps a 64-bit register name to its 8/16/32-bit sub-register.
fn subreg(name: &str, bytes: u64) -> String {
    if let Some(rest) = name.strip_prefix('r') {
        if let Ok(n) = rest.parse::<u32>() {
            if (8..=15).contains(&n) {
                return match bytes {
                    1 => format!("r{n}b"),
                    2 => format!("r{n}w"),
                    4 => format!("r{n}d"),
                    _ => name.to_string(),
                };
            }
        }
    }
    match (name, bytes) {
        ("rax", 1) => "al".to_string(),
        ("rax", 2) => "ax".to_string(),
        ("rax", 4) => "eax".to_string(),
        ("rcx", 1) => "cl".to_string(),
        ("rcx", 2) => "cx".to_string(),
        ("rcx", 4) => "ecx".to_string(),
        ("rdx", 1) => "dl".to_string(),
        ("rdx", 2) => "dx".to_string(),
        ("rdx", 4) => "edx".to_string(),
        ("rbx", 1) => "bl".to_string(),
        ("rbx", 2) => "bx".to_string(),
        ("rbx", 4) => "ebx".to_string(),
        ("rsi", 1) => "sil".to_string(),
        ("rsi", 2) => "si".to_string(),
        ("rsi", 4) => "esi".to_string(),
        ("rdi", 1) => "dil".to_string(),
        ("rdi", 2) => "di".to_string(),
        ("rdi", 4) => "edi".to_string(),
        ("rsp", 1) => "spl".to_string(),
        ("rsp", 2) => "sp".to_string(),
        ("rsp", 4) => "esp".to_string(),
        ("rbp", 1) => "bpl".to_string(),
        ("rbp", 2) => "bp".to_string(),
        ("rbp", 4) => "ebp".to_string(),
        _ => name.to_string(),
    }
}
