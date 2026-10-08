//! Structured x86-64 machine instructions.
//!
//! [`crate::lower`] turns Astronomy IR into a [`Program`] of these
//! instructions instead of text. The backends consume it independently:
//!
//! * `astronomy-nasm` prints it as NASM source text;
//! * `astronomy-object` encodes it into machine code and writes an ELF64
//!   relocatable object, with no external assembler.
//!
//! The model covers exactly the instruction forms the code generator uses.
//! Every operand is typed (register widths, memory operands, labels), so
//! both backends work from the same facts and cannot drift apart.

use astronomy::FunctionId;

/// A 64-bit general-purpose register, numbered as in the x86-64 encoding.
///
/// The full register file is modelled (and encodable) even though the
/// code generator only uses some of it.
#[allow(missing_docs, dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    Rax,
    Rcx,
    Rdx,
    Rbx,
    Rsp,
    Rbp,
    Rsi,
    Rdi,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

impl Reg {
    /// Hardware register number (0..=15).
    pub fn num(self) -> u8 {
        self as u8
    }

    /// The register viewed at `width`.
    pub fn at(self, width: Width) -> Gpr {
        Gpr { reg: self, width }
    }

    /// The full 64-bit register.
    pub fn q(self) -> Gpr {
        self.at(Width::W64)
    }

    /// NASM name of the register at `width`.
    pub fn name(self, width: Width) -> &'static str {
        const NAMES: [[&str; 4]; 16] = [
            ["al", "ax", "eax", "rax"],
            ["cl", "cx", "ecx", "rcx"],
            ["dl", "dx", "edx", "rdx"],
            ["bl", "bx", "ebx", "rbx"],
            ["spl", "sp", "esp", "rsp"],
            ["bpl", "bp", "ebp", "rbp"],
            ["sil", "si", "esi", "rsi"],
            ["dil", "di", "edi", "rdi"],
            ["r8b", "r8w", "r8d", "r8"],
            ["r9b", "r9w", "r9d", "r9"],
            ["r10b", "r10w", "r10d", "r10"],
            ["r11b", "r11w", "r11d", "r11"],
            ["r12b", "r12w", "r12d", "r12"],
            ["r13b", "r13w", "r13d", "r13"],
            ["r14b", "r14w", "r14d", "r14"],
            ["r15b", "r15w", "r15d", "r15"],
        ];
        NAMES[self as usize][width as usize]
    }
}

/// Operand width of an integer access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// 8 bits.
    W8,
    /// 16 bits.
    W16,
    /// 32 bits.
    W32,
    /// 64 bits.
    W64,
}

impl Width {
    /// Width for an access of `bytes` bytes (anything above 4 is 64-bit).
    pub fn from_bytes(bytes: u64) -> Width {
        match bytes {
            0 | 1 => Width::W8,
            2 => Width::W16,
            3 | 4 => Width::W32,
            _ => Width::W64,
        }
    }

    /// NASM size keyword (`byte`, `word`, `dword`, `qword`).
    pub fn keyword(self) -> &'static str {
        match self {
            Width::W8 => "byte",
            Width::W16 => "word",
            Width::W32 => "dword",
            Width::W64 => "qword",
        }
    }
}

/// A general-purpose register at a specific width (e.g. `eax`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gpr {
    /// The underlying 64-bit register.
    pub reg: Reg,
    /// Access width.
    pub width: Width,
}

/// An SSE register `xmm0`..`xmm15`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xmm(pub u8);

/// A read-only data entry (`arn.data.N`), indexed in emission order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataId(pub u32);

/// A code label local to one function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(pub u32);

/// What a label marks; only affects text layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    /// The start of an IR basic block (`fname.bbN`).
    Block,
    /// An internal branch target inside an instruction sequence.
    Local,
}

/// A memory operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mem {
    /// `[base + disp]`.
    Base {
        /// Base register.
        base: Reg,
        /// Signed displacement.
        disp: i32,
    },
    /// RIP-relative reference to a read-only data entry.
    Data(DataId),
}

impl Mem {
    /// `[base + disp]`.
    pub fn at(base: Reg, disp: i32) -> Mem {
        Mem::Base { base, disp }
    }
}

