//! Structured diagnostics with stable error codes (§42 Diagnostics).
//!
//! Every failure mode is a data-carrying enum — never a `String` error — so
//! CLIs and frontends can render their own diagnostics.

use std::fmt;

use crate::id::{BlockId, FunctionId, TypeId, ValueId};

fn write_prefixed(f: &mut fmt::Formatter<'_>, code: &str, msg: &str) -> fmt::Result {
    write!(f, "[{code}] {msg}")
}

// ---------------------------------------------------------------------------
// Build errors (A-BUILD-*)
// ---------------------------------------------------------------------------

/// Errors produced by the builder API while constructing IR.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// A referenced value does not exist in the function (`A-BUILD-001`).
    UnknownValue {
        /// The offending value ID.
        value: ValueId,
        /// What the builder was doing.
        context: &'static str,
    },
    /// A referenced block does not exist (`A-BUILD-002`).
    UnknownBlock {
        /// The offending block ID.
        block: BlockId,
    },
    /// A referenced function does not exist (`A-BUILD-003`).
    UnknownFunction {
        /// The offending function ID.
        function: FunctionId,
    },
    /// A referenced type does not exist (`A-BUILD-004`).
    UnknownType {
        /// The offending type ID.
        ty: TypeId,
    },
    /// Operand or result types disagree (`A-BUILD-010`).
    TypeMismatch {
        /// What was being built.
        context: String,
        /// The type that was required.
        expected: String,
        /// The type that was found.
        found: String,
    },
    /// An operand is not valid for the instruction (`A-BUILD-011`).
    InvalidOperand {
        /// What was being built.
        context: &'static str,
        /// Human-readable reason.
        reason: String,
    },
    /// The current block already has a terminator (`A-BUILD-012`).
    BlockTerminated {
        /// The block that was already complete.
        block: BlockId,
    },
    /// A block was not in the required state (`A-BUILD-013`).
    BlockNotEmpty {
        /// The block.
        block: BlockId,
        /// Human-readable reason.
        reason: &'static str,
    },
    /// A name is reserved or malformed (`A-BUILD-014`).
    InvalidName {
        /// The rejected name.
        name: String,
        /// Human-readable reason.
        reason: &'static str,
    },
    /// A constant is not well-formed (`A-BUILD-015`).
    InvalidConstant {
        /// Human-readable reason.
        reason: String,
    },
    /// A variadic function cannot have a body in the MVP (`A-BUILD-016`).
    VariadicDefinition {
        /// The function name.
        name: String,
    },
    /// A function with this name already exists (`A-BUILD-017`).
    DuplicateFunctionName {
        /// The duplicated name.
        name: String,
    },
    /// No block is selected for emitting instructions (`A-BUILD-018`).
    NoCurrentBlock {
        /// What the builder was doing.
        action: &'static str,
    },
    /// A function signature is not valid (`A-BUILD-019`).
    InvalidSignature {
        /// Human-readable reason.
        reason: String,
    },
}

