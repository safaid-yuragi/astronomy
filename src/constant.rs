//! The module constant pool (§20 Constants).
//!
//! Constants are interned per module and referenced from `const`
//! instructions by [`ConstantId`]. Integer and float values are stored as
//! raw bit patterns so every width (including `u128`) is representable.
//! String data is deduplicated through the interning table.

use std::collections::HashMap;

use crate::id::{ConstantId, TypeId};

/// A constant value in the pool.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstantData {
    /// Integer constant. `bits` holds the two's-complement bit pattern of
    /// exactly `width(ty)` bits (values are stored masked, sign is an
    /// interpretation of the type).
    Int {
        /// Integer type of the constant.
        ty: TypeId,
        /// Bit width of `ty`.
        width: u32,
        /// Two's-complement bit pattern, zero-extended into `u128`.
        bits: u128,
    },
    /// Float constant. `bits` holds the IEEE bit pattern.
    Float {
        /// `f32` or `f64` type.
        ty: TypeId,
        /// `f32` (zero-extended) or `f64` bit pattern.
        bits: u64,
    },
    /// Null pointer constant for `ptr<pointee>`.
    Null {
        /// Pointee type; the pointer type is `ptr<pointee>`.
        pointee: TypeId,
    },
    /// Immutable string data. Has type `ptr<i8>` and points at
    /// NUL-terminated immutable data (the NUL is implicit, not stored).
    String {
        /// Raw bytes without the trailing NUL.
        bytes: Vec<u8>,
    },
    /// Aggregate constant composed of other constants.
    Aggregate {
        /// Struct or array type of the constant.
        ty: TypeId,
        /// Element constants, in order.
        elements: Vec<ConstantId>,
    },
}

/// Interning table for constants.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConstantStore {
    constants: Vec<ConstantData>,
    lookup: HashMap<ConstantData, ConstantId>,
}

impl ConstantStore {
    /// Creates an empty pool.
    pub fn new() -> Self {
        ConstantStore {
            constants: Vec::new(),
            lookup: HashMap::new(),
        }
    }

    /// Interns a constant, returning its canonical [`ConstantId`].
    pub fn intern(&mut self, data: ConstantData) -> ConstantId {
        if let Some(&id) = self.lookup.get(&data) {
            return id;
        }
        let id = ConstantId::new(self.constants.len() as u32);
        self.constants.push(data.clone());
        self.lookup.insert(data, id);
        id
    }

    /// Returns the data for `id`, or `None` if the ID is invalid.
    pub fn get(&self, id: ConstantId) -> Option<&ConstantData> {
        self.constants.get(id.index())
    }

    /// Number of interned constants.
    pub fn len(&self) -> usize {
        self.constants.len()
    }

    /// Returns true if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.constants.is_empty()
    }

    /// Masks a signed value to `width` bits.
    pub fn mask_signed(value: i128, width: u32) -> u128 {
        (value as u128) & Self::mask(width)
    }

    /// Truncates an unsigned value to `width` bits.
    pub fn mask_unsigned(value: u128, width: u32) -> u128 {
        value & Self::mask(width)
    }

    /// All-ones mask for `width` bits.
    pub fn mask(width: u32) -> u128 {
        if width >= 128 {
            u128::MAX
        } else {
            (1u128 << width) - 1
        }
    }

    /// Sign-extends the low `width` bits of `bits` to `i128`.
    pub fn sign_extend(bits: u128, width: u32) -> i128 {
        if width >= 128 {
            bits as i128
        } else {
            let shift = 128 - width;
            ((bits << shift) as i128) >> shift
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_deduplicates() {
        let mut pool = ConstantStore::new();
        let a = pool.intern(ConstantData::Int {
            ty: TypeId::I64,
            width: 64,
            bits: 42,
        });
        let b = pool.intern(ConstantData::Int {
            ty: TypeId::I64,
            width: 64,
            bits: 42,
        });
        assert_eq!(a, b);

        let s1 = pool.intern(ConstantData::String {
            bytes: b"hello".to_vec(),
        });
        let s2 = pool.intern(ConstantData::String {
            bytes: b"hello".to_vec(),
        });
        assert_eq!(s1, s2);
    }

    #[test]
    fn sign_extension() {
        assert_eq!(ConstantStore::sign_extend(0xFF, 8), -1);
        assert_eq!(ConstantStore::sign_extend(0x7F, 8), 127);
        assert_eq!(ConstantStore::sign_extend(u128::MAX, 128), -1);
    }
}
