//! Source positions attached to IR nodes that originate from text.

use std::fmt;

/// A source position in an `.arn` file.
///
/// Astronomy keeps spans strictly out of the semantic identity of the IR
/// (§43 Source Span): two modules that differ only in spans are considered
/// semantically identical. Spans exist so that diagnostics produced from a
/// parsed `.arn` file can point back at the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Span {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (in characters).
    pub col: u32,
}

impl Span {
    /// Creates a new span.
    pub fn new(line: u32, col: u32) -> Self {
        Span { line, col }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}