impl BuildError {
    /// Stable error code (e.g. `A-BUILD-001`).
    pub fn code(&self) -> &'static str {
        match self {
            BuildError::UnknownValue { .. } => "A-BUILD-001",
            BuildError::UnknownBlock { .. } => "A-BUILD-002",
            BuildError::UnknownFunction { .. } => "A-BUILD-003",
            BuildError::UnknownType { .. } => "A-BUILD-004",
            BuildError::TypeMismatch { .. } => "A-BUILD-010",
            BuildError::InvalidOperand { .. } => "A-BUILD-011",
            BuildError::BlockTerminated { .. } => "A-BUILD-012",
            BuildError::BlockNotEmpty { .. } => "A-BUILD-013",
            BuildError::InvalidName { .. } => "A-BUILD-014",
            BuildError::InvalidConstant { .. } => "A-BUILD-015",
            BuildError::VariadicDefinition { .. } => "A-BUILD-016",
            BuildError::DuplicateFunctionName { .. } => "A-BUILD-017",
            BuildError::NoCurrentBlock { .. } => "A-BUILD-018",
            BuildError::InvalidSignature { .. } => "A-BUILD-019",
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::UnknownValue { value, context } => {
                write_prefixed(f, self.code(), &format!("unknown value {value} in {context}"))
            }
            BuildError::UnknownBlock { block } => {
                write_prefixed(f, self.code(), &format!("unknown block {block}"))
            }
            BuildError::UnknownFunction { function } => {
                write_prefixed(f, self.code(), &format!("unknown function {function}"))
            }
            BuildError::UnknownType { ty } => {
                write_prefixed(f, self.code(), &format!("unknown type {ty:?}"))
            }
            BuildError::TypeMismatch {
                context,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!("type mismatch in {context}: expected `{expected}`, found `{found}`"),
            ),
            BuildError::InvalidOperand { context, reason } => {
                write_prefixed(f, self.code(), &format!("invalid operand in {context}: {reason}"))
            }
            BuildError::BlockTerminated { block } => write_prefixed(
                f,
                self.code(),
                &format!("block {block} already has a terminator"),
            ),
            BuildError::BlockNotEmpty { block, reason } => write_prefixed(
                f,
                self.code(),
                &format!("block {block} is not in the required state: {reason}"),
            ),
            BuildError::InvalidName { name, reason } => {
                write_prefixed(f, self.code(), &format!("invalid name `{name}`: {reason}"))
            }
            BuildError::InvalidConstant { reason } => {
                write_prefixed(f, self.code(), &format!("invalid constant: {reason}"))
            }
            BuildError::VariadicDefinition { name } => write_prefixed(
                f,
                self.code(),
                &format!("function `{name}` is variadic and cannot have a body"),
            ),
            BuildError::DuplicateFunctionName { name } => write_prefixed(
                f,
                self.code(),
                &format!("function `{name}` is already declared"),
            ),
            BuildError::NoCurrentBlock { action } => write_prefixed(
                f,
                self.code(),
                &format!("cannot {action}: no current block (call append_block first)"),
            ),
            BuildError::InvalidSignature { reason } => {
                write_prefixed(f, self.code(), &format!("invalid signature: {reason}"))
            }
        }
    }
}

impl std::error::Error for BuildError {}

// ---------------------------------------------------------------------------
// Verify errors (A-VERIFY-*)
// ---------------------------------------------------------------------------

