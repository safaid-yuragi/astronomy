//! Structured diagnostics shared by the x86-64 backends.
//!
//! The backends never return `String` errors, mirroring the core crate's
//! convention (`A-BUILD-*`, `A-VERIFY-*`, `A-ARN-*`): every failure carries a
//! stable code so callers can match on the variant.

use std::fmt;

/// A failure while lowering a verified module for x86-64, or while writing
/// the result out (NASM text in `astronomy-nasm`, an ELF object in
/// `astronomy-object`).
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// A type used by a value, parameter, result or pointee is not
    /// representable by this backend (`A-X86-001`).
    UnsupportedType {
        /// Where the type was encountered (e.g. `parameter 0 of `f``).
        context: String,
        /// Canonical `.arn` spelling of the type.
        ty: String,
        /// Human-readable reason.
        reason: String,
    },
    /// An instruction or terminator cannot be lowered (`A-X86-002`).
    UnsupportedInstruction {
        /// Function being lowered.
        function: String,
        /// The operation name.
        op: &'static str,
        /// Human-readable reason.
        reason: String,
    },
    /// A function's ABI cannot be implemented (`A-X86-003`).
    UnsupportedAbi {
        /// Function being lowered.
        function: String,
        /// Canonical ABI keyword.
        abi: String,
    },
    /// The verified module is internally inconsistent in a way the backend
    /// requires (should be unreachable for verified input) (`A-X86-004`).
    InvalidModule {
        /// Human-readable reason.
        reason: String,
    },
    /// The module cannot be represented in an ELF64 object file, e.g. a
    /// symbol name containing NUL or more than 2 GiB of code (`A-X86-005`;
    /// raised by `astronomy-object`).
    ObjectLimit {
        /// Human-readable reason.
        reason: String,
    },
}

impl BackendError {
    /// Stable error code (e.g. `A-X86-001`).
    pub fn code(&self) -> &'static str {
        match self {
            BackendError::UnsupportedType { .. } => "A-X86-001",
            BackendError::UnsupportedInstruction { .. } => "A-X86-002",
            BackendError::UnsupportedAbi { .. } => "A-X86-003",
            BackendError::InvalidModule { .. } => "A-X86-004",
            BackendError::ObjectLimit { .. } => "A-X86-005",
        }
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::UnsupportedType {
                context,
                ty,
                reason,
            } => write!(
                f,
                "[{}] unsupported type `{ty}` in {context}: {reason}",
                self.code()
            ),
            BackendError::UnsupportedInstruction {
                function,
                op,
                reason,
            } => write!(
                f,
                "[{}] function `{function}`: cannot lower `{op}`: {reason}",
                self.code()
            ),
            BackendError::UnsupportedAbi { function, abi } => write!(
                f,
                "[{}] function `{function}`: unsupported ABI `{abi}`",
                self.code()
            ),
            BackendError::InvalidModule { reason } => {
                write!(f, "[{}] invalid module: {reason}", self.code())
            }
            BackendError::ObjectLimit { reason } => {
                write!(f, "[{}] cannot emit object file: {reason}", self.code())
            }
        }
    }
}

impl std::error::Error for BackendError {}
