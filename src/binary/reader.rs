//! `.arb` decoder: bytes → in-memory [`Module`].
//!
//! The input is untrusted. Every read is bounds-checked, every list length
//! is checked against the bytes that remain before anything is allocated,
//! and every enum tag is validated. Structural invariants the in-memory
//! stores rely on are enforced here (interned tables are duplicate-free,
//! the scalar type prefix is intact, type entries only reference earlier
//! entries, symbol references resolve). Everything else — the types of
//! operands, SSA form, dominance, dangling value/block/function IDs — is
//! left to the [`Verifier`](crate::Verifier), as for parsed `.arn`.

use crate::block::{BasicBlock, BlockParam, Terminator};
use crate::constant::{ConstantData, ConstantStore};
use crate::error::ArbError;
use crate::function::{Abi, Function, Linkage, Param};
use crate::id::{BlockId, ConstantId, FunctionId, SymbolId, TypeId, ValueId};
use crate::instruction::{Instruction, InstructionKind};
use crate::module::{ArnVersion, Module};
use crate::symbol::SymbolStore;
use crate::types::{type_to_string, FloatKind, TypeData, TypeStore};
use crate::value::{ValueData, ValueKind};

use super::{
    const_tag, crc32, opcode, section, term_tag, type_tag, value_tag, FORMAT_MAJOR, MAGIC,
};

/// Size of the fixed header: magic, format major, format minor, flags.
const HEADER_LEN: usize = 8 + 2 + 2 + 4;
/// Size of the trailing CRC-32.
const FOOTER_LEN: usize = 4;

/// Decodes `.arb` bytes into a [`Module`].
///
/// The result is *not* verified; call [`Module::verify`] before handing it
/// to a backend.
pub fn read(bytes: &[u8]) -> Result<Module, ArbError> {
    if !bytes.starts_with(&MAGIC) {
        return Err(ArbError::BadMagic);
    }
    let mut header = Cursor::new(bytes, 0);
    header.take(MAGIC.len(), "magic")?;
    let major = header.u16("format version")?;
    let _minor = header.u16("format version")?;
    if major != FORMAT_MAJOR {
        return Err(ArbError::UnsupportedFormatVersion {
            found: major,
            supported: FORMAT_MAJOR,
        });
    }
    let flags_at = header.offset();
    let flags = header.u32("header flags")?;
    if flags != 0 {
        return Err(ArbError::InvalidValue {
            offset: flags_at,
            reason: format!("reserved header flags must be zero, found {flags:#x}"),
        });
    }

    if bytes.len() < HEADER_LEN + FOOTER_LEN {
        return Err(ArbError::UnexpectedEof {
            offset: bytes.len(),
            what: "checksum",
        });
    }
    let body_end = bytes.len() - FOOTER_LEN;
    let stored = u32::from_le_bytes(bytes[body_end..].try_into().expect("4-byte footer"));
    let computed = crc32(&bytes[..body_end]);
    if stored != computed {
        return Err(ArbError::ChecksumMismatch { stored, computed });
    }

    let mut file = Cursor::new(&bytes[HEADER_LEN..body_end], HEADER_LEN);

    let mut s = file.section(section::MODULE)?;
    let version_at = s.offset();
    let version = ArnVersion {
        major: s.u32("module version")?,
        minor: s.u32("module version")?,
    };
    if version.major != ArnVersion::SUPPORTED_MAJOR {
        return Err(ArbError::UnsupportedVersion {
            offset: version_at,
            found: version.major,
            supported: ArnVersion::SUPPORTED_MAJOR,
        });
    }
    let name_at = s.offset();
    let name = s.opt_u32("module name")?;
    s.finish()?;

    let mut s = file.section(section::SYMBOLS)?;
    let symbols = read_symbols(&mut s)?;
    s.finish()?;
    let name = name
        .map(|id| check_symbol(id, &symbols, name_at))
        .transpose()?;

    let mut s = file.section(section::TYPES)?;
    let types = read_types(&mut s)?;
    s.finish()?;

    let mut s = file.section(section::CONSTANTS)?;
    let constants = read_constants(&mut s)?;
    s.finish()?;

    let mut s = file.section(section::FUNCTIONS)?;
    let count = s.count(MIN_FUNCTION_LEN, "functions")?;
    let mut functions = Vec::with_capacity(count);
    for _ in 0..count {
        functions.push(read_function(&mut s, &symbols)?);
    }
    s.finish()?;

    if !file.is_empty() {
        return Err(ArbError::TrailingBytes {
            offset: file.offset(),
        });
    }

    Ok(Module {
        version,
        name,
        symbols,
        types,
        constants,
        functions,
    })
}