/// Errors produced by the [`crate::Verifier`].
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// An operand references a value outside the function arena
    /// (`A-VERIFY-001`).
    UnknownValue {
        /// Function name.
        function: String,
        /// The offending value index.
        value: u32,
    },
    /// A terminator references a block that does not exist (`A-VERIFY-002`).
    UnknownBlock {
        /// Function name.
        function: String,
        /// The offending block index.
        block: u32,
    },
    /// A call references a function that does not exist (`A-VERIFY-003`).
    UnknownFunction {
        /// Name of the calling function.
        function: String,
        /// The offending callee index.
        callee: u32,
    },
    /// A `const` references a constant outside the pool (`A-VERIFY-004`).
    UnknownConstant {
        /// The offending constant index.
        constant: u32,
    },
    /// A structure references a type outside the store (`A-VERIFY-005`).
    UnknownType {
        /// The offending type index.
        ty: u32,
    },
    /// Types disagree where they must match (`A-VERIFY-010`).
    TypeMismatch {
        /// Function name.
        function: String,
        /// What was being checked.
        context: String,
        /// The required type.
        expected: String,
        /// The found type.
        found: String,
    },
    /// An operand type is not valid for the instruction (`A-VERIFY-011`).
    InvalidOperandType {
        /// Function name.
        function: String,
        /// Human-readable reason.
        reason: String,
    },
    /// The result value of an instruction has the wrong type
    /// (`A-VERIFY-012`).
    InvalidResultType {
        /// Function name.
        function: String,
        /// The required type.
        expected: String,
        /// The found type.
        found: String,
    },
    /// A branch condition is not `i1` (`A-VERIFY-013`).
    ConditionNotI1 {
        /// Function name.
        function: String,
        /// The found type.
        found: String,
    },
    /// A value is defined more than once (`A-VERIFY-020`).
    ValueRedefined {
        /// Function name.
        function: String,
        /// The redefined value index.
        value: u32,
        /// Description of the earlier definition.
        first: String,
        /// Description of the later definition.
        second: String,
    },
    /// A value was reserved but never defined (`A-VERIFY-021`).
    UndefinedValue {
        /// Function name.
        function: String,
        /// The value index.
        value: u32,
    },
    /// A value definition does not match its claimed site (`A-VERIFY-023`).
    InconsistentDefinition {
        /// Function name.
        function: String,
        /// The value index.
        value: u32,
        /// Human-readable reason.
        reason: String,
    },
    /// A use is not dominated by its definition (`A-VERIFY-022`).
    UseNotDominated {
        /// Function name.
        function: String,
        /// The used value index.
        value: u32,
        /// The using block.
        use_block: u32,
        /// The defining block.
        def_block: u32,
    },
    /// A block has no terminator (`A-VERIFY-030`).
    MissingTerminator {
        /// Function name.
        function: String,
        /// The block index.
        block: u32,
    },
    /// The entry block declares parameters (`A-VERIFY-031`).
    EntryBlockWithParams {
        /// Function name.
        function: String,
        /// Number of parameters found.
        count: usize,
    },
    /// Branch argument count does not match block parameters
    /// (`A-VERIFY-040`).
    BlockArgCountMismatch {
        /// Function name.
        function: String,
        /// The branching block.
        from_block: u32,
        /// The target block.
        to_block: u32,
        /// Expected argument count.
        expected: usize,
        /// Found argument count.
        found: usize,
    },
    /// A branch argument type does not match the block parameter type
    /// (`A-VERIFY-041`).
    BlockArgTypeMismatch {
        /// Function name.
        function: String,
        /// The branching block.
        from_block: u32,
        /// The target block.
        to_block: u32,
        /// Parameter index.
        index: usize,
        /// The required type.
        expected: String,
        /// The found type.
        found: String,
    },
    /// A `return` does not match the function result type (`A-VERIFY-050`).
    ReturnMismatch {
        /// Function name.
        function: String,
        /// The required type.
        expected: String,
        /// Description of what was returned.
        found: String,
    },
    /// Call argument count does not match the callee (`A-VERIFY-051`).
    CallArityMismatch {
        /// Calling function name.
        function: String,
        /// Callee name.
        callee: String,
        /// Expected argument description.
        expected: String,
        /// Found argument count.
        found: usize,
    },
    /// A call argument type does not match (`A-VERIFY-052`).
    CallArgTypeMismatch {
        /// Calling function name.
        function: String,
        /// Callee name.
        callee: String,
        /// Argument index.
        index: usize,
        /// The required type.
        expected: String,
        /// The found type.
        found: String,
    },
    /// A pool constant is not well-formed (`A-VERIFY-060`).
    InvalidConstant {
        /// The constant index.
        constant: u32,
        /// Human-readable reason.
        reason: String,
    },
    /// Two functions share a symbol name (`A-VERIFY-070`).
    DuplicateFunctionName {
        /// The duplicated name.
        name: String,
    },
    /// A declaration/definition violates linkage rules (`A-VERIFY-071`).
    InvalidDeclaration {
        /// Function name.
        function: String,
        /// Human-readable reason.
        reason: String,
    },
    /// A variadic function has a body (`A-VERIFY-072`).
    VariadicDefinition {
        /// Function name.
        function: String,
    },
    /// A function signature is not valid (`A-VERIFY-073`).
    InvalidSignature {
        /// Function name.
        function: String,
        /// Human-readable reason.
        reason: String,
    },
}

