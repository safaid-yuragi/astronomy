//! NASM source text emitter for lowered [`Program`]s.

use std::fmt::Write as _;

use astronomy_x86::asm::{
    Fp, Gpr, ImmStyle, Inst, Label, LabelKind, Mem, Program, Reg, SymbolKind, Width, Xmm,
};

/// Renders a lowered program as NASM source.
pub fn print(program: &Program) -> String {
    let mut out = String::new();
    out.push_str("bits 64\ndefault rel\n\n");
    for symbol in &program.symbols {
        match symbol.kind {
            SymbolKind::Extern => {
                let _ = writeln!(out, "extern ${}", symbol.name);
            }
            SymbolKind::Global => {
                let _ = writeln!(out, "global ${}", symbol.name);
            }
            SymbolKind::Local => {}
        }
    }
    out.push_str("\nsection .text\n");

    for func in &program.functions {
        // `$` forces NASM to treat even register/directive names as symbols.
        let _ = writeln!(out, "${}:", program.symbol(func.id).name);
        for inst in &func.insts {
            print_inst(&mut out, program, &func.labels, inst);
        }
    }

    if !program.rodata.is_empty() {
        out.push_str("\nsection .rodata\n");
        for (i, bytes) in program.rodata.iter().enumerate() {
            let _ = writeln!(out, "arn.data.{i}:");
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
    out
}

fn gpr(r: Gpr) -> &'static str {
    r.reg.name(r.width)
}

fn q(r: Reg) -> &'static str {
    r.name(Width::W64)
}

fn xmm(x: Xmm) -> String {
    format!("xmm{}", x.0)
}

fn mem(m: Mem) -> String {
    match m {
        // Frame and stack references always show their displacement.
        Mem::Base {
            base: base @ (Reg::Rbp | Reg::Rsp),
            disp,
        } => format!("[{}{disp:+}]", q(base)),
        Mem::Base { base, disp: 0 } => format!("[{}]", q(base)),
        Mem::Base { base, disp } => format!("[{}{disp:+}]", q(base)),
        Mem::Data(id) => format!("[rel arn.data.{}]", id.0),
    }
}

fn fp_width(fp: Fp) -> &'static str {
    match fp {
        Fp::Single => "dword",
        Fp::Double => "qword",
    }
}

fn print_inst(out: &mut String, program: &Program, labels: &[(String, LabelKind)], inst: &Inst) {
    let label = |l: Label| labels[l.0 as usize].0.as_str();
    let text = match inst {
        Inst::Label(l) => {
            let (name, kind) = &labels[l.0 as usize];
            if *kind == LabelKind::Block {
                out.push('\n');
            }
            let _ = writeln!(out, "{name}:");
            return;
        }
        Inst::Push(r) => format!("push {}", q(*r)),
        Inst::MovRR { dst, src } => format!("mov {}, {}", gpr(*dst), gpr(*src)),
        Inst::MovRI { dst, imm, style } => match style {
            ImmStyle::Dec => format!("mov {}, {imm}", gpr(*dst)),
            ImmStyle::Hex => format!("mov {}, 0x{imm:x}", gpr(*dst)),
        },
        Inst::Load { dst, mem: m } => {
            format!("mov {}, {} {}", gpr(*dst), dst.width.keyword(), mem(*m))
        }
        Inst::LoadExt {
            dst,
            src,
            signed,
            mem: m,
        } => match (src, signed) {
            (Width::W32, true) => format!("movsxd {}, dword {}", q(*dst), mem(*m)),
            // A 32-bit `mov` zero-extends into the full register.
            (Width::W32, false) => format!("mov {}, dword {}", dst.name(Width::W32), mem(*m)),
            (Width::W64, _) => format!("mov {}, qword {}", q(*dst), mem(*m)),
            (w, true) => format!("movsx {}, {} {}", q(*dst), w.keyword(), mem(*m)),
            (w, false) => format!("movzx {}, {} {}", q(*dst), w.keyword(), mem(*m)),
        },
        Inst::Store { mem: m, src } => {
            format!("mov {} {}, {}", src.width.keyword(), mem(*m), gpr(*src))
        }
        Inst::Lea { dst, mem: m } => format!("lea {}, {}", q(*dst), mem(*m)),
        Inst::AluRR { op, dst, src } => format!("{} {}, {}", op.mnemonic(), gpr(*dst), gpr(*src)),
        Inst::AluRI { op, dst, imm } => format!("{} {}, {imm}", op.mnemonic(), gpr(*dst)),
        Inst::AluMI {
            op,
            width,
            mem: m,
            imm,
        } => format!("{} {} {}, {imm}", op.mnemonic(), width.keyword(), mem(*m)),
        Inst::Test { a, b } => format!("test {}, {}", gpr(*a), gpr(*b)),
        Inst::ImulRR { dst, src } => format!("imul {}, {}", q(*dst), q(*src)),
        Inst::ImulRI { dst, imm } => format!("imul {}, {imm}", q(*dst)),
        Inst::Div { signed, src } => {
            format!("{} {}", if *signed { "idiv" } else { "div" }, q(*src))
        }
        Inst::ShiftCl { op, dst } => format!("{} {}, cl", op.mnemonic(), q(*dst)),
        Inst::ShiftRI { op, dst, imm } => format!("{} {}, {imm}", op.mnemonic(), q(*dst)),
        Inst::Setcc { cond, dst } => format!("set{} {}", cond.suffix(), dst.name(Width::W8)),
        Inst::Cqo => "cqo".to_string(),
        Inst::Cld => "cld".to_string(),
        Inst::RepMovsb => "rep movsb".to_string(),
        Inst::Leave => "leave".to_string(),
        Inst::Ret => "ret".to_string(),
        Inst::Ud2 => "ud2".to_string(),
        Inst::Jmp(l) => format!("jmp {}", label(*l)),
        Inst::Jcc(cond, l) => format!("j{} {}", cond.suffix(), label(*l)),
        Inst::Call(id) => format!("call ${}", program.symbol(*id).name),
        Inst::FLoad { fp, dst, mem: m } => format!(
            "mov{} {}, {} {}",
            fp.suffix(),
            xmm(*dst),
            fp_width(*fp),
            mem(*m)
        ),
        Inst::FStore { fp, mem: m, src } => format!(
            "mov{} {} {}, {}",
            fp.suffix(),
            fp_width(*fp),
            mem(*m),
            xmm(*src)
        ),
        Inst::FArith { op, fp, dst, src } => {
            format!("{}{} {}, {}", op.stem(), fp.suffix(), xmm(*dst), xmm(*src))
        }
        Inst::Ucomi { fp, a, b } => format!("ucomi{} {}, {}", fp.suffix(), xmm(*a), xmm(*b)),
        Inst::CvtSi2F { fp, dst, src } => {
            format!("cvtsi2{} {}, {}", fp.suffix(), xmm(*dst), q(*src))
        }
        Inst::Cvtss2sd { dst, src } => format!("cvtss2sd {}, {}", xmm(*dst), xmm(*src)),
        Inst::Cvttsd2si { dst, src } => format!("cvttsd2si {}, {}", q(*dst), xmm(*src)),
        Inst::Xorpd { dst, src } => format!("xorpd {}, {}", xmm(*dst), xmm(*src)),
    };
    out.push_str("    ");
    out.push_str(&text);
    out.push('\n');
}
