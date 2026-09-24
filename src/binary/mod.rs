//! Binary representation of Astronomy IR: the `.arb` format (§16).
//!
//! `.arb` is the compact, fast-loading counterpart of `.arn` text. It is a
//! direct serialization of the in-memory [`Module`](crate::Module): every
//! numeric ID (types, symbols, constants, functions, values, blocks) is
//! stored as-is, so decoding rebuilds the exact same arenas without any
//! name resolution or renumbering.
//!
//! ```text
//! In-memory Core  ←→  ARB Binary
//!         (writer / reader)
//! ```
//!
//! Guarantees:
//!
//! * **Lossless** — `read(&write(&m))` equals `m` for every module whose
//!   nodes carry no source spans. Spans point into `.arn` source text and
//!   are not part of semantic identity (§43), so they are not stored.
//! * **Deterministic** — the output depends only on arena order; the same
//!   module always encodes to the same bytes.
//! * **Untrusted input** — the reader never panics and never allocates
//!   more than the input can justify. It checks structure (framing, tags,
//!   interning invariants, checksum); semantic validity remains the job of
//!   the [`Verifier`](crate::Verifier), exactly as for parsed `.arn`.
//!
//! The byte layout is specified in `SPECIFICATION.md` §16.

mod reader;
mod writer;

pub use reader::read;
pub use writer::write;

/// The 8 magic bytes every `.arb` file starts with.
///
/// Like PNG's signature, the non-ASCII lead byte and the CR LF / SUB / LF
/// tail make text-mode transfers and truncation-by-`^Z` detectable.
pub const MAGIC: [u8; 8] = [0x7F, b'A', b'R', b'B', b'\r', b'\n', 0x1A, b'\n'];

/// Major version of the `.arb` container format written by this library.
pub const FORMAT_MAJOR: u16 = 1;

/// Minor version of the `.arb` container format written by this library.
pub const FORMAT_MINOR: u16 = 0;

/// Returns true when `bytes` starts with the `.arb` [`MAGIC`].
///
/// Useful for tools that accept both `.arn` text and `.arb` binary input.
pub fn is_arb(bytes: &[u8]) -> bool {
    bytes.starts_with(&MAGIC)
}

// ---------------------------------------------------------------------------
// Shared encoding tables
// ---------------------------------------------------------------------------

/// Section tags, in the mandatory file order.
pub(crate) mod section {
    pub const MODULE: [u8; 4] = *b"MODL";
    pub const SYMBOLS: [u8; 4] = *b"SYMB";
    pub const TYPES: [u8; 4] = *b"TYPE";
    pub const CONSTANTS: [u8; 4] = *b"CNST";
    pub const FUNCTIONS: [u8; 4] = *b"FUNC";
}

pub(crate) mod type_tag {
    pub const VOID: u8 = 0;
    pub const INT: u8 = 1;
    pub const FLOAT: u8 = 2;
    pub const POINTER: u8 = 3;
    pub const ARRAY: u8 = 4;
    pub const STRUCT: u8 = 5;
    pub const FUNCTION: u8 = 6;
}

pub(crate) mod const_tag {
    pub const INT: u8 = 0;
    pub const FLOAT: u8 = 1;
    pub const NULL: u8 = 2;
    pub const STRING: u8 = 3;
    pub const AGGREGATE: u8 = 4;
}

pub(crate) mod value_tag {
    pub const RESERVED: u8 = 0;
    pub const PARAM: u8 = 1;
    pub const BLOCK_PARAM: u8 = 2;
    pub const INST: u8 = 3;
}

pub(crate) mod term_tag {
    pub const NONE: u8 = 0;
    pub const JUMP: u8 = 1;
    pub const BRANCH: u8 = 2;
    pub const RETURN: u8 = 3;
    pub const UNREACHABLE: u8 = 4;
}

/// Instruction opcodes. Values are stable within a format major version.
pub(crate) mod opcode {
    pub const CONST: u8 = 0x00;
    pub const ADD: u8 = 0x01;
    pub const SUB: u8 = 0x02;
    pub const MUL: u8 = 0x03;
    pub const DIV: u8 = 0x04;
    pub const REM: u8 = 0x05;
    pub const AND: u8 = 0x06;
    pub const OR: u8 = 0x07;
    pub const XOR: u8 = 0x08;
    pub const SHL: u8 = 0x09;
    pub const SHR: u8 = 0x0A;
    pub const EQ: u8 = 0x0B;
    pub const NE: u8 = 0x0C;
    pub const LT: u8 = 0x0D;
    pub const LE: u8 = 0x0E;
    pub const GT: u8 = 0x0F;
    pub const GE: u8 = 0x10;
    pub const ALLOCA: u8 = 0x11;
    pub const LOAD: u8 = 0x12;
    pub const STORE: u8 = 0x13;
    pub const PTR_OFFSET: u8 = 0x14;
    pub const EXT: u8 = 0x15;
    pub const TRUNC: u8 = 0x16;
    pub const INT_TO_FLOAT: u8 = 0x17;
    pub const FLOAT_TO_INT: u8 = 0x18;
    pub const PTR_CAST: u8 = 0x19;
    pub const CALL: u8 = 0x1A;
    pub const CONSTRUCT: u8 = 0x1B;
    pub const EXTRACT: u8 = 0x1C;
    pub const INSERT: u8 = 0x1D;
}

/// CRC-32 (IEEE 802.3, reflected polynomial `0xEDB88320`) — the checksum
/// used by zlib/PNG, computed with a compile-time table.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    let mut crc = !0u32;
    for &b in bytes {
        crc = TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference_vector() {
        // The standard CRC-32 check value.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }
}
