//! Exhaustive encoder cross-check against NASM.
//!
//! Builds functions containing every instruction form of the model over
//! the full register file, every addressing shape (each base register with
//! no, 8-bit and 32-bit displacements, plus RIP-relative data) and a spread
//! of immediates, plus randomized branch-dense functions, prints them with
//! the NASM emitter, assembles them with `nasm -f elf64`, and requires
//! NASM's `.text` to equal the built-in encoder's byte for byte. Skips when
//! `nasm` is not installed.

use std::process::Command;

use astronomy::FunctionId;

use crate::asm::{
    AluOp, Cond, DataId, Fp, FuncCode, FuncSymbol, ImmStyle, Inst, Label, LabelKind, Mem, Program,
    Reg, ShiftOp, SseOp, SymbolKind, Width, Xmm,
};

const REGS: [Reg; 16] = [
    Reg::Rax,
    Reg::Rcx,
    Reg::Rdx,
    Reg::Rbx,
    Reg::Rsp,
    Reg::Rbp,
    Reg::Rsi,
    Reg::Rdi,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
    Reg::R12,
    Reg::R13,
    Reg::R14,
    Reg::R15,
];
const WIDTHS: [Width; 4] = [Width::W8, Width::W16, Width::W32, Width::W64];
const ALU: [AluOp; 6] = [
    AluOp::Add,
    AluOp::Or,
    AluOp::And,
    AluOp::Sub,
    AluOp::Xor,
    AluOp::Cmp,
];
const CONDS: [Cond; 13] = [
    Cond::B,
    Cond::Ae,
    Cond::E,
    Cond::Ne,
    Cond::Be,
    Cond::A,
    Cond::S,
    Cond::P,
    Cond::Np,
    Cond::L,
    Cond::Ge,
    Cond::Le,
    Cond::G,
];
const FPS: [Fp; 2] = [Fp::Single, Fp::Double];

/// Register-based addressing shapes (RIP-relative data is added separately,
/// see [`every_form`]).
fn mems() -> Vec<Mem> {
    let mut mems = Vec::new();
    for base in REGS {
        for disp in [0, 8, -8, 127, -128, 128, -129, 0x1234_5678, i32::MIN] {
            mems.push(Mem::at(base, disp));
        }
    }
    mems
}

fn imms(width: Width) -> &'static [i32] {
    match width {
        Width::W8 => &[0, 1, -1, 127, -128],
        Width::W16 => &[0, 1, -1, 127, -128, 128, -129, 32767, -32768],
        _ => &[0, 1, -1, 127, -128, 128, -129, 1000, i32::MAX, i32::MIN],
    }
}

/// Instructions using memory operand `mem`.
fn memory_forms(v: &mut Vec<Inst>, mem: Mem) {
    for r in [Reg::Rax, Reg::Rsp, Reg::Rsi, Reg::R8, Reg::R15] {
        v.push(Inst::Lea { dst: r, mem });
        for w in WIDTHS {
            v.push(Inst::Load { dst: r.at(w), mem });
            v.push(Inst::Store { mem, src: r.at(w) });
        }
        for (src, signed) in [
            (Width::W8, false),
            (Width::W8, true),
            (Width::W16, false),
            (Width::W16, true),
            (Width::W32, false),
            (Width::W32, true),
            (Width::W64, false),
        ] {
            v.push(Inst::LoadExt {
                dst: r,
                src,
                signed,
                mem,
            });
        }
    }
    for w in WIDTHS {
        for op in [AluOp::Cmp, AluOp::Add] {
            for &imm in imms(w) {
                v.push(Inst::AluMI {
                    op,
                    width: w,
                    mem,
                    imm,
                });
            }
        }
    }
    for x in [Xmm(0), Xmm(7), Xmm(8), Xmm(15)] {
        for fp in FPS {
            v.push(Inst::FLoad { fp, dst: x, mem });
            v.push(Inst::FStore { fp, mem, src: x });
        }
    }
}

