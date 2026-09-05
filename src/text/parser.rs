//! Parser for the `.arn` textual format (§31, §47).
//!
//! The parser never panics on malformed input; every failure is a
//! structured [`ParseError`] with a line number. Parsing is single-pass
//! into a raw tree, then a small two-phase body resolution so that value
//! definitions may appear in any textual order (the printer emits blocks
//! in reverse post-order, which can reorder definitions).

use std::collections::HashMap;

use crate::block::{BasicBlock, BlockParam};
use crate::error::ParseError;
use crate::function::{Abi, Function, Linkage};
use crate::id::{BlockId, ConstantId, FunctionId, TypeId, ValueId};
use crate::instruction::{Instruction, InstructionKind};
use crate::module::{ArnVersion, Module};
use crate::span::Span;
use crate::value::{ValueData, ValueKind};
use crate::Terminator;

use super::lexer::{lex, Token, TokenKind};

/// An annotated operand: `type %handle`.
struct RawOperand {
    ty: TypeId,
    handle: String,
    line: u32,
}

enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

enum ConvOp {
    Ext,
    Trunc,
    IntToFloat,
    FloatToInt,
    PtrCast,
}

enum RawInstKind {
    Const(ConstantId),
    Binary {
        op: BinOp,
        lhs: RawOperand,
        rhs: RawOperand,
    },
    Alloca {
        pointee: TypeId,
    },
    Load {
        ty: TypeId,
        pointer: RawOperand,
    },
    Store {
        pointer: RawOperand,
        value: RawOperand,
    },
    PtrOffset {
        pointer: RawOperand,
        offset: RawOperand,
    },
    Conv {
        op: ConvOp,
        to: TypeId,
        value: RawOperand,
    },
    Call {
        callee: String,
        args: Vec<RawOperand>,
        line: u32,
    },
    Construct {
        ty: TypeId,
        fields: Vec<RawOperand>,
    },
    Extract {
        aggregate: RawOperand,
        index: u32,
    },
    Insert {
        aggregate: RawOperand,
        index: u32,
        value: RawOperand,
    },
}

struct RawInst {
    kind: RawInstKind,
    result: Option<(String, TypeId, u32)>,
    line: u32,
}

enum RawTerm {
    Jump {
        target: String,
        args: Vec<RawOperand>,
    },
    Branch {
        condition: RawOperand,
        then_target: String,
        then_args: Vec<RawOperand>,
        else_target: String,
        else_args: Vec<RawOperand>,
    },
    Return {
        value: Option<RawOperand>,
    },
    Unreachable,
}

struct RawBlock {
    label: String,
    params: Vec<(String, TypeId, u32)>,
    insts: Vec<RawInst>,
    terminator: Option<RawTerm>,
    line: u32,
}

struct RawParam {
    handle: Option<String>,
    ty: TypeId,
}

struct RawFunction {
    name: String,
    linkage: Linkage,
    abi: Abi,
    params: Vec<RawParam>,
    result: TypeId,
    variadic: bool,
    blocks: Vec<RawBlock>,
    line: u32,
}

/// Parses `.arn` text into a [`Module`].
pub fn parse(source: &str) -> Result<Module, ParseError> {
    let tokens = lex(source)?;
    let mut parser = Parser {
        tokens,
        pos: 0,
        module: Module::new(),
        functions: Vec::new(),
        function_index: HashMap::new(),
    };
    parser.parse_module()?;
    parser.assemble()?;
    Ok(parser.module)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    module: Module,
    functions: Vec<RawFunction>,
    function_index: HashMap<String, usize>,
}

fn unexpected<T>(p: &Parser, expected: &str) -> Result<T, ParseError> {
    if p.pos >= p.tokens.len() {
        Err(ParseError::UnexpectedEof {
            expected: expected.to_string(),
        })
    } else {
        Err(ParseError::UnexpectedToken {
            line: p.tokens[p.pos].line,
            expected: expected.to_string(),
            found: Parser::describe(&p.tokens[p.pos].kind),
        })
    }
}

