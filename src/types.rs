//! The Astronomy type system and the interned [`TypeStore`] (§8, §9).
//!
//! Every type used by the IR is interned: structurally identical types share
//! one [`TypeId`], so type equality is pointer-cheap index comparison and no
//! type is ever cloned per-instruction.

use std::collections::HashMap;
use std::fmt;

use crate::id::TypeId;

/// Float kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatKind {
    /// IEEE 754 binary32.
    F32,
    /// IEEE 754 binary64.
    F64,
}

impl FloatKind {
    /// Width of the float format in bits.
    pub fn bits(self) -> u32 {
        match self {
            FloatKind::F32 => 32,
            FloatKind::F64 => 64,
        }
    }
}

impl fmt::Display for FloatKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FloatKind::F32 => f.write_str("f32"),
            FloatKind::F64 => f.write_str("f64"),
        }
    }
}

/// A structural description of a type. Never stored directly in the IR;
/// always interned through a [`TypeStore`] and referenced by [`TypeId`].
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeData {
    /// The empty type; only valid as a function result type.
    Void,
    /// Integer type. `bits` is the exact bit width (1..=128).
    /// `i1` is the boolean type and is always unsigned.
    Int {
        /// Bit width.
        bits: u32,
        /// Whether arithmetic and comparisons treat the value as signed.
        signed: bool,
    },
    /// Floating point type.
    Float(FloatKind),
    /// Pointer type. The pointee type is carried for `load`/`store` typing.
    Pointer {
        /// Type pointed to.
        pointee: TypeId,
        /// Address space index (0 = default).
        address_space: u32,
    },
    /// Fixed-size array type.
    Array {
        /// Element type.
        element: TypeId,
        /// Element count.
        length: u64,
    },
    /// Anonymous structural struct type.
    Struct {
        /// Field types, in order.
        fields: Vec<TypeId>,
    },
    /// Function signature type (used for `extern` declarations and pointers).
    Function {
        /// Parameter types.
        params: Vec<TypeId>,
        /// Whether extra arguments of any first-class type may be appended.
        variadic: bool,
        /// Result type (may be `void`).
        result: TypeId,
    },
}

impl TypeData {
    /// Returns true for any integer type (including `i1`).
    pub fn is_integer(&self) -> bool {
        matches!(self, TypeData::Int { .. })
    }

    /// Returns true for `i8`..`i128` / `u8`..`u128` (arithmetically usable integers).
    pub fn is_arith_integer(&self) -> bool {
        matches!(self, TypeData::Int { bits, .. } if *bits >= 8)
    }

    /// Returns true for float types.
    pub fn is_float(&self) -> bool {
        matches!(self, TypeData::Float(_))
    }

    /// Returns true for pointer types.
    pub fn is_pointer(&self) -> bool {
        matches!(self, TypeData::Pointer { .. })
    }

    /// Returns true for struct and array types.
    pub fn is_aggregate(&self) -> bool {
        matches!(self, TypeData::Struct { .. } | TypeData::Array { .. })
    }

    /// Returns true for function signature types.
    pub fn is_function(&self) -> bool {
        matches!(self, TypeData::Function { .. })
    }

    /// Returns true for `void`.
    pub fn is_void(&self) -> bool {
        matches!(self, TypeData::Void)
    }

    /// Types that can live in SSA values and memory:
    /// integers, floats, pointers and aggregates. Excludes `void` and
    /// function signatures.
    pub fn is_first_class(&self) -> bool {
        self.is_integer() || self.is_float() || self.is_pointer() || self.is_aggregate()
    }

    /// Bit width if this is an integer type.
    pub fn int_bits(&self) -> Option<u32> {
        match self {
            TypeData::Int { bits, .. } => Some(*bits),
            _ => None,
        }
    }

    /// Signedness if this is an integer type wider than `i1`.
    pub fn is_signed_int(&self) -> Option<bool> {
        match self {
            TypeData::Int { signed, bits } if *bits > 1 => Some(*signed),
            _ => None,
        }
    }
}

/// Interning table for types.
///
/// Every [`crate::Module`] owns exactly one `TypeStore`. A `TypeId` obtained
/// from one store is only meaningful inside that module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeStore {
    types: Vec<TypeData>,
    intern: HashMap<TypeData, TypeId>,
}

impl TypeStore {
    /// Creates a store with all scalar types pre-interned, in the exact
    /// order of the [`TypeId`] constants (`VOID` .. `F64`).
    pub fn new() -> Self {
        let mut store = TypeStore {
            types: Vec::with_capacity(32),
            intern: HashMap::new(),
        };
        let scalars = [
            TypeData::Void,
            TypeData::Int { bits: 1, signed: false },
            TypeData::Int { bits: 8, signed: true },
            TypeData::Int { bits: 16, signed: true },
            TypeData::Int { bits: 32, signed: true },
            TypeData::Int { bits: 64, signed: true },
            TypeData::Int { bits: 128, signed: true },
            TypeData::Int { bits: 8, signed: false },
            TypeData::Int { bits: 16, signed: false },
            TypeData::Int { bits: 32, signed: false },
            TypeData::Int { bits: 64, signed: false },
            TypeData::Int { bits: 128, signed: false },
            TypeData::Float(FloatKind::F32),
            TypeData::Float(FloatKind::F64),
        ];
        for data in scalars {
            store.intern(data);
        }
        debug_assert_eq!(store.types.len(), 14);
        store
    }