fn every_form() -> Vec<Inst> {
    let mems = mems();
    let xmms: Vec<Xmm> = (0..16).map(Xmm).collect();
    let mut v = vec![Inst::Label(Label(0))];

    for r in REGS {
        v.push(Inst::Push(r));
        v.push(Inst::Lea {
            dst: r,
            mem: Mem::at(Reg::Rbp, -8),
        });
        v.push(Inst::ImulRI { dst: r, imm: 7 });
        v.push(Inst::ImulRI {
            dst: r,
            imm: 100_000,
        });
        for signed in [false, true] {
            v.push(Inst::Div { signed, src: r });
        }
        for op in [ShiftOp::Shl, ShiftOp::Shr, ShiftOp::Sar] {
            v.push(Inst::ShiftCl { op, dst: r });
            for imm in [1, 2, 63] {
                v.push(Inst::ShiftRI { op, dst: r, imm });
            }
        }
        for cond in CONDS {
            v.push(Inst::Setcc { cond, dst: r });
        }
        for s in REGS {
            v.push(Inst::ImulRR { dst: r, src: s });
            for w in WIDTHS {
                v.push(Inst::MovRR {
                    dst: r.at(w),
                    src: s.at(w),
                });
                v.push(Inst::Test {
                    a: r.at(w),
                    b: s.at(w),
                });
                for op in ALU {
                    v.push(Inst::AluRR {
                        op,
                        dst: r.at(w),
                        src: s.at(w),
                    });
                }
            }
        }
        for w in WIDTHS {
            for imm in [
                0u64,
                1,
                0x7F,
                0x80,
                0xFF,
                0x7FFF_FFFF,
                0x8000_0000,
                0xFFFF_FFFF,
                0x1_0000_0000,
                0xFFFF_FFFF_8000_0000,
                0x8000_0000_0000_0000,
                u64::MAX,
            ] {
                let mask = match w {
                    Width::W8 => 0xFF,
                    Width::W16 => 0xFFFF,
                    Width::W32 => 0xFFFF_FFFF,
                    Width::W64 => u64::MAX,
                };
                v.push(Inst::MovRI {
                    dst: r.at(w),
                    imm: imm & mask,
                    style: ImmStyle::Hex,
                });
            }
            for op in ALU {
                for &imm in imms(w) {
                    v.push(Inst::AluRI {
                        op,
                        dst: r.at(w),
                        imm,
                    });
                }
            }
        }
        for &x in &xmms {
            for fp in FPS {
                v.push(Inst::CvtSi2F { fp, dst: x, src: r });
            }
            v.push(Inst::Cvttsd2si { dst: r, src: x });
        }
    }

    for &mem in &mems {
        memory_forms(&mut v, mem);
    }

    for &a in &xmms {
        for &b in &xmms {
            for fp in FPS {
                for op in [SseOp::Add, SseOp::Sub, SseOp::Mul, SseOp::Div] {
                    v.push(Inst::FArith {
                        op,
                        fp,
                        dst: a,
                        src: b,
                    });
                }
                v.push(Inst::Ucomi { fp, a, b });
            }
            v.push(Inst::Cvtss2sd { dst: a, src: b });
            v.push(Inst::Xorpd { dst: a, src: b });
        }
    }

    v.extend([
        Inst::Cqo,
        Inst::Cld,
        Inst::RepMovsb,
        Inst::Leave,
        Inst::Ret,
        Inst::Ud2,
        Inst::Call(FunctionId::new(0)),
    ]);

    // Branches: short (to a nearby label) and long (to the far start).
    v.push(Inst::Label(Label(1)));
    for cond in CONDS {
        v.push(Inst::Jcc(cond, Label(1)));
        v.push(Inst::Jcc(cond, Label(0)));
        v.push(Inst::Jcc(cond, Label(2)));
    }
    v.push(Inst::Jmp(Label(1)));
    v.push(Inst::Jmp(Label(0)));
    v.push(Inst::Jmp(Label(2)));
    v.push(Inst::Label(Label(2)));

    // RIP-relative forms come last: NASM sizes `op [rel sym], imm8` as if
    // it had an imm32 in its first pass while `sym` (in `.rodata`, printed
    // after `.text`) is still undefined. That stale first-pass layout can
    // leave a nearby forward branch in its long form, a NASM artifact the
    // code generator never triggers (it emits no immediate-to-data forms).
    memory_forms(&mut v, Mem::Data(DataId(0)));
    v
}

/// A deterministic xorshift generator (the crate has no dependencies).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// A random function: labels, `jmp`/`jcc` to them, and padding of varied
/// sizes, dense enough that many branches sit near the rel8 boundary.
fn random_branches(rng: &mut Rng) -> (Vec<Inst>, usize) {
    let labels = 1 + rng.below(4) as usize;
    let padding = [
        Inst::Ret, // 1 byte
        Inst::Lea {
            dst: Reg::Rax,
            mem: Mem::at(Reg::Rbp, -8),
        }, // 4 bytes
        Inst::Lea {
            dst: Reg::R9,
            mem: Mem::at(Reg::Rbp, -800),
        }, // 7 bytes
        Inst::MovRI {
            dst: Reg::Rax.q(),
            imm: u64::MAX >> 1,
            style: ImmStyle::Hex,
        }, // 10 bytes
    ];
    let mut v: Vec<Inst> = (0..labels as u32).map(|l| Inst::Label(Label(l))).collect();
    for _ in 0..rng.below(40) {
        let target = Label(rng.below(labels as u64) as u32);
        v.push(match rng.below(3) {
            0 => Inst::Jmp(target),
            _ => Inst::Jcc(CONDS[rng.below(CONDS.len() as u64) as usize], target),
        });
    }
    for _ in 0..rng.below(60) {
        let pad = padding[rng.below(padding.len() as u64) as usize].clone();
        for _ in 0..1 + rng.below(12) {
            v.push(pad.clone());
        }
    }
    // Shuffle (Fisher-Yates).
    for i in (1..v.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        v.swap(i, j);
    }
    (v, labels)
}

