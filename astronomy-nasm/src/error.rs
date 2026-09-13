//! Structured diagnostics for the NASM backend.
//!
//! The backend never returns `String` errors, mirroring the core crate's
//! convention (`A-BUILD-*`, `A-VERIFY-*`, `A-ARN-*`): every failure carries a
//! stable code so callers can match on the variant.

use std::fmt;

/// A failure while lowering a verified module to NASM assembly.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// A type used by a value, parameter, result or pointee is not
    /// representable by this backend (`A-NASM-001`).
    UnsupportedType {
        /// Where the type was encountered (e.g. `parameter 0 of `f``).
        context: String,
        /// Canonical `.arn` spelling of the type.
        ty: String,
        /// Human-readable reason.
        reason: String,
    },
    /// An instruction or terminator cannot be lowered (`A-NASM-002`).
    UnsupportedInstruction {
        /// Function being lowered.
        function: String,
        /// The operation name.
        op: &'static str,
        /// Human-readable reason.
        reason: String,
    },
    /// A function's ABI cannot be implemented (`A-NASM-003`).
    UnsupportedAbi {
        /// Function being lowered.
        function: String,
        /// Canonical ABI keyword.
        abi: String,
    },
    /// The verified module is internally inconsistent in a way the backend
    /// requires (should be unreachable for verified input) (`A-NASM-004`).
    InvalidModule {
        /// Human-readable reason.
        reason: String,
    },
}

impl BackendError {
    /// Stable error code (e.g. `A-NASM-001`).
    pub fn code(&self) -> &'static str {
        match self {
            BackendError::UnsupportedType { .. } => "A-NASM-001",
            BackendError::UnsupportedInstruction { .. } => "A-NASM-002",
            BackendError::UnsupportedAbi { .. } => "A-NASM-003",
            BackendError::InvalidModule { .. } => "A-NASM-004",
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
        }
    }
}

impl std::error::Error for BackendError {}