    /// Interns a type, returning its canonical [`TypeId`].
    pub fn intern(&mut self, data: TypeData) -> TypeId {
        if let Some(&id) = self.intern.get(&data) {
            return id;
        }
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(data.clone());
        self.intern.insert(data, id);
        id
    }

    /// Returns the structural data for `id`, or `None` if the ID is invalid.
    pub fn get(&self, id: TypeId) -> Option<&TypeData> {
        self.types.get(id.index())
    }

    /// Number of interned types.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Returns true when no types are interned (never true for a store
    /// created with [`TypeStore::new`]).
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Interns `ptr<pointee>` in address space 0.
    pub fn ptr(&mut self, pointee: TypeId) -> TypeId {
        self.ptr_in(pointee, 0)
    }

    /// Interns `ptr<pointee>` in a specific address space.
    pub fn ptr_in(&mut self, pointee: TypeId, address_space: u32) -> TypeId {
        self.intern(TypeData::Pointer {
            pointee,
            address_space,
        })
    }

    /// Interns `array<element, length>`.
    pub fn array(&mut self, element: TypeId, length: u64) -> TypeId {
        self.intern(TypeData::Array { element, length })
    }

    /// Interns `struct<...fields>`.
    pub fn struct_of(&mut self, fields: &[TypeId]) -> TypeId {
        self.intern(TypeData::Struct {
            fields: fields.to_vec(),
        })
    }

    /// Interns `fn(params...) -> result`.
    pub fn function(&mut self, params: &[TypeId], variadic: bool, result: TypeId) -> TypeId {
        self.intern(TypeData::Function {
            params: params.to_vec(),
            variadic,
            result,
        })
    }
}

impl Default for TypeStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Formats a type using the canonical ARN type syntax.
///
/// Returns `"<invalid-type>"` for out-of-range IDs so that diagnostics never
/// panic on malformed input.
pub fn type_to_string(store: &TypeStore, id: TypeId) -> String {
    match store.get(id) {
        None => "<invalid-type>".to_string(),
        Some(TypeData::Void) => "void".to_string(),
        Some(TypeData::Int { bits: 1, .. }) => "i1".to_string(),
        Some(TypeData::Int {
            bits,
            signed: true,
        }) => format!("i{bits}"),
        Some(TypeData::Int {
            bits,
            signed: false,
        }) => format!("u{bits}"),
        Some(TypeData::Float(k)) => k.to_string(),
        Some(TypeData::Pointer {
            pointee,
            address_space: 0,
        }) => format!("ptr<{}>", type_to_string(store, *pointee)),
        Some(TypeData::Pointer {
            pointee,
            address_space,
        }) => format!(
            "ptr<{}, {}>",
            type_to_string(store, *pointee),
            address_space
        ),
        Some(TypeData::Array { element, length }) => {
            format!("array<{}, {}>", type_to_string(store, *element), length)
        }
        Some(TypeData::Struct { fields }) => {
            let inner: Vec<String> = fields
                .iter()
                .map(|&f| type_to_string(store, f))
                .collect();
            format!("struct<{}>", inner.join(", "))
        }
        Some(TypeData::Function {
            params,
            variadic,
            result,
        }) => {
            let mut parts: Vec<String> = params
                .iter()
                .map(|&p| type_to_string(store, p))
                .collect();
            if *variadic {
                parts.push("...".to_string());
            }
            format!("fn({}) -> {}", parts.join(", "), type_to_string(store, *result))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_consts_match_intern_order() {
        let store = TypeStore::new();
        assert_eq!(store.get(TypeId::VOID), Some(&TypeData::Void));
        assert_eq!(
            store.get(TypeId::I64),
            Some(&TypeData::Int {
                bits: 64,
                signed: true
            })
        );
        assert_eq!(
            store.get(TypeId::U32),
            Some(&TypeData::Int {
                bits: 32,
                signed: false
            })
        );
        assert_eq!(store.get(TypeId::F64), Some(&TypeData::Float(FloatKind::F64)));
    }

    #[test]
    fn interning_is_deduplicating() {
        let mut store = TypeStore::new();
        let p1 = store.ptr(TypeId::I8);
        let p2 = store.ptr(TypeId::I8);
        assert_eq!(p1, p2);

        let f1 = store.function(&[TypeId::I32, TypeId::I32], false, TypeId::I32);
        let f2 = store.function(&[TypeId::I32, TypeId::I32], false, TypeId::I32);
        assert_eq!(f1, f2);

        let a = store.array(TypeId::I8, 10);
        assert_ne!(a, p1);
        assert_eq!(
            type_to_string(&store, a),
            "array<i8, 10>".to_string()
        );
        assert_eq!(type_to_string(&store, p1), "ptr<i8>".to_string());
        assert_eq!(
            type_to_string(&store, f1),
            "fn(i32, i32) -> i32".to_string()
        );
    }
}