impl VerifyError {
    /// Stable error code (e.g. `A-VERIFY-001`).
    pub fn code(&self) -> &'static str {
        match self {
            VerifyError::UnknownValue { .. } => "A-VERIFY-001",
            VerifyError::UnknownBlock { .. } => "A-VERIFY-002",
            VerifyError::UnknownFunction { .. } => "A-VERIFY-003",
            VerifyError::UnknownConstant { .. } => "A-VERIFY-004",
            VerifyError::UnknownType { .. } => "A-VERIFY-005",
            VerifyError::TypeMismatch { .. } => "A-VERIFY-010",
            VerifyError::InvalidOperandType { .. } => "A-VERIFY-011",
            VerifyError::InvalidResultType { .. } => "A-VERIFY-012",
            VerifyError::ConditionNotI1 { .. } => "A-VERIFY-013",
            VerifyError::ValueRedefined { .. } => "A-VERIFY-020",
            VerifyError::UndefinedValue { .. } => "A-VERIFY-021",
            VerifyError::UseNotDominated { .. } => "A-VERIFY-022",
            VerifyError::InconsistentDefinition { .. } => "A-VERIFY-023",
            VerifyError::MissingTerminator { .. } => "A-VERIFY-030",
            VerifyError::EntryBlockWithParams { .. } => "A-VERIFY-031",
            VerifyError::BlockArgCountMismatch { .. } => "A-VERIFY-040",
            VerifyError::BlockArgTypeMismatch { .. } => "A-VERIFY-041",
            VerifyError::ReturnMismatch { .. } => "A-VERIFY-050",
            VerifyError::CallArityMismatch { .. } => "A-VERIFY-051",
            VerifyError::CallArgTypeMismatch { .. } => "A-VERIFY-052",
            VerifyError::InvalidConstant { .. } => "A-VERIFY-060",
            VerifyError::DuplicateFunctionName { .. } => "A-VERIFY-070",
            VerifyError::InvalidDeclaration { .. } => "A-VERIFY-071",
            VerifyError::VariadicDefinition { .. } => "A-VERIFY-072",
            VerifyError::InvalidSignature { .. } => "A-VERIFY-073",
        }
    }
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::UnknownValue { function, value } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: unknown value v{value}"),
            ),
            VerifyError::UnknownBlock { function, block } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: unknown block bb{block}"),
            ),
            VerifyError::UnknownFunction { function, callee } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: unknown callee f{callee}"),
            ),
            VerifyError::UnknownConstant { constant } => write_prefixed(
                f,
                self.code(),
                &format!("unknown constant c{constant}"),
            ),
            VerifyError::UnknownType { ty } => {
                write_prefixed(f, self.code(), &format!("unknown type t{ty}"))
            }
            VerifyError::TypeMismatch {
                function,
                context,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: type mismatch in {context}: expected `{expected}`, found `{found}`"
                ),
            ),
            VerifyError::InvalidOperandType { function, reason } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: invalid operand type: {reason}"),
            ),
            VerifyError::InvalidResultType {
                function,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: instruction result type must be `{expected}`, found `{found}`"
                ),
            ),
            VerifyError::ConditionNotI1 { function, found } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: branch condition must be `i1`, found `{found}`"
                ),
            ),
            VerifyError::ValueRedefined {
                function,
                value,
                first,
                second,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: value v{value} defined more than once ({first}, then {second})"
                ),
            ),
            VerifyError::UndefinedValue { function, value } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: value v{value} was never defined"),
            ),
            VerifyError::InconsistentDefinition {
                function,
                value,
                reason,
            } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: value v{value}: {reason}"),
            ),
            VerifyError::UseNotDominated {
                function,
                value,
                use_block,
                def_block,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: use of v{value} in bb{use_block} is not dominated by its definition in bb{def_block}"
                ),
            ),
            VerifyError::MissingTerminator { function, block } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: block bb{block} has no terminator"),
            ),
            VerifyError::EntryBlockWithParams { function, count } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: entry block must not have parameters (found {count})"
                ),
            ),
            VerifyError::BlockArgCountMismatch {
                function,
                from_block,
                to_block,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: bb{from_block} passes {found} argument(s) to bb{to_block} which has {expected} parameter(s)"
                ),
            ),
            VerifyError::BlockArgTypeMismatch {
                function,
                from_block,
                to_block,
                index,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: bb{from_block} argument {index} for bb{to_block} must be `{expected}`, found `{found}`"
                ),
            ),
            VerifyError::ReturnMismatch {
                function,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: return must be `{expected}`, found {found}"
                ),
            ),
            VerifyError::CallArityMismatch {
                function,
                callee,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: call to `{callee}` expects {expected}, found {found} argument(s)"
                ),
            ),
            VerifyError::CallArgTypeMismatch {
                function,
                callee,
                index,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "function `{function}`: call to `{callee}` argument {index} must be `{expected}`, found `{found}`"
                ),
            ),
            VerifyError::InvalidConstant { constant, reason } => write_prefixed(
                f,
                self.code(),
                &format!("constant c{constant}: {reason}"),
            ),
            VerifyError::DuplicateFunctionName { name } => write_prefixed(
                f,
                self.code(),
                &format!("duplicate function name `{name}`"),
            ),
            VerifyError::InvalidDeclaration { function, reason } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: {reason}"),
            ),
            VerifyError::VariadicDefinition { function } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}` is variadic but has a body"),
            ),
            VerifyError::InvalidSignature { function, reason } => write_prefixed(
                f,
                self.code(),
                &format!("function `{function}`: invalid signature: {reason}"),
            ),
        }
    }
}

impl std::error::Error for VerifyError {}

// ---------------------------------------------------------------------------
// ARN text errors (A-ARN-*)
// ---------------------------------------------------------------------------

/// Errors produced when parsing `.arn` text.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Unsupported module version (`A-ARN-001`).
    UnsupportedVersion {
        /// Line of the version directive.
        line: u32,
        /// The major version that was found.
        found: u32,
        /// The major version this library supports.
        supported: u32,
    },
    /// An unexpected token was encountered (`A-ARN-002`).
    UnexpectedToken {
        /// Line of the token.
        line: u32,
        /// What the parser expected.
        expected: String,
        /// What was found.
        found: String,
    },
    /// An unknown `::ASTRONOMY::` directive (`A-ARN-003`).
    UnknownDirective {
        /// Line of the directive.
        line: u32,
        /// The directive name.
        name: String,
    },
    /// A function is declared twice (`A-ARN-004`).
    DuplicateFunction {
        /// Line of the duplicate.
        line: u32,
        /// The duplicated name.
        name: String,
    },
    /// A value handle is defined twice in a function (`A-ARN-005`).
    DuplicateValue {
        /// Line of the duplicate.
        line: u32,
        /// The duplicated handle.
        name: String,
    },
    /// A block label is defined twice (`A-ARN-006`).
    DuplicateBlock {
        /// Line of the duplicate.
        line: u32,
        /// The duplicated label.
        label: String,
    },
    /// A value handle is used before/without being defined (`A-ARN-007`).
    UndefinedValue {
        /// Line of the use.
        line: u32,
        /// The handle.
        name: String,
    },
    /// A block label is referenced but never defined (`A-ARN-008`).
    UndefinedBlock {
        /// Line of the reference.
        line: u32,
        /// The label.
        label: String,
    },
    /// A constant reference is out of range (`A-ARN-009`).
    UndefinedConstant {
        /// Line of the reference.
        line: u32,
        /// The referenced id.
        name: String,
    },
    /// A type is malformed (`A-ARN-010`).
    InvalidType {
        /// Line of the type token.
        line: u32,
        /// Human-readable reason.
        reason: String,
    },
    /// A number literal is malformed or out of range (`A-ARN-011`).
    InvalidNumber {
        /// Line of the literal.
        line: u32,
        /// Human-readable reason.
        reason: String,
    },
    /// An operand annotation does not match the value (`A-ARN-012`).
    OperandTypeMismatch {
        /// Line of the operand.
        line: u32,
        /// The annotated type.
        annotated: String,
        /// The actual value type.
        actual: String,
    },
    /// A string literal is malformed (`A-ARN-013`).
    InvalidString {
        /// Line of the literal.
        line: u32,
        /// Human-readable reason.
        reason: String,
    },
    /// A directive appeared in an invalid position (`A-ARN-014`).
    MisplacedDirective {
        /// Line of the directive.
        line: u32,
        /// The directive name.
        name: String,
        /// Human-readable reason.
        reason: String,
    },
    /// A required directive is missing (`A-ARN-015`).
    MissingDirective {
        /// The expected directive.
        expected: String,
    },
    /// The module structure is broken (`A-ARN-016`).
    InvalidStructure {
        /// Line where the problem was detected.
        line: u32,
        /// Human-readable reason.
        reason: String,
    },
    /// Input ended before the module was complete (`A-ARN-017`).
    UnexpectedEof {
        /// What the parser expected.
        expected: String,
    },
    /// An instruction follows a terminator (`A-ARN-018`).
    InstructionAfterTerminator {
        /// Line of the instruction.
        line: u32,
    },
    /// A name is reserved (`A-ARN-019`).
    ReservedName {
        /// Line of the name.
        line: u32,
        /// The name.
        name: String,
        /// Human-readable reason.
        reason: String,
    },
    /// A call references an undeclared function (`A-ARN-020`).
    UndefinedFunction {
        /// Line of the call.
        line: u32,
        /// The callee name.
        name: String,
    },
    /// An instruction operand is not valid (`A-ARN-021`).
    InvalidInstruction {
        /// Line of the instruction.
        line: u32,
        /// Human-readable reason.
        reason: String,
    },
}

impl ParseError {
    /// Stable error code (e.g. `A-ARN-001`).
    pub fn code(&self) -> &'static str {
        match self {
            ParseError::UnsupportedVersion { .. } => "A-ARN-001",
            ParseError::UnexpectedToken { .. } => "A-ARN-002",
            ParseError::UnknownDirective { .. } => "A-ARN-003",
            ParseError::DuplicateFunction { .. } => "A-ARN-004",
            ParseError::DuplicateValue { .. } => "A-ARN-005",
            ParseError::DuplicateBlock { .. } => "A-ARN-006",
            ParseError::UndefinedValue { .. } => "A-ARN-007",
            ParseError::UndefinedBlock { .. } => "A-ARN-008",
            ParseError::UndefinedConstant { .. } => "A-ARN-009",
            ParseError::InvalidType { .. } => "A-ARN-010",
            ParseError::InvalidNumber { .. } => "A-ARN-011",
            ParseError::OperandTypeMismatch { .. } => "A-ARN-012",
            ParseError::InvalidString { .. } => "A-ARN-013",
            ParseError::MisplacedDirective { .. } => "A-ARN-014",
            ParseError::MissingDirective { .. } => "A-ARN-015",
            ParseError::InvalidStructure { .. } => "A-ARN-016",
            ParseError::UnexpectedEof { .. } => "A-ARN-017",
            ParseError::InstructionAfterTerminator { .. } => "A-ARN-018",
            ParseError::ReservedName { .. } => "A-ARN-019",
            ParseError::UndefinedFunction { .. } => "A-ARN-020",
            ParseError::InvalidInstruction { .. } => "A-ARN-021",
        }
    }

    /// Source line of the error, when known.
    pub fn line(&self) -> Option<u32> {
        match self {
            ParseError::UnsupportedVersion { line, .. }
            | ParseError::UnexpectedToken { line, .. }
            | ParseError::UnknownDirective { line, .. }
            | ParseError::DuplicateFunction { line, .. }
            | ParseError::DuplicateValue { line, .. }
            | ParseError::DuplicateBlock { line, .. }
            | ParseError::UndefinedValue { line, .. }
            | ParseError::UndefinedBlock { line, .. }
            | ParseError::UndefinedConstant { line, .. }
            | ParseError::InvalidType { line, .. }
            | ParseError::InvalidNumber { line, .. }
            | ParseError::OperandTypeMismatch { line, .. }
            | ParseError::InvalidString { line, .. }
            | ParseError::MisplacedDirective { line, .. }
            | ParseError::InvalidStructure { line, .. }
            | ParseError::InstructionAfterTerminator { line, .. }
            | ParseError::ReservedName { line, .. }
            | ParseError::UndefinedFunction { line, .. }
            | ParseError::InvalidInstruction { line, .. } => Some(*line),
            ParseError::MissingDirective { .. } | ParseError::UnexpectedEof { .. } => None,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnsupportedVersion {
                found,
                supported,
                line,
            } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: unsupported ARN version {found} (supported: {supported})"),
            ),
            ParseError::UnexpectedToken {
                line,
                expected,
                found,
            } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: expected {expected}, found {found}"),
            ),
            ParseError::UnknownDirective { line, name } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: unknown directive `::ASTRONOMY::{name}`"),
            ),
            ParseError::DuplicateFunction { line, name } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: function `{name}` declared twice"),
            ),
            ParseError::DuplicateValue { line, name } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: value `%{name}` defined twice"),
            ),
            ParseError::DuplicateBlock { line, label } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: block `{label}` defined twice"),
            ),
            ParseError::UndefinedValue { line, name } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: use of undefined value `%{name}`"),
            ),
            ParseError::UndefinedBlock { line, label } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: use of undefined block `{label}`"),
            ),
            ParseError::UndefinedConstant { line, name } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: use of undefined constant `{name}`"),
            ),
            ParseError::InvalidType { line, reason } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: invalid type: {reason}"),
            ),
            ParseError::InvalidNumber { line, reason } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: invalid number: {reason}"),
            ),
            ParseError::OperandTypeMismatch {
                line,
                annotated,
                actual,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "line {line}: operand annotated as `{annotated}` but value has type `{actual}`"
                ),
            ),
            ParseError::InvalidString { line, reason } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: invalid string literal: {reason}"),
            ),
            ParseError::MisplacedDirective {
                line,
                name,
                reason,
            } => write_prefixed(
                f,
                self.code(),
                &format!(
                    "line {line}: directive `::ASTRONOMY::{name}` misplaced: {reason}"
                ),
            ),
            ParseError::MissingDirective { expected } => write_prefixed(
                f,
                self.code(),
                &format!("missing `{expected}`"),
            ),
            ParseError::InvalidStructure { line, reason } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: {reason}"),
            ),
            ParseError::UnexpectedEof { expected } => write_prefixed(
                f,
                self.code(),
                &format!("unexpected end of input, expected {expected}"),
            ),
            ParseError::InstructionAfterTerminator { line } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: instruction after terminator"),
            ),
            ParseError::ReservedName { line, name, reason } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: reserved name `{name}`: {reason}"),
            ),
            ParseError::UndefinedFunction { line, name } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: call to undefined function `{name}`"),
            ),
            ParseError::InvalidInstruction { line, reason } => write_prefixed(
                f,
                self.code(),
                &format!("line {line}: {reason}"),
            ),
        }
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Verification error report
// ---------------------------------------------------------------------------

/// All errors found by one verification run (§23: the verifier reports
/// everything it finds, not just the first failure).
///
/// Implements [`std::error::Error`] so `?` works out of the box in code
/// returning `Box<dyn std::error::Error>`, while structured access remains
/// available through [`VerifyErrorReport::errors`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyErrorReport {
    errors: Vec<VerifyError>,
}

impl VerifyErrorReport {
    /// Creates a report from collected errors.
    pub(crate) fn new(errors: Vec<VerifyError>) -> Self {
        VerifyErrorReport { errors }
    }

    /// The individual errors, in detection order.
    pub fn errors(&self) -> &[VerifyError] {
        &self.errors
    }

    /// Consumes the report, returning the individual errors.
    pub fn into_errors(self) -> Vec<VerifyError> {
        self.errors
    }

    /// Number of errors.
    pub fn len(&self) -> usize {
        self.errors.len()
    }

    /// Returns true when there are no errors (never true for a report).
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Iterates over the errors.
    pub fn iter(&self) -> std::slice::Iter<'_, VerifyError> {
        self.errors.iter()
    }
}

impl std::iter::IntoIterator for VerifyErrorReport {
    type Item = VerifyError;
    type IntoIter = std::vec::IntoIter<VerifyError>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.into_iter()
    }
}

impl<'a> IntoIterator for &'a VerifyErrorReport {
    type Item = &'a VerifyError;
    type IntoIter = std::slice::Iter<'a, VerifyError>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.iter()
    }
}

impl fmt::Display for VerifyErrorReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "verification failed with {} error(s):", self.errors.len())?;
        for e in &self.errors {
            write!(f, "\n  error[{}]: {e}", e.code())?;
        }
        Ok(())
    }
}

impl std::error::Error for VerifyErrorReport {}

// ---------------------------------------------------------------------------
// Constant type fit helper (shared by verifier and parser)
// ---------------------------------------------------------------------------

/// Returns true when constant `c` can be typed as `ty` in `store`.
///
/// Used by the verifier (for `const` instructions and pool validation) and
/// by the ARN parser (for aggregate constant validation).
pub fn constant_fits_type(
    store: &crate::types::TypeStore,
    c: &crate::constant::ConstantData,
    ty: TypeId,
) -> bool {
    use crate::constant::ConstantData;
    use crate::types::TypeData;
    let data = match store.get(ty) {
        Some(d) => d,
        None => return false,
    };
    match c {
        ConstantData::Int { ty: cty, width, .. } => {
            matches!(data, TypeData::Int { bits, .. } if *cty == ty && *bits == *width)
        }
        ConstantData::Float { ty: cty, .. } => *cty == ty && data.is_float(),
        ConstantData::Null { pointee } => {
            matches!(data, TypeData::Pointer { pointee: p, address_space: 0 } if *p == *pointee)
        }
        ConstantData::String { .. } => {
            matches!(data, TypeData::Pointer { pointee, address_space: 0 } if *pointee == TypeId::I8)
        }
        ConstantData::Aggregate { ty: cty, .. } => *cty == ty && data.is_aggregate(),
    }
}