impl Parser {
    fn describe(kind: &TokenKind) -> String {
        match kind {
            TokenKind::Marker(m) => format!("`::ASTRONOMY::{m}`"),
            TokenKind::ValueRef(v) => format!("`%{v}`"),
            TokenKind::FuncRef(v) => format!("`@{v}`"),
            TokenKind::Ident(v) => format!("`{v}`"),
            TokenKind::Int(v) => format!("`{v}`"),
            TokenKind::FloatText(v) => format!("`{v}`"),
            TokenKind::Str(_) => "a string literal".to_string(),
            TokenKind::Punct(c) => format!("`{c}`"),
            TokenKind::Arrow => "`->`".to_string(),
            TokenKind::Ellipsis => "`...`".to_string(),
        }
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos).map(|t| &t.kind)
    }

    fn line(&self) -> u32 {
        self.tokens.get(self.pos).map(|t| t.line).unwrap_or(0)
    }

    fn expect_marker(&mut self, name: &str) -> Result<u32, ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Marker(m)) if m == name => {
                let line = self.tokens[self.pos].line;
                self.pos += 1;
                Ok(line)
            }
            _ => unexpected(self, &format!("`::ASTRONOMY::{name}`")),
        }
    }

    fn expect_punct(&mut self, c: char) -> Result<(), ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Punct(p)) if *p == c => {
                self.pos += 1;
                Ok(())
            }
            _ => unexpected(self, &format!("`{c}`")),
        }
    }

    fn expect_arrow(&mut self) -> Result<(), ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Arrow) => {
                self.pos += 1;
                Ok(())
            }
            _ => unexpected(self, "`->`"),
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, u32), ParseError> {
        match self.peek_kind().cloned() {
            Some(TokenKind::Ident(v)) => {
                let line = self.tokens[self.pos].line;
                self.pos += 1;
                Ok((v, line))
            }
            _ => unexpected(self, &format!("an identifier ({what})")),
        }
    }

    fn expect_int(&mut self, what: &str) -> Result<(i128, u32), ParseError> {
        match self.peek_kind().cloned() {
            Some(TokenKind::Int(v)) => {
                let line = self.tokens[self.pos].line;
                self.pos += 1;
                Ok((v, line))
            }
            _ => unexpected(self, &format!("an integer ({what})")),
        }
    }

    fn expect_str(&mut self) -> Result<(Vec<u8>, u32), ParseError> {
        match self.peek_kind().cloned() {
            Some(TokenKind::Str(v)) => {
                let line = self.tokens[self.pos].line;
                self.pos += 1;
                Ok((v, line))
            }
            _ => unexpected(self, "a string literal"),
        }
    }

    fn peek_is_punct(&self, c: char) -> bool {
        matches!(self.peek_kind(), Some(TokenKind::Punct(p)) if *p == c)
    }

    fn peek_is_ident(&self, name: &str) -> bool {
        matches!(self.peek_kind(), Some(TokenKind::Ident(v)) if v == name)
    }

    // -- module --------------------------------------------------------------

    fn parse_module(&mut self) -> Result<(), ParseError> {
        self.expect_marker("MODULE_START")?;
        loop {
            if self.pos >= self.tokens.len() {
                return Err(ParseError::UnexpectedEof {
                    expected: "`::ASTRONOMY::MODULE_END`".to_string(),
                });
            }
            let name = match self.peek_kind() {
                Some(TokenKind::Marker(m)) => m.clone(),
                other => {
                    return Err(ParseError::UnexpectedToken {
                        line: self.line(),
                        expected: "a module directive".to_string(),
                        found: Self::describe(other.unwrap_or(&TokenKind::Ellipsis)),
                    })
                }
            };
            match name.as_str() {
                "MODULE_NAME" => {
                    self.pos += 1;
                    let (bytes, _) = self.expect_str()?;
                    let text =
                        String::from_utf8(bytes).map_err(|_| ParseError::InvalidString {
                            line: self.line(),
                            reason: "module name must be valid UTF-8".to_string(),
                        })?;
                    self.module.set_name(&text);
                }
                "MODULE_VERSION" => {
                    self.pos += 1;
                    let (major, line) = self.expect_int("a version number")?;
                    if !(0..=u32::MAX as i128).contains(&major) {
                        return Err(ParseError::InvalidNumber {
                            line,
                            reason: "version out of range".to_string(),
                        });
                    }
                    if major as u32 != ArnVersion::SUPPORTED_MAJOR {
                        return Err(ParseError::UnsupportedVersion {
                            line,
                            found: major as u32,
                            supported: ArnVersion::SUPPORTED_MAJOR,
                        });
                    }
                    self.module.set_version(ArnVersion {
                        major: major as u32,
                        minor: 0,
                    });
                }
                "CONSTANTS_START" => {
                    self.pos += 1;
                    self.parse_constants()?;
                }
                "FUNCTION_START" => {
                    let f = self.parse_function()?;
                    if self.function_index.contains_key(&f.name) {
                        return Err(ParseError::DuplicateFunction {
                            line: f.line,
                            name: f.name.clone(),
                        });
                    }
                    self.function_index.insert(f.name.clone(), self.functions.len());
                    self.functions.push(f);
                }
                "MODULE_END" => {
                    self.pos += 1;
                    if self.pos < self.tokens.len() {
                        return Err(ParseError::UnexpectedToken {
                            line: self.line(),
                            expected: "end of input after `::ASTRONOMY::MODULE_END`".to_string(),
                            found: Self::describe(&self.tokens[self.pos].kind.clone()),
                        });
                    }
                    return Ok(());
                }
                other => {
                    return Err(ParseError::UnknownDirective {
                        line: self.line(),
                        name: other.to_string(),
                    })
                }
            }
        }
    }

    // -- constants -----------------------------------------------------------

    fn parse_constant_ref(&mut self) -> Result<ConstantId, ParseError> {
        let (text, line) = self.expect_ident("a constant reference (`cN`)")?;
        let index = parse_cref(&text).ok_or_else(|| ParseError::UnexpectedToken {
            line,
            expected: "a constant reference (`cN`)".to_string(),
            found: format!("`{text}`"),
        })?;
        if index as usize >= self.module.constants().len() {
            return Err(ParseError::UndefinedConstant { line, name: text });
        }
        Ok(ConstantId::new(index))
    }

    fn expect_fresh_constant_ref(&mut self) -> Result<u32, ParseError> {
        let (text, line) = self.expect_ident("a new constant id (`cN`)")?;
        let index = parse_cref(&text).ok_or_else(|| ParseError::UnexpectedToken {
            line,
            expected: "a new constant id (`cN`)".to_string(),
            found: format!("`{text}`"),
        })?;
        if index as usize != self.module.constants().len() {
            return Err(ParseError::MisplacedDirective {
                line,
                name: format!("CONSTANT_* c{index}"),
                reason: format!(
                    "constants must be defined in index order; expected c{}",
                    self.module.constants().len()
                ),
            });
        }
        Ok(index)
    }

    fn parse_constants(&mut self) -> Result<(), ParseError> {
        loop {
            if self.pos >= self.tokens.len() {
                return Err(ParseError::UnexpectedEof {
                    expected: "`::ASTRONOMY::CONSTANTS_END`".to_string(),
                });
            }
            let name = match self.peek_kind() {
                Some(TokenKind::Marker(m)) => m.clone(),
                other => {
                    return Err(ParseError::UnexpectedToken {
                        line: self.line(),
                        expected: "a constant directive".to_string(),
                        found: Self::describe(other.unwrap_or(&TokenKind::Ellipsis)),
                    })
                }
            };
            match name.as_str() {
                "CONSTANT_INT" => {
                    self.pos += 1;
                    let _ = self.expect_fresh_constant_ref()?;
                    let ty = self.parse_type()?;
                    let (value, line) = self.expect_int("an integer value")?;
                    let width = match self.module.types().get(ty) {
                        Some(crate::types::TypeData::Int { bits, .. }) => *bits,
                        _ => {
                            return Err(ParseError::InvalidType {
                                line,
                                reason: format!(
                                    "`{}` is not an integer type",
                                    self.module.type_name(ty)
                                ),
                            })
                        }
                    };
                    let bits = crate::constant::ConstantStore::mask_signed(value, width);
                    self.module.intern_constant(crate::constant::ConstantData::Int {
                        ty,
                        width,
                        bits,
                    });
                }
                "CONSTANT_FLOAT" => {
                    self.pos += 1;
                    let _ = self.expect_fresh_constant_ref()?;
                    let ty = self.parse_type()?;
                    let line = self.line();
                    let raw = match self.peek_kind().cloned() {
                        Some(TokenKind::FloatText(t)) => {
                            self.pos += 1;
                            t
                        }
                        Some(TokenKind::Int(0)) => {
                            self.pos += 1;
                            "0.0".to_string()
                        }
                        _ => return unexpected(self, "a float value"),
                    };
                    let is_f32 = ty == TypeId::F32;
                    if !is_f32 && ty != TypeId::F64 {
                        return Err(ParseError::InvalidType {
                            line,
                            reason: format!("`{}` is not a float type", self.module.type_name(ty)),
                        });
                    }
                    let bits = parse_float_bits(&raw, is_f32, line)?;
                    self.module.intern_constant(crate::constant::ConstantData::Float {
                        ty,
                        bits,
                    });
                }
                "CONSTANT_NULL" => {
                    self.pos += 1;
                    let _ = self.expect_fresh_constant_ref()?;
                    let ty = self.parse_type()?;
                    let line = self.line();
                    match self.module.types().get(ty) {
                        Some(crate::types::TypeData::Pointer { pointee, .. }) => {
                            let pointee = *pointee;
                            self.module.intern_constant(crate::constant::ConstantData::Null {
                                pointee,
                            });
                        }
                        _ => {
                            return Err(ParseError::InvalidType {
                                line,
                                reason: format!(
                                    "`{}` is not a pointer type",
                                    self.module.type_name(ty)
                                ),
                            })
                        }
                    }
                }
                "CONSTANT_STRING" => {
                    self.pos += 1;
                    let _ = self.expect_fresh_constant_ref()?;
                    let (bytes, _) = self.expect_str()?;
                    self.module
                        .intern_constant(crate::constant::ConstantData::String { bytes });
                }
                "CONSTANT_AGGREGATE" => {
                    self.pos += 1;
                    let _ = self.expect_fresh_constant_ref()?;
                    let ty = self.parse_type()?;
                    let line = self.line();
                    let expected = aggregate_field_types(&self.module, ty).ok_or_else(|| {
                        ParseError::InvalidType {
                            line,
                            reason: format!(
                                "`{}` is not an aggregate type",
                                self.module.type_name(ty)
                            ),
                        }
                    })?;
                    let mut elements = Vec::new();
                    while matches!(self.peek_kind(), Some(TokenKind::Ident(t))
                        if parse_cref(t).is_some())
                    {
                        elements.push(self.parse_constant_ref()?);
                    }
                    if elements.len() != expected.len() {
                        return Err(ParseError::InvalidStructure {
                            line,
                            reason: format!(
                                "aggregate constant of `{}` needs {} element(s), found {}",
                                self.module.type_name(ty),
                                expected.len(),
                                elements.len()
                            ),
                        });
                    }
                    for (i, (&cid, &field)) in elements.iter().zip(expected.iter()).enumerate() {
                        let data = self.module.constants().get(cid).unwrap();
                        if !crate::error::constant_fits_type(self.module.types(), data, field) {
                            return Err(ParseError::InvalidStructure {
                                line,
                                reason: format!(
                                    "aggregate element {i} does not match field type `{}`",
                                    self.module.type_name(field)
                                ),
                            });
                        }
                    }
                    self.module.intern_constant(crate::constant::ConstantData::Aggregate {
                        ty,
                        elements,
                    });
                }
                "CONSTANTS_END" => {
                    self.pos += 1;
                    return Ok(());
                }
                other => {
                    return Err(ParseError::MisplacedDirective {
                        line: self.line(),
                        name: other.to_string(),
                        reason: "expected a constant directive or `CONSTANTS_END`".to_string(),
                    })
                }
            }
        }
    }

    // -- types ---------------------------------------------------------------

    fn parse_type(&mut self) -> Result<TypeId, ParseError> {
        let (word, line) = self.expect_ident("a type")?;
        let ty = match word.as_str() {
            "void" => TypeId::VOID,
            "i1" => TypeId::I1,
            "i8" => TypeId::I8,
            "i16" => TypeId::I16,
            "i32" => TypeId::I32,
            "i64" => TypeId::I64,
            "i128" => TypeId::I128,
            "u8" => TypeId::U8,
            "u16" => TypeId::U16,
            "u32" => TypeId::U32,
            "u64" => TypeId::U64,
            "u128" => TypeId::U128,
            "f32" => TypeId::F32,
            "f64" => TypeId::F64,
            "ptr" => {
                self.expect_punct('<')?;
                let pointee = self.parse_type()?;
                let address_space = if self.peek_is_punct(',') {
                    self.pos += 1;
                    let (n, nline) = self.expect_int("an address space")?;
                    if !(0..=u32::MAX as i128).contains(&n) {
                        return Err(ParseError::InvalidNumber {
                            line: nline,
                            reason: "address space out of range".to_string(),
                        });
                    }
                    n as u32
                } else {
                    0
                };
                self.expect_punct('>')?;
                return Ok(self.module.types_mut().ptr_in(pointee, address_space));
            }
            "array" => {
                self.expect_punct('<')?;
                let element = self.parse_type()?;
                self.expect_punct(',')?;
                let (n, nline) = self.expect_int("an array length")?;
                if !(0..=u64::MAX as i128).contains(&n) {
                    return Err(ParseError::InvalidNumber {
                        line: nline,
                        reason: "array length out of range".to_string(),
                    });
                }
                self.expect_punct('>')?;
                return Ok(self.module.types_mut().array(element, n as u64));
            }
            "struct" => {
                self.expect_punct('<')?;
                let mut fields = Vec::new();
                if !self.peek_is_punct('>') {
                    fields.push(self.parse_type()?);
                    while self.peek_is_punct(',') {
                        self.pos += 1;
                        fields.push(self.parse_type()?);
                    }
                }
                self.expect_punct('>')?;
                return Ok(self.module.types_mut().struct_of(&fields));
            }
            "fn" => {
                self.expect_punct('(')?;
                let mut params = Vec::new();
                let mut variadic = false;
                loop {
                    if self.peek_is_punct(')') {
                        self.pos += 1;
                        break;
                    }
                    if matches!(self.peek_kind(), Some(TokenKind::Ellipsis)) {
                        self.pos += 1;
                        variadic = true;
                        self.expect_punct(')')?;
                        break;
                    }
                    params.push(self.parse_type()?);
                    // Optional parameter handle: meaningful only in
                    // FUNCTION_START headers, skipped for plain types.
                    if matches!(self.peek_kind(), Some(TokenKind::ValueRef(_))) {
                        self.pos += 1;
                    }
                    if self.peek_is_punct(',') {
                        self.pos += 1;
                    } else {
                        self.expect_punct(')')?;
                        break;
                    }
                }
                self.expect_arrow()?;
                let result = self.parse_type()?;
                return Ok(self.module.types_mut().function(&params, variadic, result));
            }
            other => {
                return Err(ParseError::InvalidType {
                    line,
                    reason: format!("unknown type `{other}`"),
                })
            }
        };
        Ok(ty)
    }

    // -- functions -----------------------------------------------------------

    fn parse_function(&mut self) -> Result<RawFunction, ParseError> {
        let line = self.line();
        self.pos += 1; // FUNCTION_START
        let (name, name_line) = self.expect_ident("a function name")?;
        if !crate::builder::is_valid_ident(&name) {
            return Err(ParseError::ReservedName {
                line: name_line,
                name,
                reason: "function names must be identifiers".to_string(),
            });
        }

        // Signature: fn(...) -> result
        let (kw, kw_line) = self.expect_ident("`fn`")?;
        if kw != "fn" {
            return Err(ParseError::UnexpectedToken {
                line: kw_line,
                expected: "`fn`".to_string(),
                found: format!("`{kw}`"),
            });
        }
        self.expect_punct('(')?;
        let mut params = Vec::new();
        let mut variadic = false;
        loop {
            if self.peek_is_punct(')') {
                self.pos += 1;
                break;
            }
            if matches!(self.peek_kind(), Some(TokenKind::Ellipsis)) {
                self.pos += 1;
                variadic = true;
                self.expect_punct(')')?;
                break;
            }
            let ty = self.parse_type()?;
            let handle = match self.peek_kind().cloned() {
                Some(TokenKind::ValueRef(v)) => {
                    self.pos += 1;
                    Some(v)
                }
                _ => None,
            };
            params.push(RawParam { handle, ty });
            if self.peek_is_punct(',') {
                self.pos += 1;
            } else {
                self.expect_punct(')')?;
                break;
            }
        }
        self.expect_arrow()?;
        let result = self.parse_type()?;

        // Attributes.
        let mut linkage = Linkage::External;
        let mut abi = Abi::Astronomy;
        while self.peek_is_ident("linkage") || self.peek_is_ident("abi") {
            if self.peek_is_ident("linkage") {
                self.pos += 1;
                self.expect_punct('=')?;
                let (value, vline) = self.expect_ident("a linkage")?;
                linkage = match value.as_str() {
                    "internal" => Linkage::Internal,
                    "external" => Linkage::External,
                    "exported" => Linkage::Exported,
                    _ => {
                        return Err(ParseError::InvalidStructure {
                            line: vline,
                            reason: format!("unknown linkage `{value}`"),
                        })
                    }
                };
            } else {
                self.pos += 1;
                self.expect_punct('=')?;
                let (value, vline) = self.expect_ident("an ABI")?;
                abi = match value.as_str() {
                    "astronomy" => Abi::Astronomy,
                    "c" => Abi::C,
                    "system" => Abi::System,
                    "custom" => {
                        let (custom, cline) = self.expect_ident("a custom ABI name")?;
                        if !crate::builder::is_valid_ident(&custom) {
                            return Err(ParseError::ReservedName {
                                line: cline,
                                name: custom,
                                reason: "ABI names must be identifiers".to_string(),
                            });
                        }
                        Abi::Custom(self.module.symbols_mut().intern(custom))
                    }
                    _ => {
                        return Err(ParseError::InvalidStructure {
                            line: vline,
                            reason: format!("unknown ABI `{value}`"),
                        })
                    }
                };
            }
        }

        // Body: blocks until FUNCTION_END.
        let mut blocks = Vec::new();
        loop {
            if self.pos >= self.tokens.len() {
                return Err(ParseError::UnexpectedEof {
                    expected: "`::ASTRONOMY::FUNCTION_END`".to_string(),
                });
            }
            let marker = match self.peek_kind() {
                Some(TokenKind::Marker(m)) => m.clone(),
                other => {
                    return Err(ParseError::UnexpectedToken {
                        line: self.line(),
                        expected: "`::ASTRONOMY::BLOCK_START` or `::ASTRONOMY::FUNCTION_END`"
                            .to_string(),
                        found: Self::describe(other.unwrap_or(&TokenKind::Ellipsis)),
                    })
                }
            };
            match marker.as_str() {
                "BLOCK_START" => blocks.push(self.parse_block()?),
                "FUNCTION_END" => {
                    self.pos += 1;
                    return Ok(RawFunction {
                        name,
                        linkage,
                        abi,
                        params,
                        result,
                        variadic,
                        blocks,
                        line,
                    });
                }
                other => {
                    return Err(ParseError::MisplacedDirective {
                        line: self.line(),
                        name: other.to_string(),
                        reason: "expected a block or the end of the function".to_string(),
                    })
                }
            }
        }
    }

    fn parse_block(&mut self) -> Result<RawBlock, ParseError> {
        let line = self.line();
        self.pos += 1; // BLOCK_START
        let (label, _) = self.expect_ident("a block label")?;

        let mut params = Vec::new();
        if self.peek_is_punct('(') {
            self.pos += 1;
            loop {
                if self.peek_is_punct(')') {
                    self.pos += 1;
                    break;
                }
                let ty = self.parse_type()?;
                let (handle, hline) = match self.peek_kind().cloned() {
                    Some(TokenKind::ValueRef(v)) => {
                        let l = self.tokens[self.pos].line;
                        self.pos += 1;
                        (v, l)
                    }
                    _ => return unexpected(self, "a block parameter handle"),
                };
                params.push((handle, ty, hline));
                if self.peek_is_punct(',') {
                    self.pos += 1;
                } else {
                    self.expect_punct(')')?;
                    break;
                }
            }
        }

        let mut insts = Vec::new();
        let mut terminator: Option<RawTerm> = None;
        loop {
            if self.pos >= self.tokens.len() {
                return Err(ParseError::UnexpectedEof {
                    expected: "`::ASTRONOMY::BLOCK_END`".to_string(),
                });
            }
            match self.peek_kind() {
                Some(TokenKind::Marker(m)) if m == "BLOCK_END" => {
                    self.pos += 1;
                    return Ok(RawBlock {
                        label,
                        params,
                        insts,
                        terminator,
                        line,
                    });
                }
                Some(TokenKind::Marker(m)) if is_terminator_marker(m) => {
                    let marker = m.clone();
                    if terminator.is_some() {
                        return Err(ParseError::InstructionAfterTerminator {
                            line: self.line(),
                        });
                    }
                    terminator = Some(self.parse_terminator(&marker)?);
                }
                Some(TokenKind::ValueRef(_)) => {
                    if terminator.is_some() {
                        return Err(ParseError::InstructionAfterTerminator {
                            line: self.line(),
                        });
                    }
                    insts.push(self.parse_instruction()?);
                }
                Some(TokenKind::Marker(_)) => {
                    // Any other marker starts a void instruction (STORE,
                    // CALL void). Unknown opcodes are rejected by
                    // `parse_instruction` with the opcode name attached.
                    if terminator.is_some() {
                        return Err(ParseError::InstructionAfterTerminator {
                            line: self.line(),
                        });
                    }
                    insts.push(self.parse_instruction()?);
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        line: self.line(),
                        expected:
                            "an instruction, a terminator or `::ASTRONOMY::BLOCK_END`".to_string(),
                        found: Self::describe(other.unwrap_or(&TokenKind::Ellipsis)),
                    });
                }
            }
        }
    }

    fn parse_operand(&mut self) -> Result<RawOperand, ParseError> {
        let ty = self.parse_type()?;
        let (handle, line) = match self.peek_kind().cloned() {
            Some(TokenKind::ValueRef(v)) => {
                let l = self.tokens[self.pos].line;
                self.pos += 1;
                (v, l)
            }
            _ => return unexpected(self, "a value handle (`%name`)"),
        };
        Ok(RawOperand { ty, handle, line })
    }

    fn parse_parenthesized_operands(&mut self) -> Result<Vec<RawOperand>, ParseError> {
        self.expect_punct('(')?;
        let mut args = Vec::new();
        loop {
            if self.peek_is_punct(')') {
                self.pos += 1;
                break;
            }
            args.push(self.parse_operand()?);
            if self.peek_is_punct(',') {
                self.pos += 1;
            } else {
                self.expect_punct(')')?;
                break;
            }
        }
        Ok(args)
    }

    fn parse_branch_args(&mut self) -> Result<Vec<RawOperand>, ParseError> {
        if self.peek_is_punct('(') {
            self.parse_parenthesized_operands()
        } else {
            Ok(Vec::new())
        }
    }

    fn parse_terminator(&mut self, marker: &str) -> Result<RawTerm, ParseError> {
        self.pos += 1;
        match marker {
            "JUMP" => {
                let (label, _) = self.expect_ident("a block label")?;
                let args = self.parse_branch_args()?;
                Ok(RawTerm::Jump { target: label, args })
            }
            "BRANCH" => {
                let condition = self.parse_operand()?;
                self.expect_punct(',')?;
                let (then_label, _) = self.expect_ident("a block label")?;
                let then_args = self.parse_branch_args()?;
                self.expect_punct(',')?;
                let (else_label, _) = self.expect_ident("a block label")?;
                let else_args = self.parse_branch_args()?;
                Ok(RawTerm::Branch {
                    condition,
                    then_target: then_label,
                    then_args,
                    else_target: else_label,
                    else_args,
                })
            }
            "RETURN" => {
                let ty = self.parse_type()?;
                if ty == TypeId::VOID {
                    Ok(RawTerm::Return { value: None })
                } else {
                    let (handle, line) = match self.peek_kind().cloned() {
                        Some(TokenKind::ValueRef(v)) => {
                            let l = self.tokens[self.pos].line;
                            self.pos += 1;
                            (v, l)
                        }
                        _ => return unexpected(self, "a value handle"),
                    };
                    Ok(RawTerm::Return {
                        value: Some(RawOperand { ty, handle, line }),
                    })
                }
            }
            "UNREACHABLE" => Ok(RawTerm::Unreachable),
            _ => unreachable!("caller checked the marker"),
        }
    }

    fn parse_instruction(&mut self) -> Result<RawInst, ParseError> {
        let line = self.line();
        let result_handle = match self.peek_kind().cloned() {
            Some(TokenKind::ValueRef(handle)) => {
                self.pos += 1;
                self.expect_punct('=')?;
                Some(handle)
            }
            _ => None,
        };

        let (marker, mline) = match self.peek_kind().cloned() {
            Some(TokenKind::Marker(m)) => {
                let l = self.tokens[self.pos].line;
                self.pos += 1;
                (m, l)
            }
            _ => return unexpected(self, "an instruction opcode (`::ASTRONOMY::OP`)"),
        };

        // Result annotation: instructions with a result handle annotate the
        // result type right after the opcode. `CALL void` carries `void`
        // instead of a handle; `STORE` has no result at all.
        let mut result_decl = None;
        match result_handle {
            Some(handle) => {
                if marker == "STORE" {
                    return Err(ParseError::InvalidInstruction {
                        line,
                        reason: "store must not have a result".to_string(),
                    });
                }
                if marker == "CALL" && self.peek_is_ident("void") {
                    return Err(ParseError::InvalidInstruction {
                        line,
                        reason: "a call returning void must not have a result".to_string(),
                    });
                }
                let ty = self.parse_type()?;
                result_decl = Some((handle, ty, line));
            }
            None => {
                if marker == "CALL" {
                    if !self.peek_is_ident("void") {
                        return Err(ParseError::InvalidInstruction {
                            line,
                            reason: format!(
                                "`CALL` requires a result (`%handle = ...`) unless it returns void"
                            ),
                        });
                    }
                    self.pos += 1; // consume `void`
                } else if marker != "STORE" {
                    return Err(ParseError::InvalidInstruction {
                        line,
                        reason: format!("`{marker}` requires a result (`%handle = ...`)"),
                    });
                }
            }
        }

        let kind = match marker.as_str() {
            "CONST" => {
                let cid = self.parse_constant_ref()?;
                RawInstKind::Const(cid)
            }
            "ALLOCA" => {
                let pointee = self.parse_type()?;
                RawInstKind::Alloca { pointee }
            }
            "LOAD" => {
                // The result type annotation (consumed above) is the loaded
                // type; only the pointer operand remains.
                let pointer = self.parse_operand()?;
                let ty = result_decl.as_ref().map(|(_, t, _)| *t).unwrap_or(TypeId::VOID);
                RawInstKind::Load { ty, pointer }
            }
            "STORE" => {
                let pointer = self.parse_operand()?;
                self.expect_punct(',')?;
                let value = self.parse_operand()?;
                RawInstKind::Store { pointer, value }
            }
            "PTR_OFFSET" => {
                let pointer = self.parse_operand()?;
                self.expect_punct(',')?;
                let offset = self.parse_operand()?;
                RawInstKind::PtrOffset { pointer, offset }
            }
            "EXT" => self.parse_conv(ConvOp::Ext, &result_decl)?,
            "TRUNC" => self.parse_conv(ConvOp::Trunc, &result_decl)?,
            "INT_TO_FLOAT" => self.parse_conv(ConvOp::IntToFloat, &result_decl)?,
            "FLOAT_TO_INT" => self.parse_conv(ConvOp::FloatToInt, &result_decl)?,
            "PTR_CAST" => self.parse_conv(ConvOp::PtrCast, &result_decl)?,
            "CALL" => {
                let callee = match self.peek_kind().cloned() {
                    Some(TokenKind::FuncRef(name)) => {
                        self.pos += 1;
                        name
                    }
                    _ => return unexpected(self, "a callee (`@name`)"),
                };
                let args = self.parse_parenthesized_operands()?;
                RawInstKind::Call {
                    callee,
                    args,
                    line,
                }
            }
            "CONSTRUCT" => {
                // The result type annotation is the aggregate type; only
                // the field list remains.
                let fields = self.parse_parenthesized_operands()?;
                let ty = result_decl.as_ref().map(|(_, t, _)| *t).unwrap_or(TypeId::VOID);
                RawInstKind::Construct { ty, fields }
            }
            "EXTRACT" => {
                let aggregate = self.parse_operand()?;
                self.expect_punct(',')?;
                let (index, iline) = self.expect_int("a field index")?;
                if !(0..=u32::MAX as i128).contains(&index) {
                    return Err(ParseError::InvalidNumber {
                        line: iline,
                        reason: "field index out of range".to_string(),
                    });
                }
                RawInstKind::Extract {
                    aggregate,
                    index: index as u32,
                }
            }
            "INSERT" => {
                let aggregate = self.parse_operand()?;
                self.expect_punct(',')?;
                let (index, iline) = self.expect_int("a field index")?;
                if !(0..=u32::MAX as i128).contains(&index) {
                    return Err(ParseError::InvalidNumber {
                        line: iline,
                        reason: "field index out of range".to_string(),
                    });
                }
                self.expect_punct(',')?;
                let value = self.parse_operand()?;
                RawInstKind::Insert {
                    aggregate,
                    index: index as u32,
                    value,
                }
            }
            op => {
                if let Some(bin) = bin_op_of(op) {
                    let lhs = self.parse_operand()?;
                    self.expect_punct(',')?;
                    let rhs = self.parse_operand()?;
                    RawInstKind::Binary { op: bin, lhs, rhs }
                } else {
                    return Err(ParseError::UnknownDirective {
                        line: mline,
                        name: op.to_string(),
                    });
                }
            }
        };

        Ok(RawInst {
            kind,
            result: result_decl,
            line,
        })
    }

    fn parse_conv(
        &mut self,
        op: ConvOp,
        result_decl: &Option<(String, TypeId, u32)>,
    ) -> Result<RawInstKind, ParseError> {
        // The result type annotation (consumed by the caller) is the
        // conversion target; only the source operand remains.
        let value = self.parse_operand()?;
        let to = result_decl.as_ref().map(|(_, t, _)| *t).unwrap_or(TypeId::VOID);
        Ok(RawInstKind::Conv { op, to, value })
    }

    // -- assembly ------------------------------------------------------------

    fn assemble(&mut self) -> Result<(), ParseError> {
        // Snapshot of the type table for error formatting (assembly never
        // interns new types).
        let types = self.module.types().clone();

        // Create all function skeletons first so calls can reference any
        // function regardless of text order. Digit-only handles are the
        // printer's anonymous numbering and do not become debug names.
        let mut functions_by_name: HashMap<String, FunctionId> = HashMap::new();
        for raw in &self.functions {
            let symbol = self.module.symbols_mut().intern(raw.name.clone());
            let params: Vec<(Option<crate::id::SymbolId>, TypeId)> = raw
                .params
                .iter()
                .map(|p| {
                    let name = p
                        .handle
                        .as_ref()
                        .filter(|h| !is_anonymous_handle(h))
                        .map(|h| self.module.symbols_mut().intern(h.clone()));
                    (name, p.ty)
                })
                .collect();
            let function =
                Function::new(symbol, raw.linkage, raw.abi, params, raw.result, raw.variadic);
            self.module.functions_mut().push(function);
            let id = FunctionId::new(self.module.functions().len() as u32 - 1);
            functions_by_name.insert(raw.name.clone(), id);
        }

        for i in 0..self.functions.len() {
            let raw = &self.functions[i];
            let id = functions_by_name[&raw.name];
            Self::assemble_function(&mut self.module, raw, id, &functions_by_name, &types)?;
        }
        Ok(())
    }

    fn assemble_function(
        module: &mut Module,
        raw: &RawFunction,
        id: FunctionId,
        functions_by_name: &HashMap<String, FunctionId>,
        types: &crate::types::TypeStore,
    ) -> Result<(), ParseError> {
        let mut values: HashMap<String, ValueId> = HashMap::new();
        let mut labels: HashMap<String, BlockId> = HashMap::new();

        // Pre-pass: intern debug names (block labels, named block params and
        // named instruction results). Digit-only handles and `bb<digits>`
        // labels are the printer's anonymous numbering and stay anonymous.
        let mut debug_names: HashMap<String, crate::id::SymbolId> = HashMap::new();
        for rb in &raw.blocks {
            if !crate::builder::is_reserved_block_name(&rb.label) {
                let label_sym = module.symbols_mut().intern(rb.label.clone());
                debug_names.insert(rb.label.clone(), label_sym);
            }
            for (handle, _, _) in &rb.params {
                if !is_anonymous_handle(handle) {
                    let sym = module.symbols_mut().intern(handle.clone());
                    debug_names.insert(handle.clone(), sym);
                }
            }
            for inst in &rb.insts {
                if let Some((handle, _, _)) = &inst.result {
                    if !is_anonymous_handle(handle) {
                        let sym = module.symbols_mut().intern(handle.clone());
                        debug_names.insert(handle.clone(), sym);
                    }
                }
            }
        }

        // Pass 1: create blocks, block params and instruction results.
        {
            let f = module.function_mut(id).unwrap();
            for (i, p) in raw.params.iter().enumerate() {
                if let Some(handle) = &p.handle {
                    if values.contains_key(handle) {
                        return Err(ParseError::DuplicateValue {
                            line: raw.line,
                            name: handle.clone(),
                        });
                    }
                    values.insert(handle.clone(), f.params[i].value);
                }
            }
        }

        for rb in &raw.blocks {
            if labels.contains_key(&rb.label) {
                return Err(ParseError::DuplicateBlock {
                    line: rb.line,
                    label: rb.label.clone(),
                });
            }
            let block = BlockId::new(module.function(id).unwrap().blocks.len() as u32);
            module
                .function_mut(id)
                .unwrap()
                .blocks
                .push(BasicBlock::new());
            if let Some(sym) = debug_names.get(&rb.label) {
                module.function_mut(id).unwrap().blocks[block.index()].name = Some(*sym);
            }
            labels.insert(rb.label.clone(), block);

            let f = module.function_mut(id).unwrap();
            for (index, (handle, ty, line)) in rb.params.iter().enumerate() {
                if values.contains_key(handle) {
                    return Err(ParseError::DuplicateValue {
                        line: *line,
                        name: handle.clone(),
                    });
                }
                let value = ValueId::new(f.values.len() as u32);
                f.values.push(ValueData {
                    ty: *ty,
                    kind: ValueKind::BlockParam {
                        block,
                        index: index as u32,
                    },
                    name: debug_names.get(handle).copied(),
                    span: Some(Span::new(*line, 1)),
                });
                f.blocks[block.index()].params.push(BlockParam {
                    value,
                    ty: *ty,
                });
                values.insert(handle.clone(), value);
            }
            for (ii, inst) in rb.insts.iter().enumerate() {
                if let Some((handle, ty, line)) = &inst.result {
                    if values.contains_key(handle) {
                        return Err(ParseError::DuplicateValue {
                            line: *line,
                            name: handle.clone(),
                        });
                    }
                    let value = ValueId::new(f.values.len() as u32);
                    f.values.push(ValueData {
                        ty: *ty,
                        kind: ValueKind::Inst {
                            block,
                            index: ii as u32,
                        },
                        name: debug_names.get(handle).copied(),
                        span: Some(Span::new(*line, 1)),
                    });
                    values.insert(handle.clone(), value);
                }
            }
        }

        // Pass 2: resolve operands and build instructions/terminators.
        let resolve =
            |values: &HashMap<String, ValueId>, f: &Function, op: &RawOperand| -> Result<ValueId, ParseError> {
                let v = values
                    .get(&op.handle)
                    .ok_or_else(|| ParseError::UndefinedValue {
                        line: op.line,
                        name: op.handle.clone(),
                    })?;
                let actual = f.value(*v).map(|d| d.ty).unwrap_or(TypeId::VOID);
                if actual != op.ty {
                    return Err(ParseError::OperandTypeMismatch {
                        line: op.line,
                        annotated: crate::types::type_to_string(types, op.ty),
                        actual: crate::types::type_to_string(types, actual),
                    });
                }
                Ok(*v)
            };

        for rb in &raw.blocks {
            let block = labels[&rb.label];
            let f = module.function_mut(id).unwrap();
            for inst in &rb.insts {
                let kind = match &inst.kind {
                    RawInstKind::Const(cid) => InstructionKind::Const(*cid),
                    RawInstKind::Binary { op, lhs, rhs } => {
                        let l = resolve(&values, f, lhs)?;
                        let r = resolve(&values, f, rhs)?;
                        match op {
                            BinOp::Add => InstructionKind::Add { lhs: l, rhs: r },
                            BinOp::Sub => InstructionKind::Sub { lhs: l, rhs: r },
                            BinOp::Mul => InstructionKind::Mul { lhs: l, rhs: r },
                            BinOp::Div => InstructionKind::Div { lhs: l, rhs: r },
                            BinOp::Rem => InstructionKind::Rem { lhs: l, rhs: r },
                            BinOp::And => InstructionKind::And { lhs: l, rhs: r },
                            BinOp::Or => InstructionKind::Or { lhs: l, rhs: r },
                            BinOp::Xor => InstructionKind::Xor { lhs: l, rhs: r },
                            BinOp::Shl => InstructionKind::Shl { lhs: l, rhs: r },
                            BinOp::Shr => InstructionKind::Shr { lhs: l, rhs: r },
                            BinOp::Eq => InstructionKind::Eq { lhs: l, rhs: r },
                            BinOp::Ne => InstructionKind::Ne { lhs: l, rhs: r },
                            BinOp::Lt => InstructionKind::Lt { lhs: l, rhs: r },
                            BinOp::Le => InstructionKind::Le { lhs: l, rhs: r },
                            BinOp::Gt => InstructionKind::Gt { lhs: l, rhs: r },
                            BinOp::Ge => InstructionKind::Ge { lhs: l, rhs: r },
                        }
                    }
                    RawInstKind::Alloca { pointee } => InstructionKind::Alloca {
                        pointee: *pointee,
                    },
                    RawInstKind::Load { ty, pointer } => InstructionKind::Load {
                        ty: *ty,
                        pointer: resolve(&values, f, pointer)?,
                    },
                    RawInstKind::Store { pointer, value } => InstructionKind::Store {
                        pointer: resolve(&values, f, pointer)?,
                        value: resolve(&values, f, value)?,
                    },
                    RawInstKind::PtrOffset { pointer, offset } => InstructionKind::PtrOffset {
                        pointer: resolve(&values, f, pointer)?,
                        offset: resolve(&values, f, offset)?,
                    },
                    RawInstKind::Conv { op, to, value } => {
                        let v = resolve(&values, f, value)?;
                        match op {
                            ConvOp::Ext => InstructionKind::Ext { to: *to, value: v },
                            ConvOp::Trunc => InstructionKind::Trunc { to: *to, value: v },
                            ConvOp::IntToFloat => {
                                InstructionKind::IntToFloat { to: *to, value: v }
                            }
                            ConvOp::FloatToInt => {
                                InstructionKind::FloatToInt { to: *to, value: v }
                            }
                            ConvOp::PtrCast => InstructionKind::PtrCast { to: *to, pointer: v },
                        }
                    }
                    RawInstKind::Call { callee, args, line } => {
                        let callee_id = functions_by_name.get(callee).ok_or_else(|| {
                            ParseError::UndefinedFunction {
                                line: *line,
                                name: callee.clone(),
                            }
                        })?;
                        let mut resolved = Vec::with_capacity(args.len());
                        for a in args {
                            resolved.push(resolve(&values, f, a)?);
                        }
                        InstructionKind::Call {
                            callee: *callee_id,
                            args: resolved,
                        }
                    }
                    RawInstKind::Construct { ty, fields } => {
                        let mut resolved = Vec::with_capacity(fields.len());
                        for field in fields {
                            resolved.push(resolve(&values, f, field)?);
                        }
                        InstructionKind::Construct {
                            ty: *ty,
                            fields: resolved,
                        }
                    }
                    RawInstKind::Extract { aggregate, index } => InstructionKind::Extract {
                        aggregate: resolve(&values, f, aggregate)?,
                        index: *index,
                    },
                    RawInstKind::Insert {
                        aggregate,
                        index,
                        value,
                    } => InstructionKind::Insert {
                        aggregate: resolve(&values, f, aggregate)?,
                        index: *index,
                        value: resolve(&values, f, value)?,
                    },
                };
                let result = inst.result.as_ref().map(|(handle, _, _)| values[handle]);
                f.blocks[block.index()].instructions.push(Instruction {
                    kind,
                    result,
                    span: Some(Span::new(inst.line, 1)),
                });
            }

            let terminator = match &rb.terminator {
                None => {
                    return Err(ParseError::InvalidStructure {
                        line: rb.line,
                        reason: format!("block `{}` has no terminator", rb.label),
                    })
                }
                Some(RawTerm::Jump { target, args }) => {
                    let target = labels.get(target).ok_or_else(|| ParseError::UndefinedBlock {
                        line: rb.line,
                        label: target.clone(),
                    })?;
                    let mut resolved = Vec::with_capacity(args.len());
                    for a in args {
                        resolved.push(resolve(&values, f, a)?);
                    }
                    Terminator::Jump {
                        target: *target,
                        args: resolved,
                    }
                }
                Some(RawTerm::Branch {
                    condition,
                    then_target,
                    then_args,
                    else_target,
                    else_args,
                }) => {
                    let then_block =
                        labels.get(then_target).ok_or_else(|| ParseError::UndefinedBlock {
                            line: rb.line,
                            label: then_target.clone(),
                        })?;
                    let else_block =
                        labels.get(else_target).ok_or_else(|| ParseError::UndefinedBlock {
                            line: rb.line,
                            label: else_target.clone(),
                        })?;
                    let mut then_resolved = Vec::with_capacity(then_args.len());
                    for a in then_args {
                        then_resolved.push(resolve(&values, f, a)?);
                    }
                    let mut else_resolved = Vec::with_capacity(else_args.len());
                    for a in else_args {
                        else_resolved.push(resolve(&values, f, a)?);
                    }
                    Terminator::Branch {
                        condition: resolve(&values, f, condition)?,
                        then_block: *then_block,
                        then_args: then_resolved,
                        else_block: *else_block,
                        else_args: else_resolved,
                    }
                }
                Some(RawTerm::Return { value }) => Terminator::Return {
                    value: match value {
                        Some(op) => Some(resolve(&values, f, op)?),
                        None => None,
                    },
                },
                Some(RawTerm::Unreachable) => Terminator::Unreachable,
            };
            f.blocks[block.index()].terminator = Some(terminator);
        }
        Ok(())
    }
}