/// Condition codes, valued as the x86 `cc` nibble.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    B = 0x2,
    Ae = 0x3,
    E = 0x4,
    Ne = 0x5,
    Be = 0x6,
    A = 0x7,
    S = 0x8,
    P = 0xA,
    Np = 0xB,
    L = 0xC,
    Ge = 0xD,
    Le = 0xE,
    G = 0xF,
}

impl Cond {
    /// Mnemonic suffix (`e`, `ne`, `ae`, ...).
    pub fn suffix(self) -> &'static str {
        match self {
            Cond::B => "b",
            Cond::Ae => "ae",
            Cond::E => "e",
            Cond::Ne => "ne",
            Cond::Be => "be",
            Cond::A => "a",
            Cond::S => "s",
            Cond::P => "p",
            Cond::Np => "np",
            Cond::L => "l",
            Cond::Ge => "ge",
            Cond::Le => "le",
            Cond::G => "g",
        }
    }
}

/// Two-operand integer ALU operations, valued as their ModRM `/digit`.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AluOp {
    Add = 0,
    Or = 1,
    And = 4,
    Sub = 5,
    Xor = 6,
    Cmp = 7,
}

impl AluOp {
    /// Mnemonic.
    pub fn mnemonic(self) -> &'static str {
        match self {
            AluOp::Add => "add",
            AluOp::Or => "or",
            AluOp::And => "and",
            AluOp::Sub => "sub",
            AluOp::Xor => "xor",
            AluOp::Cmp => "cmp",
        }
    }
}

/// Shift operations, valued as their ModRM `/digit`.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftOp {
    Shl = 4,
    Shr = 5,
    Sar = 7,
}

impl ShiftOp {
    /// Mnemonic.
    pub fn mnemonic(self) -> &'static str {
        match self {
            ShiftOp::Shl => "shl",
            ShiftOp::Shr => "shr",
            ShiftOp::Sar => "sar",
        }
    }
}

/// Scalar SSE arithmetic, valued as the `0F xx` opcode byte.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseOp {
    Add = 0x58,
    Mul = 0x59,
    Sub = 0x5C,
    Div = 0x5E,
}

impl SseOp {
    /// Mnemonic stem (`add`, `mul`, ...).
    pub fn stem(self) -> &'static str {
        match self {
            SseOp::Add => "add",
            SseOp::Mul => "mul",
            SseOp::Sub => "sub",
            SseOp::Div => "div",
        }
    }
}

/// Scalar float precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fp {
    /// `f32` (`ss` forms).
    Single,
    /// `f64` (`sd` forms).
    Double,
}

impl Fp {
    /// `Single` when `f32` is true.
    pub fn of(f32: bool) -> Fp {
        if f32 {
            Fp::Single
        } else {
            Fp::Double
        }
    }

    /// Mnemonic suffix (`ss` / `sd`).
    pub fn suffix(self) -> &'static str {
        match self {
            Fp::Single => "ss",
            Fp::Double => "sd",
        }
    }
}

/// How an immediate is spelled in NASM text (no effect on encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImmStyle {
    /// Decimal (counts, sizes).
    Dec,
    /// Hexadecimal (bit patterns).
    Hex,
}

