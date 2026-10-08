//! Built-in x86-64 machine-code encoder.
//!
//! Turns a lowered [`Program`] into the bytes of a `.text` section plus the
//! relocations an object file needs — no external assembler involved.
//!
//! * Every instruction form of [`Inst`] has exactly one encoding, chosen to
//!   be the shortest canonical form (the same choices NASM makes with its
//!   default optimizer, which the test suite cross-checks byte for byte).
//! * Branches start in their 2-byte short form and are widened to the
//!   32-bit form when the target is out of `rel8` range. Sizing runs in
//!   passes over the whole section, the way NASM's optimizer does it: a
//!   branch is sized from offsets already reached in the current pass
//!   (backward targets) or recorded in the previous pass (forward targets,
//!   optimistically short while still unknown), until nothing moves. This
//!   makes [`crate::compile_object`] byte-identical to `nasm -f elf64` on
//!   [`crate::compile_nasm`]'s text. A final grow-only pass then guarantees every
//!   short branch is in range regardless of how the passes ended.
//! * Functions are laid out back to back. Calls between functions of the
//!   module are resolved directly; calls to declarations and references to
//!   read-only data become relocations.

use astronomy::FunctionId;

use crate::asm::{
    rodata_layout, AluOp, Cond, DataId, Fp, Gpr, Inst, Mem, Program, SymbolKind, Width,
};
use crate::error::BackendError;

/// Assembled code of a whole program.
#[derive(Debug, Clone, Default)]
pub struct Text {
    /// Contents of `.text`.
    pub bytes: Vec<u8>,
    /// `(function, start offset, size)` of every defined function.
    pub functions: Vec<(FunctionId, u64, u64)>,
    /// Relocations against `.text`, in offset order.
    pub relocs: Vec<Reloc>,
}

/// What a relocation refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocTarget {
    /// A declared (external) function; a `PLT32` call relocation.
    Function(FunctionId),
    /// The `.rodata` section; a `PC32` relocation.
    Rodata,
}

/// One relocation in `.text` (ELF RELA semantics: `S + A - P`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reloc {
    /// Offset of the 32-bit field in `.text`.
    pub offset: u64,
    /// Symbol the field refers to.
    pub target: RelocTarget,
    /// Addend.
    pub addend: i64,
}

#[derive(Debug, Clone, Copy)]
enum FixupKind {
    /// `rel32` operand of a `call`.
    Call(FunctionId),
    /// RIP-relative `disp32` to a data entry; `tail` bytes follow the field.
    Data { id: DataId, tail: u8 },
}

/// One unit of layout.
#[derive(Debug, Clone, Copy)]
enum Piece {
    /// A fixed encoding: `len` bytes at `start` in the scratch buffer, with
    /// an optional fixup (field offset within the encoding, kind).
    Bytes {
        start: usize,
        len: usize,
        fixup: Option<(usize, FixupKind)>,
    },
    /// A label definition (section-wide label number).
    Label(usize),
    /// A `jmp` (`cond: None`) or `jcc` to a section-wide label.
    Branch {
        cond: Option<Cond>,
        target: usize,
        long: bool,
    },
}

impl Piece {
    fn size(&self) -> usize {
        match *self {
            Piece::Bytes { len, .. } => len,
            Piece::Label(_) => 0,
            Piece::Branch { long: false, .. } => 2,
            Piece::Branch {
                cond: None,
                long: true,
                ..
            } => 5,
            Piece::Branch {
                cond: Some(_),
                long: true,
                ..
            } => 6,
        }
    }
}

