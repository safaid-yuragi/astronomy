//! `.arb` encoder: in-memory [`Module`] → bytes.

use crate::block::{BasicBlock, Terminator};
use crate::constant::ConstantData;
use crate::function::{Abi, Function, Linkage};
use crate::instruction::{Instruction, InstructionKind};
use crate::module::Module;
use crate::types::{FloatKind, TypeData};
use crate::value::ValueKind;

use super::{
    const_tag, crc32, opcode, section, term_tag, type_tag, value_tag, FORMAT_MAJOR, FORMAT_MINOR,
    MAGIC,
};

/// Serializes a module to `.arb` bytes.
///
/// Works on any [`Module`], verified or not (`&VerifiedModule` derefs to
/// `&Module`); the output is deterministic. Source spans are not stored.
///
/// # Panics
///
/// Panics if a single string or list exceeds `u32::MAX` entries, which no
/// module addressable by `u32` IDs can reach in practice.
pub fn write(module: &Module) -> Vec<u8> {
    let mut w = Writer {
        out: Vec::with_capacity(256),
    };
    w.bytes(&MAGIC);
    w.u16(FORMAT_MAJOR);
    w.u16(FORMAT_MINOR);
    w.u32(0); // flags: reserved, must be zero

    w.section(section::MODULE, |w| {
        w.u32(module.version.major);
        w.u32(module.version.minor);
        w.opt_u32(module.name.map(|s| s.as_u32()));
    });

    w.section(section::SYMBOLS, |w| {
        let symbols = &module.symbols;
        w.len(symbols.len());
        for i in 0..symbols.len() {
            let name = symbols
                .name(crate::id::SymbolId::new(i as u32))
                .expect("symbol index in range");
            w.blob(name.as_bytes());
        }
    });

    w.section(section::TYPES, |w| {
        let types = &module.types;
        w.len(types.len());
        for i in 0..types.len() {
            let data = types
                .get(crate::id::TypeId::new(i as u32))
                .expect("type index in range");
            w.type_data(data);
        }
    });

    w.section(section::CONSTANTS, |w| {
        let constants = &module.constants;
        w.len(constants.len());
        for i in 0..constants.len() {
            let data = constants
                .get(crate::id::ConstantId::new(i as u32))
                .expect("constant index in range");
            w.constant(data);
        }
    });

    w.section(section::FUNCTIONS, |w| {
        w.len(module.functions.len());
        for f in &module.functions {
            w.function(f);
        }
    });

    let crc = crc32(&w.out);
    w.u32(crc);
    w.out
}

struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn bytes(&mut self, b: &[u8]) {
        self.out.extend_from_slice(b);
    }

    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    fn u128(&mut self, v: u128) {
        self.bytes(&v.to_le_bytes());
    }

    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect(".arb lists are limited to u32::MAX entries"));
    }

    fn opt_u32(&mut self, v: Option<u32>) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                self.u32(v);
            }
        }
    }

    fn blob(&mut self, b: &[u8]) {
        self.len(b.len());
        self.bytes(b);
    }

    fn ids(&mut self, ids: impl ExactSizeIterator<Item = u32>) {
        self.len(ids.len());
        for id in ids {
            self.u32(id);
        }
    }

    /// Writes `tag`, a length placeholder, the body, then patches the
    /// length.
    fn section(&mut self, tag: [u8; 4], body: impl FnOnce(&mut Self)) {
        self.bytes(&tag);
        let len_at = self.out.len();
        self.u64(0);
        let start = self.out.len();
        body(self);
        let len = (self.out.len() - start) as u64;
        self.out[len_at..len_at + 8].copy_from_slice(&len.to_le_bytes());
    }

    fn type_data(&mut self, data: &TypeData) {
        match data {
            TypeData::Void => self.u8(type_tag::VOID),
            TypeData::Int { bits, signed } => {
                self.u8(type_tag::INT);
                self.u32(*bits);
                self.bool(*signed);
            }
            TypeData::Float(kind) => {
                self.u8(type_tag::FLOAT);
                self.u8(match kind {
                    FloatKind::F32 => 0,
                    FloatKind::F64 => 1,
                });
            }
            TypeData::Pointer {
                pointee,
                address_space,
            } => {
                self.u8(type_tag::POINTER);
                self.u32(pointee.as_u32());
                self.u32(*address_space);
            }
            TypeData::Array { element, length } => {
                self.u8(type_tag::ARRAY);
                self.u32(element.as_u32());
                self.u64(*length);
            }
            TypeData::Struct { fields } => {
                self.u8(type_tag::STRUCT);
                self.ids(fields.iter().map(|t| t.as_u32()));
            }
            TypeData::Function {
                params,
                variadic,
                result,
            } => {
                self.u8(type_tag::FUNCTION);
                self.ids(params.iter().map(|t| t.as_u32()));
                self.bool(*variadic);
                self.u32(result.as_u32());
            }
        }
    }

    fn constant(&mut self, data: &ConstantData) {
        match data {
            ConstantData::Int { ty, width, bits } => {
                self.u8(const_tag::INT);
                self.u32(ty.as_u32());
                self.u32(*width);
                self.u128(*bits);
            }
            ConstantData::Float { ty, bits } => {
                self.u8(const_tag::FLOAT);
                self.u32(ty.as_u32());
                self.u64(*bits);
            }
            ConstantData::Null { pointee } => {
                self.u8(const_tag::NULL);
                self.u32(pointee.as_u32());
            }
            ConstantData::String { bytes } => {
                self.u8(const_tag::STRING);
                self.blob(bytes);
            }
            ConstantData::Aggregate { ty, elements } => {
                self.u8(const_tag::AGGREGATE);
                self.u32(ty.as_u32());
                self.ids(elements.iter().map(|c| c.as_u32()));
            }
        }
    }

    fn function(&mut self, f: &Function) {
        self.u32(f.symbol.as_u32());
        self.u8(match f.linkage {
            Linkage::Internal => 0,
            Linkage::External => 1,
            Linkage::Exported => 2,
        });
        match f.abi {
            Abi::Astronomy => self.u8(0),
            Abi::C => self.u8(1),
            Abi::System => self.u8(2),
            Abi::Custom(sym) => {
                self.u8(3);
                self.u32(sym.as_u32());
            }
        }
        self.bool(f.variadic);
        self.u32(f.result.as_u32());

        self.len(f.params.len());
        for p in &f.params {
            self.u32(p.value.as_u32());
            self.u32(p.ty.as_u32());
        }

        self.len(f.values.len());
        for v in &f.values {
            self.u32(v.ty.as_u32());
            match v.kind {
                ValueKind::Reserved => self.u8(value_tag::RESERVED),
                ValueKind::Param { index } => {
                    self.u8(value_tag::PARAM);
                    self.u32(index);
                }
                ValueKind::BlockParam { block, index } => {
                    self.u8(value_tag::BLOCK_PARAM);
                    self.u32(block.as_u32());
                    self.u32(index);
                }
                ValueKind::Inst { block, index } => {
                    self.u8(value_tag::INST);
                    self.u32(block.as_u32());
                    self.u32(index);
                }
            }
            self.opt_u32(v.name.map(|s| s.as_u32()));
        }

        self.len(f.blocks.len());
        for b in &f.blocks {
            self.block(b);
        }
    }

    fn block(&mut self, b: &BasicBlock) {
        self.opt_u32(b.name.map(|s| s.as_u32()));
        self.len(b.params.len());
        for p in &b.params {
            self.u32(p.value.as_u32());
            self.u32(p.ty.as_u32());
        }
        self.len(b.instructions.len());
        for inst in &b.instructions {
            self.instruction(inst);
        }
        match &b.terminator {
            None => self.u8(term_tag::NONE),
            Some(Terminator::Jump { target, args }) => {
                self.u8(term_tag::JUMP);
                self.u32(target.as_u32());
                self.ids(args.iter().map(|v| v.as_u32()));
            }
            Some(Terminator::Branch {
                condition,
                then_block,
                then_args,
                else_block,
                else_args,
            }) => {
                self.u8(term_tag::BRANCH);
                self.u32(condition.as_u32());
                self.u32(then_block.as_u32());
                self.ids(then_args.iter().map(|v| v.as_u32()));
                self.u32(else_block.as_u32());
                self.ids(else_args.iter().map(|v| v.as_u32()));
            }
            Some(Terminator::Return { value }) => {
                self.u8(term_tag::RETURN);
                self.opt_u32(value.map(|v| v.as_u32()));
            }
            Some(Terminator::Unreachable) => self.u8(term_tag::UNREACHABLE),
        }
    }

    fn instruction(&mut self, inst: &Instruction) {
        use InstructionKind as K;
        let binary = |w: &mut Self, op: u8, lhs: &crate::ValueId, rhs: &crate::ValueId| {
            w.u8(op);
            w.u32(lhs.as_u32());
            w.u32(rhs.as_u32());
        };
        let convert = |w: &mut Self, op: u8, to: &crate::TypeId, value: &crate::ValueId| {
            w.u8(op);
            w.u32(to.as_u32());
            w.u32(value.as_u32());
        };
        match &inst.kind {
            K::Const(c) => {
                self.u8(opcode::CONST);
                self.u32(c.as_u32());
            }
            K::Add { lhs, rhs } => binary(self, opcode::ADD, lhs, rhs),
            K::Sub { lhs, rhs } => binary(self, opcode::SUB, lhs, rhs),
            K::Mul { lhs, rhs } => binary(self, opcode::MUL, lhs, rhs),
            K::Div { lhs, rhs } => binary(self, opcode::DIV, lhs, rhs),
            K::Rem { lhs, rhs } => binary(self, opcode::REM, lhs, rhs),
            K::And { lhs, rhs } => binary(self, opcode::AND, lhs, rhs),
            K::Or { lhs, rhs } => binary(self, opcode::OR, lhs, rhs),
            K::Xor { lhs, rhs } => binary(self, opcode::XOR, lhs, rhs),
            K::Shl { lhs, rhs } => binary(self, opcode::SHL, lhs, rhs),
            K::Shr { lhs, rhs } => binary(self, opcode::SHR, lhs, rhs),
            K::Eq { lhs, rhs } => binary(self, opcode::EQ, lhs, rhs),
            K::Ne { lhs, rhs } => binary(self, opcode::NE, lhs, rhs),
            K::Lt { lhs, rhs } => binary(self, opcode::LT, lhs, rhs),
            K::Le { lhs, rhs } => binary(self, opcode::LE, lhs, rhs),
            K::Gt { lhs, rhs } => binary(self, opcode::GT, lhs, rhs),
            K::Ge { lhs, rhs } => binary(self, opcode::GE, lhs, rhs),
            K::Alloca { pointee } => {
                self.u8(opcode::ALLOCA);
                self.u32(pointee.as_u32());
            }
            K::Load { ty, pointer } => {
                self.u8(opcode::LOAD);
                self.u32(ty.as_u32());
                self.u32(pointer.as_u32());
            }
            K::Store { pointer, value } => binary(self, opcode::STORE, pointer, value),
            K::PtrOffset { pointer, offset } => binary(self, opcode::PTR_OFFSET, pointer, offset),
            K::Ext { to, value } => convert(self, opcode::EXT, to, value),
            K::Trunc { to, value } => convert(self, opcode::TRUNC, to, value),
            K::IntToFloat { to, value } => convert(self, opcode::INT_TO_FLOAT, to, value),
            K::FloatToInt { to, value } => convert(self, opcode::FLOAT_TO_INT, to, value),
            K::PtrCast { to, pointer } => convert(self, opcode::PTR_CAST, to, pointer),
            K::Call { callee, args } => {
                self.u8(opcode::CALL);
                self.u32(callee.as_u32());
                self.ids(args.iter().map(|v| v.as_u32()));
            }
            K::Construct { ty, fields } => {
                self.u8(opcode::CONSTRUCT);
                self.u32(ty.as_u32());
                self.ids(fields.iter().map(|v| v.as_u32()));
            }
            K::Extract { aggregate, index } => {
                self.u8(opcode::EXTRACT);
                self.u32(aggregate.as_u32());
                self.u32(*index);
            }
            K::Insert {
                aggregate,
                index,
                value,
            } => {
                self.u8(opcode::INSERT);
                self.u32(aggregate.as_u32());
                self.u32(*index);
                self.u32(value.as_u32());
            }
        }
        self.opt_u32(inst.result.map(|v| v.as_u32()));
    }
}