fn parse_cref(text: &str) -> Option<u32> {
    let rest = text.strip_prefix('c')?;
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// Digit-only handles are the printer's anonymous numbering (§32) and never
/// become debug names.
fn is_anonymous_handle(handle: &str) -> bool {
    !handle.is_empty() && handle.chars().all(|c| c.is_ascii_digit())
}

fn is_terminator_marker(marker: &str) -> bool {
    matches!(marker, "JUMP" | "BRANCH" | "RETURN" | "UNREACHABLE")
}

fn bin_op_of(op: &str) -> Option<BinOp> {
    Some(match op {
        "ADD" => BinOp::Add,
        "SUB" => BinOp::Sub,
        "MUL" => BinOp::Mul,
        "DIV" => BinOp::Div,
        "REM" => BinOp::Rem,
        "AND" => BinOp::And,
        "OR" => BinOp::Or,
        "XOR" => BinOp::Xor,
        "SHL" => BinOp::Shl,
        "SHR" => BinOp::Shr,
        "EQ" => BinOp::Eq,
        "NE" => BinOp::Ne,
        "LT" => BinOp::Lt,
        "LE" => BinOp::Le,
        "GT" => BinOp::Gt,
        "GE" => BinOp::Ge,
        _ => return None,
    })
}

fn aggregate_field_types(module: &Module, ty: TypeId) -> Option<Vec<TypeId>> {
    use crate::types::TypeData;
    match module.types().get(ty) {
        Some(TypeData::Array { element, length }) => Some(vec![*element; *length as usize]),
        Some(TypeData::Struct { fields }) => Some(fields.clone()),
        _ => None,
    }
}

fn parse_float_bits(raw: &str, is_f32: bool, line: u32) -> Result<u64, ParseError> {
    let invalid = || ParseError::InvalidNumber {
        line,
        reason: format!("invalid float literal `{raw}`"),
    };
    if is_f32 {
        let v = match raw {
            "nan" | "NaN" => f32::NAN,
            "inf" | "+inf" => f32::INFINITY,
            "-inf" => f32::NEG_INFINITY,
            _ => raw.parse::<f32>().map_err(|_| invalid())?,
        };
        Ok(v.to_bits() as u64)
    } else {
        let v = match raw {
            "nan" | "NaN" => f64::NAN,
            "inf" | "+inf" => f64::INFINITY,
            "-inf" => f64::NEG_INFINITY,
            _ => raw.parse::<f64>().map_err(|_| invalid())?,
        };
        Ok(v.to_bits())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_module() {
        let src = concat!(
            "::ASTRONOMY::MODULE_START\n",
            "::ASTRONOMY::MODULE_VERSION 1\n",
            "::ASTRONOMY::MODULE_END\n"
        );
        let module = parse(src).unwrap();
        assert!(module.functions().is_empty());
    }

    #[test]
    fn rejects_unknown_version() {
        let src = concat!(
            "::ASTRONOMY::MODULE_START\n",
            "::ASTRONOMY::MODULE_VERSION 2\n",
            "::ASTRONOMY::MODULE_END\n"
        );
        let err = parse(src).unwrap_err();
        assert_eq!(err.code(), "A-ARN-001");
    }

    #[test]
    fn rejects_missing_module_end() {
        let err = parse("::ASTRONOMY::MODULE_START\n").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }));
    }

    #[test]
    fn rejects_duplicate_function() {
        let src = concat!(
            "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
            "::ASTRONOMY::FUNCTION_START f fn() -> void linkage=external abi=astronomy\n",
            "::ASTRONOMY::FUNCTION_END\n",
            "::ASTRONOMY::FUNCTION_START f fn() -> void linkage=external abi=astronomy\n",
            "::ASTRONOMY::FUNCTION_END\n",
            "::ASTRONOMY::MODULE_END\n"
        );
        let err = parse(src).unwrap_err();
        assert_eq!(err.code(), "A-ARN-004");
    }

    #[test]
    fn parses_extern_declaration() {
        let src = concat!(
            "::ASTRONOMY::MODULE_START\n::ASTRONOMY::MODULE_VERSION 1\n",
            "::ASTRONOMY::FUNCTION_START printf fn(ptr<i8>, ...) -> i32 linkage=external abi=c\n",
            "::ASTRONOMY::FUNCTION_END\n",
            "::ASTRONOMY::MODULE_END\n"
        );
        let module = parse(src).unwrap();
        let f = &module.functions()[0];
        assert_eq!(module.symbol_name(f.symbol), Some("printf"));
        assert!(f.is_declaration());
        assert!(f.variadic);
        assert_eq!(f.abi, Abi::C);
    }
}