/// Assembles every function of `program`.
pub fn assemble(program: &Program) -> Result<Text, BackendError> {
    // 1. Pre-encode everything; labels are numbered section-wide.
    let mut scratch = Vec::new();
    let mut pieces = Vec::new();
    let mut ranges = Vec::with_capacity(program.functions.len());
    let mut label_count = 0usize;
    for func in &program.functions {
        let first = pieces.len();
        let local = |l: crate::asm::Label| {
            let l = l.0 as usize;
            if l < func.labels.len() {
                Ok(label_count + l)
            } else {
                Err(undefined(l))
            }
        };
        for inst in &func.insts {
            pieces.push(match *inst {
                Inst::Label(l) => Piece::Label(local(l)?),
                Inst::Jmp(l) => Piece::Branch {
                    cond: None,
                    target: local(l)?,
                    long: false,
                },
                Inst::Jcc(cond, l) => Piece::Branch {
                    cond: Some(cond),
                    target: local(l)?,
                    long: false,
                },
                ref other => {
                    let start = scratch.len();
                    let fixup = encode(&mut scratch, other);
                    Piece::Bytes {
                        start,
                        len: scratch.len() - start,
                        fixup,
                    }
                }
            });
        }
        ranges.push((func.id, first, pieces.len()));
        label_count += func.labels.len();
    }

    // 2. Choose branch sizes.
    let (offsets, labels) = relax(&mut pieces, label_count)?;
    let total = offsets[pieces.len()];
    if total > i32::MAX as usize {
        return Err(BackendError::ObjectLimit {
            reason: format!(".text is {total} bytes; x86-64 rel32 calls reach at most 2 GiB"),
        });
    }

    // 3. Emit bytes and resolve fixups.
    let (data_offsets, _) = rodata_layout(&program.rodata);
    let mut starts: Vec<Option<usize>> = vec![None; program.symbols.len()];
    for &(id, first, _) in &ranges {
        starts[id.index()] = Some(offsets[first]);
    }
    let mut text = Text {
        bytes: Vec::with_capacity(total),
        functions: ranges
            .iter()
            .map(|&(id, first, end)| {
                (
                    id,
                    offsets[first] as u64,
                    (offsets[end] - offsets[first]) as u64,
                )
            })
            .collect(),
        relocs: Vec::new(),
    };
    for (i, piece) in pieces.iter().enumerate() {
        let at = offsets[i];
        match *piece {
            Piece::Bytes { start, len, fixup } => {
                text.bytes.extend_from_slice(&scratch[start..start + len]);
                let Some((field, kind)) = fixup else { continue };
                let field_at = at + field;
                match kind {
                    FixupKind::Call(callee)
                        if program.symbol(callee).kind == SymbolKind::Extern =>
                    {
                        text.relocs.push(Reloc {
                            offset: field_at as u64,
                            target: RelocTarget::Function(callee),
                            addend: -4,
                        });
                    }
                    FixupKind::Call(callee) => {
                        let target =
                            starts[callee.index()].ok_or_else(|| BackendError::InvalidModule {
                                reason: format!("call to f{} has no code", callee.as_u32()),
                            })?;
                        let rel = target as i64 - (field_at + 4) as i64;
                        text.bytes[field_at..field_at + 4]
                            .copy_from_slice(&(rel as i32).to_le_bytes());
                    }
                    FixupKind::Data { id, tail } => {
                        // The CPU adds the displacement to the address of
                        // the next instruction, `tail` bytes after the field.
                        text.relocs.push(Reloc {
                            offset: field_at as u64,
                            target: RelocTarget::Rodata,
                            addend: data_offsets[id.0 as usize] as i64 - 4 - tail as i64,
                        });
                    }
                }
            }
            Piece::Label(_) => {}
            Piece::Branch { cond, target, long } => {
                let rel = labels[target] as i64 - (at + piece.size()) as i64;
                match (cond, long) {
                    (None, false) => text.bytes.extend_from_slice(&[0xEB, rel as i8 as u8]),
                    (Some(c), false) => text
                        .bytes
                        .extend_from_slice(&[0x70 | c as u8, rel as i8 as u8]),
                    (None, true) => {
                        text.bytes.push(0xE9);
                        text.bytes.extend_from_slice(&(rel as i32).to_le_bytes());
                    }
                    (Some(c), true) => {
                        text.bytes.extend_from_slice(&[0x0F, 0x80 | c as u8]);
                        text.bytes.extend_from_slice(&(rel as i32).to_le_bytes());
                    }
                }
            }
        }
    }
    debug_assert_eq!(text.bytes.len(), total);
    Ok(text)
}

/// Upper bound on NASM-style sizing passes; the grow-only pass that
/// follows keeps the result correct even if this were ever reached.
const MAX_PASSES: usize = 1000;