/// One machine instruction (or a label pseudo-instruction).
///
/// Operands are named by role, in Intel order: `dst` is written, `src`,
/// `mem`, `imm`, `a`/`b` are read (`mem` is written by stores), `op`
/// selects the operation and `fp` the float precision.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inst {
    /// Defines a label at this position.
    Label(Label),
    /// `push r64`.
    Push(Reg),
    /// `mov dst, src` (equal widths).
    MovRR { dst: Gpr, src: Gpr },
    /// `mov dst, imm`; the immediate is the bit pattern at `dst`'s width.
    MovRI { dst: Gpr, imm: u64, style: ImmStyle },
    /// `mov dst, width [mem]` (width taken from `dst`).
    Load { dst: Gpr, mem: Mem },
    /// `movsx`/`movsxd`/`movzx` from a narrower memory operand into a
    /// 64-bit register.
    LoadExt {
        dst: Reg,
        src: Width,
        signed: bool,
        mem: Mem,
    },
    /// `mov width [mem], src` (width taken from `src`).
    Store { mem: Mem, src: Gpr },
    /// `lea r64, [mem]`.
    Lea { dst: Reg, mem: Mem },
    /// `op dst, src` (equal widths).
    AluRR { op: AluOp, dst: Gpr, src: Gpr },
    /// `op dst, imm`.
    AluRI { op: AluOp, dst: Gpr, imm: i32 },
    /// `op width [mem], imm`.
    AluMI {
        op: AluOp,
        width: Width,
        mem: Mem,
        imm: i32,
    },
    /// `test a, b`.
    Test { a: Gpr, b: Gpr },
    /// `imul dst, src` (64-bit).
    ImulRR { dst: Reg, src: Reg },
    /// `imul dst, imm` (64-bit, i.e. `imul dst, dst, imm`).
    ImulRI { dst: Reg, imm: i32 },
    /// `div src` / `idiv src` (64-bit).
    Div { signed: bool, src: Reg },
    /// `op dst, cl` (64-bit).
    ShiftCl { op: ShiftOp, dst: Reg },
    /// `op dst, imm` (64-bit).
    ShiftRI { op: ShiftOp, dst: Reg, imm: u8 },
    /// `setcc dst8`.
    Setcc { cond: Cond, dst: Reg },
    /// `cqo`.
    Cqo,
    /// `cld`.
    Cld,
    /// `rep movsb`.
    RepMovsb,
    /// `leave`.
    Leave,
    /// `ret`.
    Ret,
    /// `ud2`.
    Ud2,
    /// `jmp label`.
    Jmp(Label),
    /// `jcc label`.
    Jcc(Cond, Label),
    /// Direct call to a module function (defined or declared).
    Call(FunctionId),
    /// `movss`/`movsd xmm, [mem]`.
    FLoad { fp: Fp, dst: Xmm, mem: Mem },
    /// `movss`/`movsd [mem], xmm`.
    FStore { fp: Fp, mem: Mem, src: Xmm },
    /// `addss`/`subsd`/... dst, src.
    FArith {
        op: SseOp,
        fp: Fp,
        dst: Xmm,
        src: Xmm,
    },
    /// `ucomiss`/`ucomisd` a, b.
    Ucomi { fp: Fp, a: Xmm, b: Xmm },
    /// `cvtsi2ss`/`cvtsi2sd` dst, r64.
    CvtSi2F { fp: Fp, dst: Xmm, src: Reg },
    /// `cvtss2sd` dst, src.
    Cvtss2sd { dst: Xmm, src: Xmm },
    /// `cvttsd2si` r64, xmm.
    Cvttsd2si { dst: Reg, src: Xmm },
    /// `xorpd` dst, src.
    Xorpd { dst: Xmm, src: Xmm },
}

/// The lowered body of one defined function.
#[derive(Debug, Clone)]
pub struct FuncCode {
    /// The IR function this code implements.
    pub id: FunctionId,
    /// Label names and kinds, indexed by [`Label`].
    pub labels: Vec<(String, LabelKind)>,
    /// The instruction stream, starting right after the function label.
    pub insts: Vec<Inst>,
}

/// Visibility of a module function's symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A declaration: defined elsewhere (`extern`).
    Extern,
    /// A definition visible outside the object (`global`).
    Global,
    /// A definition private to the object (internal linkage).
    Local,
}

/// The symbol of one module function.
#[derive(Debug, Clone)]
pub struct FuncSymbol {
    /// Symbol name.
    pub name: String,
    /// Visibility.
    pub kind: SymbolKind,
}

/// A whole lowered module.
#[derive(Debug, Clone, Default)]
pub struct Program {
    /// Every module function's symbol, indexed by `FunctionId`.
    pub symbols: Vec<FuncSymbol>,
    /// Defined functions, in module order.
    pub functions: Vec<FuncCode>,
    /// Read-only data entries, indexed by [`DataId`].
    pub rodata: Vec<Vec<u8>>,
}

impl Program {
    /// The symbol of a module function.
    pub fn symbol(&self, id: FunctionId) -> &FuncSymbol {
        &self.symbols[id.index()]
    }
}

/// Byte offset of each read-only data entry, plus the total size.
///
/// An empty entry still occupies one zero byte so that every label is
/// distinct (NASM text spells it `db 0`).
pub fn rodata_layout(entries: &[Vec<u8>]) -> (Vec<u64>, u64) {
    let mut offsets = Vec::with_capacity(entries.len());
    let mut at = 0u64;
    for bytes in entries {
        offsets.push(at);
        at += bytes.len().max(1) as u64;
    }
    (offsets, at)
}