/// Returns the contents of `.text` in an ELF64 object.
fn text_section(elf: &[u8]) -> &[u8] {
    let u16_at = |at: usize| u16::from_le_bytes(elf[at..at + 2].try_into().unwrap()) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(elf[at..at + 4].try_into().unwrap()) as usize;
    let u64_at = |at: usize| u64::from_le_bytes(elf[at..at + 8].try_into().unwrap()) as usize;
    let (shoff, shnum, shstrndx) = (u64_at(0x28), u16_at(0x3C), u16_at(0x3E));
    let names = u64_at(shoff + shstrndx * 64 + 24);
    (0..shnum)
        .map(|i| shoff + i * 64)
        .find(|&h| elf[names + u32_at(h)..].starts_with(b".text\0"))
        .map(|h| &elf[u64_at(h + 24)..u64_at(h + 24) + u64_at(h + 32)])
        .expect("object has .text")
}

fn nasm_available() -> bool {
    matches!(Command::new("nasm").arg("-v").output(), Ok(o) if o.status.success())
}

/// Assembles `program`'s NASM text and returns NASM's `.text`.
fn nasm_text(program: &Program, tag: &str) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("astronomy-nasm-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (asm_path, obj_path) = (dir.join("forms.asm"), dir.join("forms.o"));
    std::fs::write(&asm_path, crate::nasm::print(program)).unwrap();
    let out = Command::new("nasm")
        .args(["-f", "elf64", "-w-all"])
        .arg(&asm_path)
        .arg("-o")
        .arg(&obj_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let elf = std::fs::read(&obj_path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    text_section(&elf).to_vec()
}

fn function(id: u32, labels: usize, insts: Vec<Inst>) -> FuncCode {
    FuncCode {
        id: FunctionId::new(id),
        labels: (0..labels)
            .map(|l| (format!("f{id}.l{l}"), LabelKind::Local))
            .collect(),
        insts,
    }
}

fn program(functions: Vec<FuncCode>) -> Program {
    Program {
        symbols: (0..functions.len())
            .map(|i| FuncSymbol {
                name: format!("f{i}"),
                kind: SymbolKind::Global,
            })
            .collect(),
        functions,
        rodata: vec![b"data".to_vec()],
    }
}

#[test]
fn encoder_matches_nasm_for_every_form() {
    if !nasm_available() {
        eprintln!("skipping: `nasm` not available");
        return;
    }
    let insts = every_form();
    assert!(insts.len() > 10_000, "the matrix should be exhaustive");
    let program = program(vec![function(0, 3, insts.clone())]);
    let ours = crate::encode::assemble(&program).expect("encodes").bytes;
    let theirs = nasm_text(&program, "forms");
    if ours == theirs {
        return;
    }

    // Report the first differing instruction, not just a byte offset.
    let mut at = 0usize;
    for inst in &insts {
        match inst {
            Inst::Label(_) => continue,
            Inst::Jmp(_) | Inst::Jcc(..) => break,
            // Its displacement depends on the position; skip the check.
            Inst::Call(_) => {
                at += 5;
                continue;
            }
            _ => {}
        }
        let single = self::program(vec![function(0, 0, vec![inst.clone()])]);
        let one = crate::encode::assemble(&single).expect("encodes").bytes;
        if theirs.get(at..at + one.len()) != Some(&one[..]) {
            let end = (at + one.len() + 4).min(theirs.len());
            panic!(
                "encoding differs for {inst:?}\n ours: {one:02x?}\n nasm: {:02x?}",
                &theirs[at..end]
            );
        }
        at += one.len();
    }
    panic!(
        "encodings differ ({} vs {} bytes) in the branch section",
        ours.len(),
        theirs.len()
    );
}

/// Branch relaxation picks the same short/near forms as NASM on many
/// random branch-dense functions laid out back to back.
#[test]
fn branch_relaxation_matches_nasm() {
    if !nasm_available() {
        eprintln!("skipping: `nasm` not available");
        return;
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let functions = (0..300)
        .map(|id| {
            let (insts, labels) = random_branches(&mut rng);
            function(id, labels, insts)
        })
        .collect();
    let program = program(functions);
    let ours = crate::encode::assemble(&program).expect("encodes");
    let theirs = nasm_text(&program, "branches");
    if ours.bytes != theirs {
        let first = ours
            .functions
            .iter()
            .find(|(_, start, size)| {
                let range = *start as usize..(*start + *size) as usize;
                theirs.get(range.clone()) != Some(&ours.bytes[range])
            })
            .map(|(id, ..)| id.index());
        panic!("branch layout differs from NASM, first in function {first:?}");
    }
}
