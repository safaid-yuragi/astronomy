//! The module container: types, symbols, constants and functions (§27).

use std::fmt;
use std::ops::Deref;

use crate::constant::{ConstantData, ConstantStore};
use crate::error::ParseError;
use crate::function::Function;
use crate::id::{FunctionId, SymbolId, TypeId};
use crate::symbol::SymbolStore;
use crate::types::{type_to_string, TypeData, TypeStore};
use crate::verify::Verifier;

/// The ARN format version carried by a module (§47 Versioning).
///
/// The major version changes on incompatible grammar/semantic changes; the
/// parser rejects unknown major versions. The minor version changes
/// additively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArnVersion {
    /// Incompatible version component.
    pub major: u32,
    /// Additive version component.
    pub minor: u32,
}

impl ArnVersion {
    /// The version this library reads and writes.
    pub const CURRENT: ArnVersion = ArnVersion {
        major: 1,
        minor: 0,
    };

    /// The lowest major version this library can read.
    pub const SUPPORTED_MAJOR: u32 = 1;
}

impl Default for ArnVersion {
    fn default() -> Self {
        ArnVersion::CURRENT
    }
}

impl fmt::Display for ArnVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// An Astronomy module: the unit of compilation passed to backends.
///
/// A module owns its type, symbol and constant tables; all [`TypeId`] /
/// [`SymbolId`] / [`ConstantId`](crate::id::ConstantId) handles are only
/// meaningful inside the module that produced them.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub(crate) version: ArnVersion,
    pub(crate) name: Option<SymbolId>,
    pub(crate) symbols: SymbolStore,
    pub(crate) types: TypeStore,
    pub(crate) constants: ConstantStore,
    pub(crate) functions: Vec<Function>,
}

impl Module {
    /// Creates an empty module with all scalar types pre-interned, so the
    /// [`TypeId`] scalar constants are immediately valid.
    pub fn new() -> Self {
        Module {
            version: ArnVersion::CURRENT,
            name: None,
            symbols: SymbolStore::new(),
            types: TypeStore::new(),
            constants: ConstantStore::new(),
            functions: Vec::new(),
        }
    }

    /// The ARN version of this module.
    pub fn version(&self) -> ArnVersion {
        self.version
    }

    /// Sets the ARN version (used by the parser).
    pub fn set_version(&mut self, version: ArnVersion) {
        self.version = version;
    }

    /// The module name, if any.
    pub fn name(&self) -> Option<&str> {
        self.name.and_then(|s| self.symbols.name(s))
    }

    /// Sets the module name.
    pub fn set_name(&mut self, name: &str) {
        self.name = Some(self.symbols.intern(name));
    }

    /// The symbol table.
    pub fn symbols(&self) -> &SymbolStore {
        &self.symbols
    }

    /// The symbol table (mutable, for low-level construction).
    pub fn symbols_mut(&mut self) -> &mut SymbolStore {
        &mut self.symbols
    }

    /// Resolves a symbol name.
    pub fn symbol_name(&self, id: SymbolId) -> Option<&str> {
        self.symbols.name(id)
    }

    /// The type table.
    pub fn types(&self) -> &TypeStore {
        &self.types
    }

    /// The type table (mutable, for low-level construction and interning).
    pub fn types_mut(&mut self) -> &mut TypeStore {
        &mut self.types
    }

    /// Formats a type in canonical ARN syntax.
    pub fn type_name(&self, ty: TypeId) -> String {
        type_to_string(&self.types, ty)
    }

    /// Interns a type.
    pub fn intern_type(&mut self, data: TypeData) -> TypeId {
        self.types.intern(data)
    }

    /// The constant pool.
    pub fn constants(&self) -> &ConstantStore {
        &self.constants
    }

    /// The constant pool (mutable, for low-level construction).
    pub fn constants_mut(&mut self) -> &mut ConstantStore {
        &mut self.constants
    }

    /// Interns a constant.
    pub fn intern_constant(&mut self, data: ConstantData) -> crate::id::ConstantId {
        self.constants.intern(data)
    }

    /// All functions, in declaration order.
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    /// All functions, mutable (for low-level construction; see §30).
    pub fn functions_mut(&mut self) -> &mut Vec<Function> {
        &mut self.functions
    }

    /// Looks up a function by ID.
    pub fn function(&self, id: FunctionId) -> Option<&Function> {
        self.functions.get(id.index())
    }

    /// Looks up a function by ID (mutable).
    pub fn function_mut(&mut self, id: FunctionId) -> Option<&mut Function> {
        self.functions.get_mut(id.index())
    }

    /// Finds a function by symbol name.
    pub fn function_by_name(&self, name: &str) -> Option<FunctionId> {
        self.functions
            .iter()
            .position(|f| self.symbols.name(f.symbol) == Some(name))
            .map(|i| FunctionId::new(i as u32))
    }

    /// The name of a function.
    pub fn function_name(&self, id: FunctionId) -> &str {
        self.function(id)
            .and_then(|f| self.symbols.name(f.symbol))
            .unwrap_or("<invalid-function>")
    }

    /// Verifies this module, returning a [`VerifiedModule`] on success or
    /// all collected errors on failure (§24 VerifiedModule).
    pub fn verify(self) -> Result<VerifiedModule, crate::error::VerifyErrorReport> {
        Verifier::verify(self)
    }

    /// Parses an `.arn` text into a module (§31).
    pub fn parse_arn(source: &str) -> Result<Module, ParseError> {
        crate::text::parse(source)
    }

    /// Serializes the module to canonical `.arn` text (§32).
    pub fn to_arn(&self) -> String {
        crate::text::print(self)
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}

/// A module that has passed the [`Verifier`].
///
/// Construction is only possible through verification, so backends can rely
/// on receiving valid IR by accepting `&VerifiedModule` (§24).
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedModule {
    module: Module,
}

impl VerifiedModule {
    /// Creates a verified wrapper. Not part of the public API contract —
    /// use [`Module::verify`] instead.
    pub(crate) fn new(module: Module) -> Self {
        VerifiedModule { module }
    }

    /// Borrows the inner module.
    pub fn module(&self) -> &Module {
        &self.module
    }

    /// Consumes the wrapper, returning the inner module.
    pub fn into_inner(self) -> Module {
        self.module
    }

    /// Re-serializes to canonical `.arn`.
    pub fn to_arn(&self) -> String {
        self.module.to_arn()
    }
}

impl Deref for VerifiedModule {
    type Target = Module;

    fn deref(&self) -> &Module {
        &self.module
    }
}

impl AsRef<Module> for VerifiedModule {
    fn as_ref(&self) -> &Module {
        &self.module
    }
}