// Minimum encoded sizes, used to reject list lengths the remaining input
// cannot possibly hold before allocating for them.
const MIN_SYMBOL_LEN: usize = 4;
const MIN_TYPE_LEN: usize = 1;
const MIN_CONSTANT_LEN: usize = 5;
const MIN_FUNCTION_LEN: usize = 4 + 1 + 1 + 1 + 4 + 4 + 4 + 4;
const MIN_PARAM_LEN: usize = 8;
const MIN_VALUE_LEN: usize = 4 + 1 + 1;
const MIN_BLOCK_LEN: usize = 1 + 4 + 4 + 1;
const MIN_INSTRUCTION_LEN: usize = 1 + 4 + 1;
const ID_LEN: usize = 4;

/// Number of scalar types every [`TypeStore`] pre-interns.
const SCALAR_TYPES: usize = 14;

fn read_symbols(s: &mut Cursor<'_>) -> Result<SymbolStore, ArbError> {
    let count = s.count(MIN_SYMBOL_LEN, "symbols")?;
    let mut symbols = SymbolStore::new();
    for i in 0..count {
        let at = s.offset();
        let raw = s.blob("symbol name")?;
        let name = std::str::from_utf8(raw).map_err(|e| ArbError::InvalidValue {
            offset: at,
            reason: format!("symbol {i} is not valid UTF-8: {e}"),
        })?;
        if symbols.intern(name).index() != i {
            return Err(ArbError::DuplicateEntry {
                offset: at,
                table: "symbol",
                index: i as u32,
            });
        }
    }
    Ok(symbols)
}

fn read_types(s: &mut Cursor<'_>) -> Result<TypeStore, ArbError> {
    let count_at = s.offset();
    let count = s.count(MIN_TYPE_LEN, "types")?;
    if count < SCALAR_TYPES {
        return Err(ArbError::InvalidValue {
            offset: count_at,
            reason: format!(
                "type table has {count} entries; the {SCALAR_TYPES} scalar types are mandatory"
            ),
        });
    }
    let mut types = TypeStore::new();
    for i in 0..count {
        let at = s.offset();
        let data = read_type(s, i as u32)?;
        if i < SCALAR_TYPES {
            let expected = types.get(TypeId::new(i as u32)).expect("scalar prefix");
            if &data != expected {
                return Err(ArbError::InvalidValue {
                    offset: at,
                    reason: format!(
                        "type {i} must be the scalar `{}`",
                        type_to_string(&types, TypeId::new(i as u32))
                    ),
                });
            }
            continue;
        }
        if types.intern(data).index() != i {
            return Err(ArbError::DuplicateEntry {
                offset: at,
                table: "type",
                index: i as u32,
            });
        }
    }
    Ok(types)
}