/// Chooses short/long forms for every branch. Returns the offset of every
/// piece (plus the end) and of every label.
fn relax(
    pieces: &mut [Piece],
    label_count: usize,
) -> Result<(Vec<usize>, Vec<usize>), BackendError> {
    // NASM-style passes: `seen` holds each label's offset as of its latest
    // definition — this pass for labels already passed, the previous pass
    // for labels ahead (`None` until first defined).
    let mut previous: Vec<Option<usize>> = vec![None; label_count];
    for _ in 0..MAX_PASSES {
        let mut seen = previous.clone();
        let mut at = 0usize;
        let mut resized = false;
        for piece in pieces.iter_mut() {
            match piece {
                Piece::Label(l) => seen[*l] = Some(at),
                Piece::Branch { target, long, .. } => {
                    let short = match seen[*target] {
                        None => true,
                        Some(dest) => fits_i8(dest as i64 - (at + 2) as i64),
                    };
                    if *long == short {
                        *long = !short;
                        resized = true;
                    }
                }
                Piece::Bytes { .. } => {}
            }
            at += piece.size();
        }
        let settled = !resized && seen == previous;
        previous = seen;
        if settled {
            break;
        }
    }

    // Grow-only pass (a no-op once the passes settled): widen any short
    // branch whose target is out of rel8 range until the layout holds.
    let mut offsets = vec![0usize; pieces.len() + 1];
    let mut labels = vec![usize::MAX; label_count];
    loop {
        let mut at = 0usize;
        for (i, piece) in pieces.iter().enumerate() {
            offsets[i] = at;
            if let Piece::Label(l) = piece {
                labels[*l] = at;
            }
            at += piece.size();
        }
        offsets[pieces.len()] = at;

        let mut changed = false;
        for (i, piece) in pieces.iter_mut().enumerate() {
            if let Piece::Branch {
                target,
                long: long @ false,
                ..
            } = piece
            {
                let dest = labels[*target];
                if dest == usize::MAX {
                    return Err(undefined(*target));
                }
                if !fits_i8(dest as i64 - (offsets[i] + 2) as i64) {
                    *long = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    if let Some(Piece::Branch { target, .. }) = pieces
        .iter()
        .find(|p| matches!(p, Piece::Branch { target, .. } if labels[*target] == usize::MAX))
    {
        return Err(undefined(*target));
    }
    Ok((offsets, labels))
}

fn undefined(label: usize) -> BackendError {
    BackendError::InvalidModule {
        reason: format!("branch to undefined label {label}"),
    }
}

// ---------------------------------------------------------------------------
// Single-instruction encoding
// ---------------------------------------------------------------------------

/// The r/m operand of a ModRM-encoded instruction.
#[derive(Debug, Clone, Copy)]
enum Rm {
    Reg(u8),
    Mem(Mem),
}

/// A ModRM-form instruction: `[prefix] [REX] opcode modrm [sib] [disp]`.
struct Op<'a> {
    /// Operand-size (`66`) or mandatory SSE (`66`/`F2`/`F3`) prefix.
    prefix: Option<u8>,
    /// REX.W (64-bit operand size).
    w: bool,
    /// Emit a REX prefix even without W/R/B bits (for `spl`/`bpl`/`sil`/`dil`).
    force_rex: bool,
    opcode: &'a [u8],
    /// ModRM.reg: a register number or an opcode extension (`/digit`).
    reg: u8,
    rm: Rm,
}

/// Is `r` one of the byte registers that need REX (`spl`, `bpl`, `sil`, `dil`)?
fn byte_rex(r: u8) -> bool {
    (4..8).contains(&r)
}

fn fits_i8(v: i64) -> bool {
    i8::try_from(v).is_ok()
}

/// Emits a ModRM-form instruction. Returns the offset of a RIP-relative
/// `disp32` field, relative to the instruction start, if there is one.
fn emit_op(out: &mut Vec<u8>, op: Op<'_>) -> Option<usize> {
    let start = out.len();
    if let Some(p) = op.prefix {
        out.push(p);
    }
    let b = match op.rm {
        Rm::Reg(r) => r >= 8,
        Rm::Mem(Mem::Base { base, .. }) => base.num() >= 8,
        Rm::Mem(Mem::Data(_)) => false,
    };
    let r = op.reg >= 8;
    if op.w || r || b || op.force_rex {
        out.push(0x40 | (op.w as u8) << 3 | (r as u8) << 2 | b as u8);
    }
    out.extend_from_slice(op.opcode);
    let reg = (op.reg & 7) << 3;
    match op.rm {
        Rm::Reg(rm) => {
            out.push(0xC0 | reg | (rm & 7));
            None
        }
        Rm::Mem(Mem::Data(_)) => {
            out.push(reg | 0b101);
            let at = out.len() - start;
            out.extend_from_slice(&[0; 4]);
            Some(at)
        }
        Rm::Mem(Mem::Base { base, disp }) => {
            let low = base.num() & 7;
            // mod=00 with rbp/r13 means RIP-relative, so they need a disp8.
            let (mode, disp_len) = if disp == 0 && low != 0b101 {
                (0b00, 0)
            } else if fits_i8(disp as i64) {
                (0b01, 1)
            } else {
                (0b10, 4)
            };
            out.push(mode << 6 | reg | low);
            // rsp/r12 as a base require a SIB byte (no index).
            if low == 0b100 {
                out.push(0x24);
            }
            match disp_len {
                1 => out.push(disp as i8 as u8),
                4 => out.extend_from_slice(&disp.to_le_bytes()),
                _ => {}
            }
            None
        }
    }
}

/// Encodes a register-width-dependent ModRM instruction: picks the 8-bit or
/// full-size opcode and the `66`/REX.W prefixes from `width`.
fn emit_sized(
    out: &mut Vec<u8>,
    width: Width,
    op8: u8,
    op: u8,
    reg: u8,
    rm: Rm,
    byte_regs: &[u8],
) -> Option<usize> {
    emit_op(
        out,
        Op {
            prefix: (width == Width::W16).then_some(0x66),
            w: width == Width::W64,
            force_rex: width == Width::W8 && byte_regs.iter().any(|&r| byte_rex(r)),
            opcode: &[if width == Width::W8 { op8 } else { op }],
            reg,
            rm,
        },
    )
}

/// `opcode+reg` short forms (`push`, `mov r, imm`): optional REX.B/W.
fn emit_plus_reg(out: &mut Vec<u8>, w: bool, force_rex: bool, opcode: u8, reg: u8) {
    if w || reg >= 8 || force_rex {
        out.push(0x40 | (w as u8) << 3 | (reg >= 8) as u8);
    }
    out.push(opcode + (reg & 7));
}

fn imm_bytes(out: &mut Vec<u8>, width: Width, imm: i64) {
    match width {
        Width::W8 => out.push(imm as u8),
        Width::W16 => out.extend_from_slice(&(imm as u16).to_le_bytes()),
        _ => out.extend_from_slice(&(imm as u32).to_le_bytes()),
    }
}

fn sse_prefix(fp: Fp) -> u8 {
    match fp {
        Fp::Single => 0xF3,
        Fp::Double => 0xF2,
    }
}

/// Group-1 ALU `op r/m, imm`: the sign-extended `imm8` form when the value
/// fits, else the short accumulator form for `al/ax/eax/rax`, else `imm32`.
fn emit_alu_imm(out: &mut Vec<u8>, op: AluOp, width: Width, rm: Rm, imm: i32) -> Option<usize> {
    let digit = op as u8;
    let reg_operand = match rm {
        Rm::Reg(r) => Some(r),
        Rm::Mem(_) => None,
    };
    let byte_regs = reg_operand.as_slice();
    if width == Width::W8 && reg_operand == Some(0) {
        out.extend_from_slice(&[digit << 3 | 0x04, imm as u8]);
        return None;
    }
    if width == Width::W8 {
        let field = emit_sized(out, width, 0x80, 0x80, digit, rm, byte_regs);
        out.push(imm as u8);
        return field;
    }
    if fits_i8(imm as i64) {
        let field = emit_sized(out, width, 0x83, 0x83, digit, rm, &[]);
        out.push(imm as i8 as u8);
        return field;
    }
    if let Rm::Reg(0) = rm {
        if width == Width::W16 {
            out.push(0x66);
        }
        if width == Width::W64 {
            out.push(0x48);
        }
        out.push(digit << 3 | 0x05);
        imm_bytes(out, width, imm as i64);
        return None;
    }
    let field = emit_sized(out, width, 0x81, 0x81, digit, rm, &[]);
    imm_bytes(out, width, imm as i64);
    field
}

fn r(g: Gpr) -> u8 {
    g.reg.num()
}

/// Appends the encoding of one non-branch, non-label instruction. Returns
/// its fixup (field offset within the instruction and kind), if any.
fn encode(out: &mut Vec<u8>, inst: &Inst) -> Option<(usize, FixupKind)> {
    let start = out.len();
    let data_ref = |field: Option<usize>, mem: Mem, out: &Vec<u8>| match (field, mem) {
        (Some(at), Mem::Data(id)) => Some((
            at,
            FixupKind::Data {
                id,
                tail: (out.len() - start - at - 4) as u8,
            },
        )),
        _ => None,
    };
    match *inst {
        Inst::Label(_) | Inst::Jmp(_) | Inst::Jcc(..) => {
            unreachable!("labels and branches are laid out by the relaxation pass")
        }
        Inst::Push(reg) => emit_plus_reg(out, false, false, 0x50, reg.num()),
        Inst::MovRR { dst, src } => {
            emit_sized(
                out,
                dst.width,
                0x88,
                0x89,
                r(src),
                Rm::Reg(r(dst)),
                &[r(src), r(dst)],
            );
        }
        Inst::MovRI { dst, imm, .. } => {
            let reg = r(dst);
            match dst.width {
                Width::W8 => {
                    emit_plus_reg(out, false, byte_rex(reg), 0xB0, reg);
                    out.push(imm as u8);
                }
                Width::W16 => {
                    out.push(0x66);
                    emit_plus_reg(out, false, false, 0xB8, reg);
                    out.extend_from_slice(&(imm as u16).to_le_bytes());
                }
                // Writing a 32-bit register zero-extends into the full
                // 64-bit register, so small 64-bit values use it too.
                Width::W32 => {
                    emit_plus_reg(out, false, false, 0xB8, reg);
                    out.extend_from_slice(&(imm as u32).to_le_bytes());
                }
                Width::W64 if imm <= u32::MAX as u64 => {
                    emit_plus_reg(out, false, false, 0xB8, reg);
                    out.extend_from_slice(&(imm as u32).to_le_bytes());
                }
                Width::W64 if i32::try_from(imm as i64).is_ok() => {
                    emit_op(
                        out,
                        Op {
                            prefix: None,
                            w: true,
                            force_rex: false,
                            opcode: &[0xC7],
                            reg: 0,
                            rm: Rm::Reg(reg),
                        },
                    );
                    out.extend_from_slice(&(imm as u32).to_le_bytes());
                }
                Width::W64 => {
                    emit_plus_reg(out, true, false, 0xB8, reg);
                    out.extend_from_slice(&imm.to_le_bytes());
                }
            }
        }
        Inst::Load { dst, mem } => {
            let field = emit_sized(out, dst.width, 0x8A, 0x8B, r(dst), Rm::Mem(mem), &[r(dst)]);
            return data_ref(field, mem, out);
        }
        Inst::LoadExt {
            dst,
            src,
            signed,
            mem,
        } => {
            let (w, opcode): (bool, &[u8]) = match (src, signed) {
                (Width::W64, _) => (true, &[0x8B]),
                (Width::W32, true) => (true, &[0x63]),
                (Width::W32, false) => (false, &[0x8B]),
                (Width::W16, true) => (true, &[0x0F, 0xBF]),
                (Width::W16, false) => (true, &[0x0F, 0xB7]),
                (Width::W8, true) => (true, &[0x0F, 0xBE]),
                (Width::W8, false) => (true, &[0x0F, 0xB6]),
            };
            let field = emit_op(
                out,
                Op {
                    prefix: None,
                    w,
                    force_rex: false,
                    opcode,
                    reg: dst.num(),
                    rm: Rm::Mem(mem),
                },
            );
            return data_ref(field, mem, out);
        }
        Inst::Store { mem, src } => {
            let field = emit_sized(out, src.width, 0x88, 0x89, r(src), Rm::Mem(mem), &[r(src)]);
            return data_ref(field, mem, out);
        }
        Inst::Lea { dst, mem } => {
            let field = emit_op(
                out,
                Op {
                    prefix: None,
                    w: true,
                    force_rex: false,
                    opcode: &[0x8D],
                    reg: dst.num(),
                    rm: Rm::Mem(mem),
                },
            );
            return data_ref(field, mem, out);
        }
        Inst::AluRR { op, dst, src } => {
            let base = (op as u8) << 3;
            emit_sized(
                out,
                dst.width,
                base,
                base | 1,
                r(src),
                Rm::Reg(r(dst)),
                &[r(src), r(dst)],
            );
        }
        Inst::AluRI { op, dst, imm } => {
            emit_alu_imm(out, op, dst.width, Rm::Reg(r(dst)), imm);
        }
        Inst::AluMI {
            op,
            width,
            mem,
            imm,
        } => {
            let field = emit_alu_imm(out, op, width, Rm::Mem(mem), imm);
            return data_ref(field, mem, out);
        }
        Inst::Test { a, b } => {
            emit_sized(out, a.width, 0x84, 0x85, r(b), Rm::Reg(r(a)), &[r(a), r(b)]);
        }
        Inst::ImulRR { dst, src } => {
            emit_op(
                out,
                Op {
                    prefix: None,
                    w: true,
                    force_rex: false,
                    opcode: &[0x0F, 0xAF],
                    reg: dst.num(),
                    rm: Rm::Reg(src.num()),
                },
            );
        }
        Inst::ImulRI { dst, imm } => {
            let short = fits_i8(imm as i64);
            emit_op(
                out,
                Op {
                    prefix: None,
                    w: true,
                    force_rex: false,
                    opcode: &[if short { 0x6B } else { 0x69 }],
                    reg: dst.num(),
                    rm: Rm::Reg(dst.num()),
                },
            );
            if short {
                out.push(imm as i8 as u8);
            } else {
                out.extend_from_slice(&imm.to_le_bytes());
            }
        }
        Inst::Div { signed, src } => {
            emit_op(
                out,
                Op {
                    prefix: None,
                    w: true,
                    force_rex: false,
                    opcode: &[0xF7],
                    reg: if signed { 7 } else { 6 },
                    rm: Rm::Reg(src.num()),
                },
            );
        }
        Inst::ShiftCl { op, dst } => {
            emit_op(
                out,
                Op {
                    prefix: None,
                    w: true,
                    force_rex: false,
                    opcode: &[0xD3],
                    reg: op as u8,
                    rm: Rm::Reg(dst.num()),
                },
            );
        }
        Inst::ShiftRI { op, dst, imm } => {
            emit_op(
                out,
                Op {
                    prefix: None,
                    w: true,
                    force_rex: false,
                    opcode: &[if imm == 1 { 0xD1 } else { 0xC1 }],
                    reg: op as u8,
                    rm: Rm::Reg(dst.num()),
                },
            );
            if imm != 1 {
                out.push(imm);
            }
        }
        Inst::Setcc { cond, dst } => {
            emit_op(
                out,
                Op {
                    prefix: None,
                    w: false,
                    force_rex: byte_rex(dst.num()),
                    opcode: &[0x0F, 0x90 | cond as u8],
                    reg: 0,
                    rm: Rm::Reg(dst.num()),
                },
            );
        }
        Inst::Cqo => out.extend_from_slice(&[0x48, 0x99]),
        Inst::Cld => out.push(0xFC),
        Inst::RepMovsb => out.extend_from_slice(&[0xF3, 0xA4]),
        Inst::Leave => out.push(0xC9),
        Inst::Ret => out.push(0xC3),
        Inst::Ud2 => out.extend_from_slice(&[0x0F, 0x0B]),
        Inst::Call(callee) => {
            out.extend_from_slice(&[0xE8, 0, 0, 0, 0]);
            return Some((1, FixupKind::Call(callee)));
        }
        Inst::FLoad { fp, dst, mem } => {
            let field = emit_op(
                out,
                Op {
                    prefix: Some(sse_prefix(fp)),
                    w: false,
                    force_rex: false,
                    opcode: &[0x0F, 0x10],
                    reg: dst.0,
                    rm: Rm::Mem(mem),
                },
            );
            return data_ref(field, mem, out);
        }
        Inst::FStore { fp, mem, src } => {
            let field = emit_op(
                out,
                Op {
                    prefix: Some(sse_prefix(fp)),
                    w: false,
                    force_rex: false,
                    opcode: &[0x0F, 0x11],
                    reg: src.0,
                    rm: Rm::Mem(mem),
                },
            );
            return data_ref(field, mem, out);
        }
        Inst::FArith { op, fp, dst, src } => {
            emit_op(
                out,
                Op {
                    prefix: Some(sse_prefix(fp)),
                    w: false,
                    force_rex: false,
                    opcode: &[0x0F, op as u8],
                    reg: dst.0,
                    rm: Rm::Reg(src.0),
                },
            );
        }
        Inst::Ucomi { fp, a, b } => {
            emit_op(
                out,
                Op {
                    prefix: (fp == Fp::Double).then_some(0x66),
                    w: false,
                    force_rex: false,
                    opcode: &[0x0F, 0x2E],
                    reg: a.0,
                    rm: Rm::Reg(b.0),
                },
            );
        }
        Inst::CvtSi2F { fp, dst, src } => {
            emit_op(
                out,
                Op {
                    prefix: Some(sse_prefix(fp)),
                    w: true,
                    force_rex: false,
                    opcode: &[0x0F, 0x2A],
                    reg: dst.0,
                    rm: Rm::Reg(src.num()),
                },
            );
        }
        Inst::Cvtss2sd { dst, src } => {
            emit_op(
                out,
                Op {
                    prefix: Some(0xF3),
                    w: false,
                    force_rex: false,
                    opcode: &[0x0F, 0x5A],
                    reg: dst.0,
                    rm: Rm::Reg(src.0),
                },
            );
        }
        Inst::Cvttsd2si { dst, src } => {
            emit_op(
                out,
                Op {
                    prefix: Some(0xF2),
                    w: true,
                    force_rex: false,
                    opcode: &[0x0F, 0x2C],
                    reg: dst.num(),
                    rm: Rm::Reg(src.0),
                },
            );
        }
        Inst::Xorpd { dst, src } => {
            emit_op(
                out,
                Op {
                    prefix: Some(0x66),
                    w: false,
                    force_rex: false,
                    opcode: &[0x0F, 0x57],
                    reg: dst.0,
                    rm: Rm::Reg(src.0),
                },
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::{ImmStyle, Reg, ShiftOp, SseOp, Xmm};

    fn enc(inst: Inst) -> Vec<u8> {
        let mut out = Vec::new();
        encode(&mut out, &inst);
        out
    }

    #[test]
    fn integer_moves() {
        let rax = Reg::Rax.q();
        let mov = |dst, imm| Inst::MovRI {
            dst,
            imm,
            style: ImmStyle::Hex,
        };
        assert_eq!(enc(mov(rax, 5)), [0xB8, 5, 0, 0, 0]);
        assert_eq!(enc(mov(rax, 0x8000_0000)), [0xB8, 0, 0, 0, 0x80]);
        assert_eq!(
            enc(mov(rax, u64::MAX)),
            [0x48, 0xC7, 0xC0, 0xFF, 0xFF, 0xFF, 0xFF]
        );
        assert_eq!(
            enc(mov(rax, 0x1_2345_6789)),
            [0x48, 0xB8, 0x89, 0x67, 0x45, 0x23, 0x01, 0, 0, 0]
        );
        assert_eq!(enc(mov(Reg::R9.q(), 5)), [0x41, 0xB9, 5, 0, 0, 0]);
        assert_eq!(enc(mov(Reg::Rax.at(Width::W8), 3)), [0xB0, 3]);
        assert_eq!(
            enc(Inst::MovRR {
                dst: Reg::Rbp.q(),
                src: Reg::Rsp.q()
            }),
            [0x48, 0x89, 0xE5]
        );
        assert_eq!(enc(Inst::Push(Reg::Rbp)), [0x55]);
    }

    #[test]
    fn memory_operands() {
        let load = |dst, mem| Inst::Load { dst, mem };
        assert_eq!(
            enc(load(Reg::Rax.q(), Mem::at(Reg::Rbp, -8))),
            [0x48, 0x8B, 0x45, 0xF8]
        );
        assert_eq!(
            enc(load(Reg::Rax.q(), Mem::at(Reg::Rbp, -200))),
            [0x48, 0x8B, 0x85, 0x38, 0xFF, 0xFF, 0xFF]
        );
        // rbp needs a disp8 even for zero; rsp needs a SIB byte.
        assert_eq!(
            enc(load(Reg::Rax.q(), Mem::at(Reg::Rbp, 0))),
            [0x48, 0x8B, 0x45, 0x00]
        );
        assert_eq!(
            enc(Inst::Store {
                mem: Mem::at(Reg::Rsp, 0),
                src: Reg::Rax.q()
            }),
            [0x48, 0x89, 0x04, 0x24]
        );
        assert_eq!(
            enc(Inst::Store {
                mem: Mem::at(Reg::Rsp, 8),
                src: Reg::Rax.q()
            }),
            [0x48, 0x89, 0x44, 0x24, 0x08]
        );
        // `sil` needs a bare REX prefix; 16-bit stores need `66`.
        assert_eq!(
            enc(Inst::Store {
                mem: Mem::at(Reg::Rbp, 0),
                src: Reg::Rsi.at(Width::W8)
            }),
            [0x40, 0x88, 0x75, 0x00]
        );
        assert_eq!(
            enc(Inst::Store {
                mem: Mem::at(Reg::Rax, 0),
                src: Reg::Rdx.at(Width::W16)
            }),
            [0x66, 0x89, 0x10]
        );
        assert_eq!(
            enc(Inst::Store {
                mem: Mem::at(Reg::Rbp, -4),
                src: Reg::R9.at(Width::W8)
            }),
            [0x44, 0x88, 0x4D, 0xFC]
        );
        assert_eq!(
            enc(Inst::LoadExt {
                dst: Reg::Rax,
                src: Width::W8,
                signed: false,
                mem: Mem::at(Reg::Rbp, -8)
            }),
            [0x48, 0x0F, 0xB6, 0x45, 0xF8]
        );
        assert_eq!(
            enc(Inst::LoadExt {
                dst: Reg::Rcx,
                src: Width::W32,
                signed: true,
                mem: Mem::at(Reg::Rbp, -8)
            }),
            [0x48, 0x63, 0x4D, 0xF8]
        );
    }

    #[test]
    fn rip_relative_data_fixups() {
        let mut out = Vec::new();
        let fixup = encode(
            &mut out,
            &Inst::Lea {
                dst: Reg::Rax,
                mem: Mem::Data(DataId(3)),
            },
        );
        assert_eq!(out, [0x48, 0x8D, 0x05, 0, 0, 0, 0]);
        assert!(matches!(
            fixup,
            Some((
                3,
                FixupKind::Data {
                    id: DataId(3),
                    tail: 0
                }
            ))
        ));

        let mut out = Vec::new();
        encode(
            &mut out,
            &Inst::FLoad {
                fp: Fp::Double,
                dst: Xmm(1),
                mem: Mem::Data(DataId(0)),
            },
        );
        assert_eq!(out, [0xF2, 0x0F, 0x10, 0x0D, 0, 0, 0, 0]);

        // An immediate after the displacement shifts the addend.
        let mut out = Vec::new();
        let fixup = encode(
            &mut out,
            &Inst::AluMI {
                op: AluOp::Cmp,
                width: Width::W8,
                mem: Mem::Data(DataId(0)),
                imm: 0,
            },
        );
        assert!(matches!(fixup, Some((2, FixupKind::Data { tail: 1, .. }))));
    }

    #[test]
    fn alu_forms() {
        let alu = |op, dst, imm| Inst::AluRI { op, dst, imm };
        assert_eq!(
            enc(alu(AluOp::Sub, Reg::Rsp.q(), 16)),
            [0x48, 0x83, 0xEC, 0x10]
        );
        assert_eq!(
            enc(alu(AluOp::Sub, Reg::Rsp.q(), 200)),
            [0x48, 0x81, 0xEC, 0xC8, 0, 0, 0]
        );
        assert_eq!(
            enc(alu(AluOp::Add, Reg::Rax.q(), 1000)),
            [0x48, 0x05, 0xE8, 0x03, 0, 0]
        );
        assert_eq!(
            enc(alu(AluOp::Cmp, Reg::Rcx.q(), -1)),
            [0x48, 0x83, 0xF9, 0xFF]
        );
        assert_eq!(
            enc(alu(AluOp::And, Reg::Rax.at(Width::W32), 1)),
            [0x83, 0xE0, 0x01]
        );
        assert_eq!(
            enc(Inst::AluRR {
                op: AluOp::And,
                dst: Reg::Rax.at(Width::W8),
                src: Reg::Rcx.at(Width::W8)
            }),
            [0x20, 0xC8]
        );
        assert_eq!(
            enc(Inst::AluRR {
                op: AluOp::Xor,
                dst: Reg::Rdx.at(Width::W32),
                src: Reg::Rdx.at(Width::W32)
            }),
            [0x31, 0xD2]
        );
        assert_eq!(
            enc(Inst::AluMI {
                op: AluOp::Cmp,
                width: Width::W8,
                mem: Mem::at(Reg::Rbp, -8),
                imm: 0
            }),
            [0x80, 0x7D, 0xF8, 0x00]
        );
        assert_eq!(
            enc(Inst::ImulRI {
                dst: Reg::Rcx,
                imm: 8
            }),
            [0x48, 0x6B, 0xC9, 0x08]
        );
        assert_eq!(
            enc(Inst::ImulRI {
                dst: Reg::Rcx,
                imm: 1000
            }),
            [0x48, 0x69, 0xC9, 0xE8, 0x03, 0, 0]
        );
        assert_eq!(
            enc(Inst::ShiftRI {
                op: ShiftOp::Shr,
                dst: Reg::Rcx,
                imm: 1
            }),
            [0x48, 0xD1, 0xE9]
        );
        assert_eq!(
            enc(Inst::ShiftRI {
                op: ShiftOp::Sar,
                dst: Reg::Rax,
                imm: 63
            }),
            [0x48, 0xC1, 0xF8, 0x3F]
        );
        assert_eq!(
            enc(Inst::Setcc {
                cond: Cond::Np,
                dst: Reg::Rcx
            }),
            [0x0F, 0x9B, 0xC1]
        );
    }

    #[test]
    fn sse_forms() {
        assert_eq!(
            enc(Inst::FArith {
                op: SseOp::Add,
                fp: Fp::Single,
                dst: Xmm(0),
                src: Xmm(1)
            }),
            [0xF3, 0x0F, 0x58, 0xC1]
        );
        assert_eq!(
            enc(Inst::Ucomi {
                fp: Fp::Double,
                a: Xmm(0),
                b: Xmm(1)
            }),
            [0x66, 0x0F, 0x2E, 0xC1]
        );
        assert_eq!(
            enc(Inst::CvtSi2F {
                fp: Fp::Double,
                dst: Xmm(0),
                src: Reg::Rcx
            }),
            [0xF2, 0x48, 0x0F, 0x2A, 0xC1]
        );
        assert_eq!(
            enc(Inst::Cvttsd2si {
                dst: Reg::Rax,
                src: Xmm(0)
            }),
            [0xF2, 0x48, 0x0F, 0x2C, 0xC0]
        );
        assert_eq!(
            enc(Inst::FStore {
                fp: Fp::Single,
                mem: Mem::at(Reg::Rbp, -4),
                src: Xmm(7)
            }),
            [0xF3, 0x0F, 0x11, 0x7D, 0xFC]
        );
    }

    fn assemble_one(insts: Vec<Inst>, labels: usize) -> Vec<u8> {
        use crate::asm::{FuncCode, FuncSymbol, LabelKind};
        let program = Program {
            symbols: vec![FuncSymbol {
                name: "f".into(),
                kind: SymbolKind::Global,
            }],
            functions: vec![FuncCode {
                id: FunctionId::new(0),
                labels: (0..labels)
                    .map(|l| (format!("l{l}"), LabelKind::Local))
                    .collect(),
                insts,
            }],
            rodata: Vec::new(),
        };
        assemble(&program).unwrap().bytes
    }

    #[test]
    fn branches_relax_only_when_needed() {
        use crate::asm::Label;
        // A backward jump within range stays short.
        let bytes = assemble_one(vec![Inst::Label(Label(0)), Inst::Jmp(Label(0))], 1);
        assert_eq!(bytes, [0xEB, 0xFE]);

        // 200 bytes of padding push the target out of rel8 range.
        let mut insts = vec![Inst::Jcc(Cond::E, Label(0))];
        insts.extend((0..200).map(|_| Inst::Ret));
        insts.push(Inst::Label(Label(0)));
        let bytes = assemble_one(insts, 1);
        assert_eq!(&bytes[..6], &[0x0F, 0x84, 200, 0, 0, 0]);
        assert_eq!(bytes.len(), 206);

        // Exactly at the boundary: +127 forward and -128 backward fit.
        let mut insts = vec![Inst::Jmp(Label(0))];
        insts.extend((0..127).map(|_| Inst::Ret));
        insts.push(Inst::Label(Label(0)));
        assert_eq!(&assemble_one(insts, 1)[..2], &[0xEB, 0x7F]);
        let mut insts = vec![Inst::Label(Label(0))];
        insts.extend((0..126).map(|_| Inst::Ret));
        insts.push(Inst::Jmp(Label(0)));
        assert_eq!(&assemble_one(insts, 1)[126..], &[0xEB, 0x80]);
    }

    #[test]
    fn undefined_labels_are_errors() {
        use crate::asm::{FuncCode, FuncSymbol, Label, LabelKind};
        let program = Program {
            symbols: vec![FuncSymbol {
                name: "f".into(),
                kind: SymbolKind::Global,
            }],
            functions: vec![FuncCode {
                id: FunctionId::new(0),
                labels: vec![("l0".into(), LabelKind::Local)],
                insts: vec![Inst::Jmp(Label(0))],
            }],
            rodata: Vec::new(),
        };
        assert_eq!(assemble(&program).unwrap_err().code(), "A-OBJ-004");
    }
}
