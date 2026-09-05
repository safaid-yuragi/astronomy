//! Functions, linkage and ABI (§17 Function, §18 ABI).

use std::fmt;

use crate::block::BasicBlock;
use crate::id::{BlockId, SymbolId, TypeId, ValueId};
use crate::span::Span;
use crate::value::ValueData;

/// Symbol linkage of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Linkage {
    /// Function is internal to this module.
    Internal,
    /// Function is visible to other modules; if it has no body it is a
    /// declaration of something defined elsewhere (an *extern*).
    External,
    /// Function is exported from the final artifact.
    Exported,
}

impl Linkage {
    /// Canonical ARN keyword.
    pub fn keyword(self) -> &'static str {
        match self {
            Linkage::Internal => "internal",
            Linkage::External => "external",
            Linkage::Exported => "exported",
        }
    }
}

impl fmt::Display for Linkage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// Calling convention / ABI of a function (§18).
///
/// Backends never guess the call ABI: it is an explicit fact on every
/// function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Abi {
    /// The default Astronomy calling convention.
    Astronomy,
    /// The platform C ABI.
    C,
    /// The raw system (syscall-style) convention.
    System,
    /// A named custom convention.
    Custom(SymbolId),
}

impl Abi {
    /// Canonical ARN keyword (`custom` yields `custom <name>`; the name
    /// must be resolved separately through a [`crate::SymbolStore`]).
    pub fn keyword(&self) -> &'static str {
        match self {
            Abi::Astronomy => "astronomy",
            Abi::C => "c",
            Abi::System => "system",
            Abi::Custom(_) => "custom",
        }
    }
}

/// A function parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    /// SSA value bound to the parameter (the value's optional name is the
    /// parameter name).
    pub value: ValueId,
    /// Declared type.
    pub ty: TypeId,
}

/// A function definition or external declaration.
///
/// A function with no blocks is a *declaration* (an extern); declarations
/// must use [`Linkage::External`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// Interned symbol name.
    pub symbol: SymbolId,
    /// Linkage.
    pub linkage: Linkage,
    /// ABI / calling convention.
    pub abi: Abi,
    /// Parameters, in order. The first `params.len()` values in `values`
    /// belong to these parameters.
    pub params: Vec<Param>,
    /// Result type (may be `void`).
    pub result: TypeId,
    /// Whether calls may pass extra arguments (declarations only in the
    /// MVP).
    pub variadic: bool,
    /// Basic blocks. Block 0 is the entry block. Empty for declarations.
    pub blocks: Vec<BasicBlock>,
    /// Per-function SSA value arena.
    pub values: Vec<ValueData>,
    /// Optional source span (parsed IR only).
    pub span: Option<Span>,
}

impl Function {
    /// Creates a function skeleton with parameters allocated in the value
    /// arena.
    pub fn new(
        symbol: SymbolId,
        linkage: Linkage,
        abi: Abi,
        params: Vec<(Option<SymbolId>, TypeId)>,
        result: TypeId,
        variadic: bool,
    ) -> Self {
        let mut function = Function {
            symbol,
            linkage,
            abi,
            params: Vec::with_capacity(params.len()),
            result,
            variadic,
            blocks: Vec::new(),
            values: Vec::with_capacity(params.len() + 8),
            span: None,
        };
        for (index, (name, ty)) in params.into_iter().enumerate() {
            let value = ValueId::new(function.values.len() as u32);
            function.values.push(ValueData {
                ty,
                kind: crate::value::ValueKind::Param {
                    index: index as u32,
                },
                name,
                span: None,
            });
            function.params.push(Param { value, ty });
        }
        function
    }

    /// True when the function has no body (it is an extern declaration).
    pub fn is_declaration(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Returns the entry block (block 0), if the function has a body.
    pub fn entry(&self) -> Option<&BasicBlock> {
        self.blocks.first()
    }

    /// Looks up a block by ID.
    pub fn block(&self, id: BlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.index())
    }

    /// Looks up a value by ID.
    pub fn value(&self, id: ValueId) -> Option<&ValueData> {
        self.values.get(id.index())
    }
}