fn read_type(s: &mut Cursor<'_>, index: u32) -> Result<TypeData, ArbError> {
    let entry_at = s.offset();
    // Types may only reference earlier entries: this keeps the table
    // acyclic, so recursive consumers such as `type_to_string` terminate.
    let earlier = |id: u32| {
        if id < index {
            Ok(TypeId::new(id))
        } else {
            Err(ArbError::ForwardTypeReference {
                offset: entry_at,
                index,
                referenced: id,
            })
        }
    };
    let tag_at = s.offset();
    let data = match s.u8("type tag")? {
        type_tag::VOID => TypeData::Void,
        type_tag::INT => {
            let bits_at = s.offset();
            let bits = s.u32("integer width")?;
            let signed = s.bool("integer signedness")?;
            if !(1..=128).contains(&bits) || (bits == 1 && signed) {
                return Err(ArbError::InvalidValue {
                    offset: bits_at,
                    reason: format!(
                        "invalid integer type (bits={bits}, signed={signed}); \
                         widths are 1..=128 and `i1` is unsigned"
                    ),
                });
            }
            TypeData::Int { bits, signed }
        }
        type_tag::FLOAT => {
            let kind_at = s.offset();
            match s.u8("float kind")? {
                0 => TypeData::Float(FloatKind::F32),
                1 => TypeData::Float(FloatKind::F64),
                tag => {
                    return Err(ArbError::InvalidTag {
                        offset: kind_at,
                        what: "float kind",
                        tag,
                    })
                }
            }
        }
        type_tag::POINTER => TypeData::Pointer {
            pointee: earlier(s.u32("pointee type")?)?,
            address_space: s.u32("address space")?,
        },
        type_tag::ARRAY => TypeData::Array {
            element: earlier(s.u32("element type")?)?,
            length: s.u64("array length")?,
        },
        type_tag::STRUCT => TypeData::Struct {
            fields: s
                .ids("struct fields")?
                .into_iter()
                .map(earlier)
                .collect::<Result<_, _>>()?,
        },
        type_tag::FUNCTION => {
            let params = s
                .ids("function type parameters")?
                .into_iter()
                .map(earlier)
                .collect::<Result<_, _>>()?;
            let variadic = s.bool("variadic flag")?;
            let result = earlier(s.u32("result type")?)?;
            TypeData::Function {
                params,
                variadic,
                result,
            }
        }
        tag => {
            return Err(ArbError::InvalidTag {
                offset: tag_at,
                what: "type",
                tag,
            })
        }
    };
    Ok(data)
}

fn read_constants(s: &mut Cursor<'_>) -> Result<ConstantStore, ArbError> {
    let count = s.count(MIN_CONSTANT_LEN, "constants")?;
    let mut constants = ConstantStore::new();
    for i in 0..count {
        let at = s.offset();
        let data = match s.u8("constant tag")? {
            const_tag::INT => {
                let ty = TypeId::new(s.u32("constant type")?);
                let width_at = s.offset();
                let width = s.u32("constant width")?;
                let bits = s.u128("constant bits")?;
                if !(1..=128).contains(&width) || bits & !ConstantStore::mask(width) != 0 {
                    return Err(ArbError::InvalidValue {
                        offset: width_at,
                        reason: format!("integer constant {i} is not a masked {width}-bit pattern"),
                    });
                }
                ConstantData::Int { ty, width, bits }
            }
            const_tag::FLOAT => ConstantData::Float {
                ty: TypeId::new(s.u32("constant type")?),
                bits: s.u64("constant bits")?,
            },
            const_tag::NULL => ConstantData::Null {
                pointee: TypeId::new(s.u32("pointee type")?),
            },
            const_tag::STRING => ConstantData::String {
                bytes: s.blob("string bytes")?.to_vec(),
            },
            const_tag::AGGREGATE => ConstantData::Aggregate {
                ty: TypeId::new(s.u32("constant type")?),
                elements: s
                    .ids("aggregate elements")?
                    .into_iter()
                    .map(ConstantId::new)
                    .collect(),
            },
            tag => {
                return Err(ArbError::InvalidTag {
                    offset: at,
                    what: "constant",
                    tag,
                })
            }
        };
        if constants.intern(data).index() != i {
            return Err(ArbError::DuplicateEntry {
                offset: at,
                table: "constant",
                index: i as u32,
            });
        }
    }
    Ok(constants)
}

fn check_symbol(id: u32, symbols: &SymbolStore, offset: usize) -> Result<SymbolId, ArbError> {
    if (id as usize) < symbols.len() {
        Ok(SymbolId::new(id))
    } else {
        Err(ArbError::InvalidValue {
            offset,
            reason: format!(
                "symbol {id} is out of range (the symbol table has {} entries)",
                symbols.len()
            ),
        })
    }
}

fn read_opt_symbol(
    s: &mut Cursor<'_>,
    symbols: &SymbolStore,
    what: &'static str,
) -> Result<Option<SymbolId>, ArbError> {
    let at = s.offset();
    s.opt_u32(what)?
        .map(|id| check_symbol(id, symbols, at))
        .transpose()
}

