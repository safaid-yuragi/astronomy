//! Textual representation of Astronomy IR: the `.arn` format (§31).
//!
//! The text layer depends on the core model, never the other way around:
//!
//! ```text
//! In-memory Core  ←→  ARN Text
//!        (parser / printer)
//! ```
//!
//! `.arn` is a canonical, human-inspectable external format. Normal
//! compilation flows do not pass through it (§4).

mod lexer;
mod parser;
mod printer;

pub use parser::parse;
pub use printer::print;

#[cfg(test)]
mod tests {
    use crate::text::{parse, print};

    #[test]
    fn empty_module_roundtrip() {
        let src = concat!(
            "::ASTRONOMY::MODULE_START\n",
            "::ASTRONOMY::MODULE_VERSION 1\n",
            "::ASTRONOMY::MODULE_END\n"
        );
        let module = parse(src).unwrap();
        let printed = print(&module);
        assert_eq!(printed, src);
    }

    #[test]
    fn print_is_canonical_and_idempotent() {
        // Whitespace and comments must not change canonical output.
        let src = concat!(
            "::ASTRONOMY::MODULE_START\n",
            "# a comment\n",
            "::ASTRONOMY::MODULE_VERSION 1\n",
            "::ASTRONOMY::FUNCTION_START f fn(i32 %x) -> i32 linkage=exported abi=astronomy\n",
            "::ASTRONOMY::BLOCK_START entry\n",
            "::ASTRONOMY::RETURN i32 %x\n",
            "::ASTRONOMY::BLOCK_END\n",
            "::ASTRONOMY::FUNCTION_END\n",
            "::ASTRONOMY::MODULE_END\n"
        );
        let module = parse(src).unwrap();
        let once = print(&module);
        let again = print(&parse(&once).unwrap());
        assert_eq!(once, again);
        assert!(!once.contains('#'));
    }
}
