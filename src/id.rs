//! Opaque numeric identifier newtypes used across the Astronomy Core IR.
//!
//! Astronomy minimizes string references inside the IR (§10 Numeric IDs).
//! Every entity is referenced through a small, `Copy`, strongly-typed index.
//! Distinct newtypes (`ValueId`, `BlockId`, ...) make it impossible to
//! accidentally mix identifier spaces.
//!
//! # Trust model
//! IDs are untrusted: they may be fabricated (for example by a deserializer
//! or by low-level direct construction). The [`crate::Verifier`] is the
//! component that guarantees every referenced ID actually exists.

use std::fmt;

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
        pub struct $name(u32);

        impl $name {
            /// Creates an ID from a raw index.
            ///
            /// Existence is *not* guaranteed until the module passes the
            /// [`crate::Verifier`].
            pub fn new(index: u32) -> Self {
                $name(index)
            }

            /// Returns the zero-based index of this ID.
            pub fn index(self) -> usize {
                self.0 as usize
            }

            /// Returns the raw `u32` value.
            pub fn as_u32(self) -> u32 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

define_id!(
    /// Identifies an interned [`crate::TypeData`] inside a [`crate::TypeStore`].
    TypeId,
    "t"
);
define_id!(
    /// Identifies an interned symbol name inside a [`crate::SymbolStore`].
    SymbolId,
    "s"
);
define_id!(
    /// Identifies a constant inside the module constant pool.
    ConstantId,
    "c"
);
define_id!(
    /// Identifies an SSA value inside a single function.
    ValueId,
    "v"
);
define_id!(
    /// Identifies a basic block inside a single function.
    BlockId,
    "bb"
);
define_id!(
    /// Identifies a function (definition or external declaration) in a module.
    FunctionId,
    "f"
);
define_id!(
    /// Reserved for future global variables (not yet part of the MVP model).
    GlobalId,
    "g"
);
define_id!(
    /// Reserved for future metadata nodes (not yet part of the MVP model).
    MetadataId,
    "m"
);

impl TypeId {
    /// Scalar types pre-interned by every [`crate::TypeStore`].
    ///
    /// These constants are valid in every module because `TypeStore::new`
    /// always interns the scalar types first, in exactly this order.
    pub const VOID: TypeId = TypeId(0);
    /// The boolean type (`i1`).
    pub const I1: TypeId = TypeId(1);
    /// The `i8` type.
    pub const I8: TypeId = TypeId(2);
    /// The `i16` type.
    pub const I16: TypeId = TypeId(3);
    /// The `i32` type.
    pub const I32: TypeId = TypeId(4);
    /// The `i64` type.
    pub const I64: TypeId = TypeId(5);
    /// The `i128` type.
    pub const I128: TypeId = TypeId(6);
    /// The `u8` type.
    pub const U8: TypeId = TypeId(7);
    /// The `u16` type.
    pub const U16: TypeId = TypeId(8);
    /// The `u32` type.
    pub const U32: TypeId = TypeId(9);
    /// The `u64` type.
    pub const U64: TypeId = TypeId(10);
    /// The `u128` type.
    pub const U128: TypeId = TypeId(11);
    /// The IEEE binary32 type.
    pub const F32: TypeId = TypeId(12);
    /// The IEEE binary64 type.
    pub const F64: TypeId = TypeId(13);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_do_not_mix() {
        // Compile-time separation; runtime check of distinct spaces.
        let v = ValueId::new(0);
        let b = BlockId::new(0);
        assert_eq!(v.as_u32(), b.as_u32());
        assert_eq!(v.to_string(), "v0");
        assert_eq!(b.to_string(), "bb0");
    }
}