fn read_function(s: &mut Cursor<'_>, symbols: &SymbolStore) -> Result<Function, ArbError> {
    let at = s.offset();
    let symbol = check_symbol(s.u32("function symbol")?, symbols, at)?;

    let at = s.offset();
    let linkage = match s.u8("linkage")? {
        0 => Linkage::Internal,
        1 => Linkage::External,
        2 => Linkage::Exported,
        tag => {
            return Err(ArbError::InvalidTag {
                offset: at,
                what: "linkage",
                tag,
            })
        }
    };

    let at = s.offset();
    let abi = match s.u8("abi")? {
        0 => Abi::Astronomy,
        1 => Abi::C,
        2 => Abi::System,
        3 => {
            let sym_at = s.offset();
            Abi::Custom(check_symbol(s.u32("custom abi symbol")?, symbols, sym_at)?)
        }
        tag => {
            return Err(ArbError::InvalidTag {
                offset: at,
                what: "abi",
                tag,
            })
        }
    };

    let variadic = s.bool("variadic flag")?;
    let result = TypeId::new(s.u32("result type")?);

    let count = s.count(MIN_PARAM_LEN, "parameters")?;
    let mut params = Vec::with_capacity(count);
    for _ in 0..count {
        params.push(Param {
            value: ValueId::new(s.u32("parameter value")?),
            ty: TypeId::new(s.u32("parameter type")?),
        });
    }

    let count = s.count(MIN_VALUE_LEN, "values")?;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        let ty = TypeId::new(s.u32("value type")?);
        let at = s.offset();
        let kind = match s.u8("value kind")? {
            value_tag::RESERVED => ValueKind::Reserved,
            value_tag::PARAM => ValueKind::Param {
                index: s.u32("parameter index")?,
            },
            value_tag::BLOCK_PARAM => ValueKind::BlockParam {
                block: BlockId::new(s.u32("block")?),
                index: s.u32("block parameter index")?,
            },
            value_tag::INST => ValueKind::Inst {
                block: BlockId::new(s.u32("block")?),
                index: s.u32("instruction index")?,
            },
            tag => {
                return Err(ArbError::InvalidTag {
                    offset: at,
                    what: "value kind",
                    tag,
                })
            }
        };
        let name = read_opt_symbol(s, symbols, "value name")?;
        values.push(ValueData {
            ty,
            kind,
            name,
            span: None,
        });
    }

    let count = s.count(MIN_BLOCK_LEN, "blocks")?;
    let mut blocks = Vec::with_capacity(count);
    for _ in 0..count {
        blocks.push(read_block(s, symbols)?);
    }

    Ok(Function {
        symbol,
        linkage,
        abi,
        params,
        result,
        variadic,
        blocks,
        values,
        span: None,
    })
}

fn read_block(s: &mut Cursor<'_>, symbols: &SymbolStore) -> Result<BasicBlock, ArbError> {
    let name = read_opt_symbol(s, symbols, "block name")?;

    let count = s.count(MIN_PARAM_LEN, "block parameters")?;
    let mut params = Vec::with_capacity(count);
    for _ in 0..count {
        params.push(BlockParam {
            value: ValueId::new(s.u32("block parameter value")?),
            ty: TypeId::new(s.u32("block parameter type")?),
        });
    }

    let count = s.count(MIN_INSTRUCTION_LEN, "instructions")?;
    let mut instructions = Vec::with_capacity(count);
    for _ in 0..count {
        instructions.push(read_instruction(s)?);
    }

    let at = s.offset();
    let terminator = match s.u8("terminator tag")? {
        term_tag::NONE => None,
        term_tag::JUMP => Some(Terminator::Jump {
            target: BlockId::new(s.u32("jump target")?),
            args: s.values("jump arguments")?,
        }),
        term_tag::BRANCH => Some(Terminator::Branch {
            condition: ValueId::new(s.u32("branch condition")?),
            then_block: BlockId::new(s.u32("branch target")?),
            then_args: s.values("branch arguments")?,
            else_block: BlockId::new(s.u32("branch target")?),
            else_args: s.values("branch arguments")?,
        }),
        term_tag::RETURN => Some(Terminator::Return {
            value: s.opt_u32("return value")?.map(ValueId::new),
        }),
        term_tag::UNREACHABLE => Some(Terminator::Unreachable),
        tag => {
            return Err(ArbError::InvalidTag {
                offset: at,
                what: "terminator",
                tag,
            })
        }
    };

    Ok(BasicBlock {
        params,
        instructions,
        terminator,
        name,
        span: None,
    })
}

fn read_instruction(s: &mut Cursor<'_>) -> Result<Instruction, ArbError> {
    use InstructionKind as K;
    let at = s.offset();
    let op = s.u8("opcode")?;
    let value = |s: &mut Cursor<'_>| s.u32("operand").map(ValueId::new);
    let ty = |s: &mut Cursor<'_>| s.u32("type operand").map(TypeId::new);
    let kind = match op {
        opcode::CONST => K::Const(ConstantId::new(s.u32("constant operand")?)),
        opcode::ADD..=opcode::GE => {
            let lhs = value(s)?;
            let rhs = value(s)?;
            match op {
                opcode::ADD => K::Add { lhs, rhs },
                opcode::SUB => K::Sub { lhs, rhs },
                opcode::MUL => K::Mul { lhs, rhs },
                opcode::DIV => K::Div { lhs, rhs },
                opcode::REM => K::Rem { lhs, rhs },
                opcode::AND => K::And { lhs, rhs },
                opcode::OR => K::Or { lhs, rhs },
                opcode::XOR => K::Xor { lhs, rhs },
                opcode::SHL => K::Shl { lhs, rhs },
                opcode::SHR => K::Shr { lhs, rhs },
                opcode::EQ => K::Eq { lhs, rhs },
                opcode::NE => K::Ne { lhs, rhs },
                opcode::LT => K::Lt { lhs, rhs },
                opcode::LE => K::Le { lhs, rhs },
                opcode::GT => K::Gt { lhs, rhs },
                _ => K::Ge { lhs, rhs },
            }
        }
        opcode::ALLOCA => K::Alloca { pointee: ty(s)? },
        opcode::LOAD => K::Load {
            ty: ty(s)?,
            pointer: value(s)?,
        },
        opcode::STORE => K::Store {
            pointer: value(s)?,
            value: value(s)?,
        },
        opcode::PTR_OFFSET => K::PtrOffset {
            pointer: value(s)?,
            offset: value(s)?,
        },
        opcode::EXT => K::Ext {
            to: ty(s)?,
            value: value(s)?,
        },
        opcode::TRUNC => K::Trunc {
            to: ty(s)?,
            value: value(s)?,
        },
        opcode::INT_TO_FLOAT => K::IntToFloat {
            to: ty(s)?,
            value: value(s)?,
        },
        opcode::FLOAT_TO_INT => K::FloatToInt {
            to: ty(s)?,
            value: value(s)?,
        },
        opcode::PTR_CAST => K::PtrCast {
            to: ty(s)?,
            pointer: value(s)?,
        },
        opcode::CALL => K::Call {
            callee: FunctionId::new(s.u32("callee")?),
            args: s.values("call arguments")?,
        },
        opcode::CONSTRUCT => K::Construct {
            ty: ty(s)?,
            fields: s.values("construct fields")?,
        },
        opcode::EXTRACT => K::Extract {
            aggregate: value(s)?,
            index: s.u32("field index")?,
        },
        opcode::INSERT => K::Insert {
            aggregate: value(s)?,
            index: s.u32("field index")?,
            value: value(s)?,
        },
        tag => {
            return Err(ArbError::InvalidTag {
                offset: at,
                what: "opcode",
                tag,
            })
        }
    };
    let result = s.opt_u32("instruction result")?.map(ValueId::new);
    Ok(Instruction {
        kind,
        result,
        span: None,
    })
}

// ---------------------------------------------------------------------------
// Bounds-checked little-endian cursor
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    /// Absolute offset of `data[0]` in the whole input (for diagnostics).
    base: usize,
    /// Section tag, when this cursor spans one section.
    section: Option<[u8; 4]>,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], base: usize) -> Self {
        Cursor {
            data,
            pos: 0,
            base,
            section: None,
        }
    }

    fn offset(&self) -> usize {
        self.base + self.pos
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], ArbError> {
        if n > self.remaining() {
            return Err(ArbError::UnexpectedEof {
                offset: self.base + self.data.len(),
                what,
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], ArbError> {
        Ok(self.take(N, what)?.try_into().expect("exact length"))
    }

    fn u8(&mut self, what: &'static str) -> Result<u8, ArbError> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, ArbError> {
        self.array(what).map(u16::from_le_bytes)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, ArbError> {
        self.array(what).map(u32::from_le_bytes)
    }

    fn u64(&mut self, what: &'static str) -> Result<u64, ArbError> {
        self.array(what).map(u64::from_le_bytes)
    }

    fn u128(&mut self, what: &'static str) -> Result<u128, ArbError> {
        self.array(what).map(u128::from_le_bytes)
    }

    fn bool(&mut self, what: &'static str) -> Result<bool, ArbError> {
        let at = self.offset();
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(ArbError::InvalidValue {
                offset: at,
                reason: format!("{what} must be 0 or 1, found {v}"),
            }),
        }
    }

    fn opt_u32(&mut self, what: &'static str) -> Result<Option<u32>, ArbError> {
        let at = self.offset();
        match self.u8(what)? {
            0 => Ok(None),
            1 => self.u32(what).map(Some),
            v => Err(ArbError::InvalidValue {
                offset: at,
                reason: format!("{what} presence flag must be 0 or 1, found {v}"),
            }),
        }
    }

    /// Reads a `u32` list length and rejects it unless `count * min_len`
    /// bytes actually remain, so hostile lengths never drive allocation.
    fn count(&mut self, min_len: usize, what: &'static str) -> Result<usize, ArbError> {
        let at = self.offset();
        let count = self.u32(what)? as usize;
        if count.saturating_mul(min_len) > self.remaining() {
            return Err(ArbError::InvalidValue {
                offset: at,
                reason: format!(
                    "{count} {what} cannot fit in the remaining {} byte(s)",
                    self.remaining()
                ),
            });
        }
        Ok(count)
    }

    fn blob(&mut self, what: &'static str) -> Result<&'a [u8], ArbError> {
        let len = self.count(1, what)?;
        self.take(len, what)
    }

    fn ids(&mut self, what: &'static str) -> Result<Vec<u32>, ArbError> {
        let count = self.count(ID_LEN, what)?;
        (0..count).map(|_| self.u32(what)).collect()
    }

    fn values(&mut self, what: &'static str) -> Result<Vec<ValueId>, ArbError> {
        Ok(self.ids(what)?.into_iter().map(ValueId::new).collect())
    }

    /// Reads a section header that must carry `tag` and returns a cursor
    /// restricted to the section body.
    fn section(&mut self, tag: [u8; 4]) -> Result<Cursor<'a>, ArbError> {
        let at = self.offset();
        let found = self
            .array::<4>("section tag")
            .map_err(|_| ArbError::UnexpectedSection {
                offset: at,
                expected: tag_name(tag),
                found: "<end of data>".to_string(),
            })?;
        if found != tag {
            return Err(ArbError::UnexpectedSection {
                offset: at,
                expected: tag_name(tag),
                found: tag_name(found),
            });
        }
        let len = self.u64("section length")?;
        if len > self.remaining() as u64 {
            return Err(ArbError::SectionLength {
                offset: at,
                section: tag_name(tag),
                reason: format!(
                    "declared length {len} exceeds the remaining {} byte(s)",
                    self.remaining()
                ),
            });
        }
        let body_at = self.offset();
        let body = self.take(len as usize, "section body")?;
        let mut cursor = Cursor::new(body, body_at);
        cursor.section = Some(tag);
        Ok(cursor)
    }

    /// Ensures a section body was consumed exactly.
    fn finish(self) -> Result<(), ArbError> {
        if self.is_empty() {
            return Ok(());
        }
        Err(ArbError::SectionLength {
            offset: self.offset(),
            section: self.section.map(tag_name).unwrap_or_default(),
            reason: format!(
                "{} unused byte(s) at the end of the section",
                self.remaining()
            ),
        })
    }
}

fn tag_name(tag: [u8; 4]) -> String {
    tag.iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                (b as char).to_string()
            } else {
                format!("\\x{b:02X}")
            }
        })
        .collect()
}
