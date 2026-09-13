//! Memory layout of Astronomy types on x86-64 (System V AMD64).
//!
//! The backend is deliberately simple: every SSA value lives in a stack
//! slot, so a value's *size and alignment* fully determine how it is moved
//! into and out of memory. Aggregates use a C-like layout (each field
//! aligned to its own alignment, the aggregate padded to its alignment),
//! which makes `alloca`/`load`/`store`/`construct`/`extract`/`insert`
//! consistent with aggregate constants.
//!
//! This module also decides which types the backend supports at all.
//! `i128`/`u128` are rejected with a structured error rather than lowered
//! incorrectly.

use astronomy::{FloatKind, TypeData, TypeId, TypeStore};

use crate::error::BackendError;

/// Size of a pointer on x86-64.
pub const POINTER_SIZE: u64 = 8;
/// Alignment of a pointer on x86-64.
pub const POINTER_ALIGN: u64 = 8;

/// Rounds `value` up to the next multiple of `align` (`align` a power of two).
/// Returns `None` when the aligned value would overflow.
pub fn align_up(value: u64, align: u64) -> Option<u64> {
    debug_assert!(align.is_power_of_two());
    value.checked_add(align - 1).map(|v| v & !(align - 1))
}

/// Number of bytes an integer of `bits` width occupies (at least one byte).
pub fn int_bytes(bits: u32) -> u64 {
    (bits as u64).div_ceil(8).max(1)
}

/// Natural alignment of an integer of `bits` width.
pub fn int_align(bits: u32) -> u64 {
    match bits {
        0..=8 => 1,
        9..=16 => 2,
        17..=32 => 4,
        33..=64 => 8,
        _ => 16,
    }
}

fn unsupported(ty: TypeId, store: &TypeStore, context: &str, reason: &str) -> BackendError {
    BackendError::UnsupportedType {
        context: context.to_string(),
        ty: astronomy::types::type_to_string(store, ty),
        reason: reason.to_string(),
    }
}

/// Validates that `ty` can be represented by this backend.
///
/// Rejects `i128`/`u128`, function signature types and `void`, recursively
/// through aggregate fields (so `struct<i64, i128>` is rejected too).
/// Pointers are fine regardless of pointee: a pointer is always 8 bytes.
pub fn check_supported(store: &TypeStore, ty: TypeId, context: &str) -> Result<(), BackendError> {
    match store.get(ty) {
        Some(TypeData::Void) => Err(unsupported(
            ty,
            store,
            context,
            "`void` is not a value type",
        )),
        Some(TypeData::Int { bits, .. }) if *bits > 64 => Err(unsupported(
            ty,
            store,
            context,
            "128-bit integers are not supported by this backend",
        )),
        Some(TypeData::Int {
            bits: 1 | 8 | 16 | 32 | 64,
            ..
        }) => Ok(()),
        Some(TypeData::Int { .. }) => Err(unsupported(
            ty,
            store,
            context,
            "integer width must be 1, 8, 16, 32 or 64 bits",
        )),
        Some(TypeData::Float(_)) => Ok(()),
        Some(TypeData::Pointer { .. }) => Ok(()),
        Some(TypeData::Array { element, .. }) => check_supported(store, *element, context),
        Some(TypeData::Struct { fields }) => {
            for &field in fields {
                check_supported(store, field, context)?;
            }
            Ok(())
        }
        Some(TypeData::Function { .. }) => Err(unsupported(
            ty,
            store,
            context,
            "function signature types are not first-class",
        )),
        None => Err(unsupported(ty, store, context, "unknown type")),
    }
}

/// Size in bytes of a supported type.
pub fn size_of(store: &TypeStore, ty: TypeId) -> Result<u64, BackendError> {
    let data = store
        .get(ty)
        .ok_or_else(|| unsupported(ty, store, "size_of", "unknown type"))?;
    match data {
        TypeData::Void => Ok(0),
        TypeData::Int { bits, .. } => Ok(int_bytes(*bits)),
        TypeData::Float(FloatKind::F32) => Ok(4),
        TypeData::Float(FloatKind::F64) => Ok(8),
        TypeData::Pointer { .. } => Ok(POINTER_SIZE),
        TypeData::Array { element, length } => {
            let elem = size_of(store, *element)?;
            elem.checked_mul(*length)
                .ok_or_else(|| unsupported(ty, store, "size_of", "array size overflows u64"))
        }
        TypeData::Struct { fields } => {
            let (size, _) = struct_layout(store, ty, fields)?;
            Ok(size)
        }
        TypeData::Function { .. } => Err(unsupported(
            ty,
            store,
            "size_of",
            "function signature types have no size",
        )),
    }
}

/// Alignment in bytes of a supported type.
pub fn align_of(store: &TypeStore, ty: TypeId) -> Result<u64, BackendError> {
    let data = store
        .get(ty)
        .ok_or_else(|| unsupported(ty, store, "align_of", "unknown type"))?;
    match data {
        TypeData::Void => Ok(1),
        TypeData::Int { bits, .. } => Ok(int_align(*bits)),
        TypeData::Float(FloatKind::F32) => Ok(4),
        TypeData::Float(FloatKind::F64) => Ok(8),
        TypeData::Pointer { .. } => Ok(POINTER_ALIGN),
        TypeData::Array { element, .. } => align_of(store, *element),
        TypeData::Struct { fields } => {
            let (_, align) = struct_layout(store, ty, fields)?;
            Ok(align)
        }
        TypeData::Function { .. } => Err(unsupported(
            ty,
            store,
            "align_of",
            "function signature types have no alignment",
        )),
    }
}

/// Field offsets and total size of a structural struct.
fn struct_layout(store: &TypeStore, ty: TypeId, fields: &[TypeId]) -> Result<(u64, u64), BackendError> {
    let overflow = || unsupported(ty, store, "struct layout", "struct size overflows u64");
    let mut offset = 0u64;
    let mut align = 1u64;
    for &field in fields {
        let field_align = align_of(store, field)?;
        let field_size = size_of(store, field)?;
        offset = align_up(offset, field_align).ok_or_else(overflow)?;
        offset = offset.checked_add(field_size).ok_or_else(overflow)?;
        align = align.max(field_align);
    }
    Ok((align_up(offset, align).ok_or_else(overflow)?, align))
}

/// Type of field `index` of an aggregate.
pub fn field_type(store: &TypeStore, ty: TypeId, index: u64) -> Result<TypeId, BackendError> {
    match store.get(ty) {
        Some(TypeData::Array { element, .. }) => Ok(*element),
        Some(TypeData::Struct { fields }) => fields
            .get(index as usize)
            .copied()
            .ok_or_else(|| BackendError::InvalidModule {
                reason: format!("field {index} out of range for aggregate"),
            }),
        _ => Err(unsupported(
            ty,
            store,
            "field_type",
            "not an aggregate type",
        )),
    }
}

/// Byte offset of field `index` of an aggregate.
pub fn field_offset(store: &TypeStore, ty: TypeId, index: u64) -> Result<u64, BackendError> {
    let overflow = || unsupported(ty, store, "field_offset", "field offset overflows u64");
    match store.get(ty) {
        Some(TypeData::Array { element, .. }) => {
            let elem = size_of(store, *element)?;
            elem.checked_mul(index).ok_or_else(overflow)
        }
        Some(TypeData::Struct { fields }) => {
            if index as usize >= fields.len() {
                return Err(BackendError::InvalidModule {
                    reason: format!("field {index} out of range for struct"),
                });
            }
            let mut offset = 0u64;
            for &field in &fields[..index as usize] {
                offset = align_up(offset, align_of(store, field)?).ok_or_else(overflow)?;
                offset = offset
                    .checked_add(size_of(store, field)?)
                    .ok_or_else(overflow)?;
            }
            offset = align_up(offset, align_of(store, fields[index as usize])?)
                .ok_or_else(overflow)?;
            Ok(offset)
        }
        _ => Err(unsupported(
            ty,
            store,
            "field_offset",
            "not an aggregate type",
        )),
    }
}

/// True when `ty` is a struct or array (copied with `rep movsb`).
pub fn is_aggregate(store: &TypeStore, ty: TypeId) -> bool {
    matches!(
        store.get(ty),
        Some(TypeData::Struct { .. }) | Some(TypeData::Array { .. })
    )
}

/// True when `ty` is `f32`.
pub fn is_f32(store: &TypeStore, ty: TypeId) -> bool {
    matches!(store.get(ty), Some(TypeData::Float(FloatKind::F32)))
}

/// True when `ty` is a float.
pub fn is_float(store: &TypeStore, ty: TypeId) -> bool {
    matches!(store.get(ty), Some(TypeData::Float(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use astronomy::TypeId;

    fn store_with(f: impl FnOnce(&mut TypeStore)) -> TypeStore {
        let mut store = TypeStore::new();
        f(&mut store);
        store
    }

    #[test]
    fn scalar_sizes_and_alignments() {
        let store = TypeStore::new();
        assert_eq!(size_of(&store, TypeId::I1).unwrap(), 1);
        assert_eq!(size_of(&store, TypeId::U8).unwrap(), 1);
        assert_eq!(size_of(&store, TypeId::I16).unwrap(), 2);
        assert_eq!(size_of(&store, TypeId::U32).unwrap(), 4);
        assert_eq!(size_of(&store, TypeId::I64).unwrap(), 8);
        assert_eq!(size_of(&store, TypeId::F32).unwrap(), 4);
        assert_eq!(size_of(&store, TypeId::F64).unwrap(), 8);
        assert_eq!(align_of(&store, TypeId::I1).unwrap(), 1);
        assert_eq!(align_of(&store, TypeId::I64).unwrap(), 8);
    }

    #[test]
    fn struct_layout_pads_fields() {
        let store = store_with(|s| {
            s.struct_of(&[TypeId::I8, TypeId::I64]);
        });
        let ty = TypeId::new(14);
        assert_eq!(size_of(&store, ty).unwrap(), 16);
        assert_eq!(align_of(&store, ty).unwrap(), 8);
        assert_eq!(field_offset(&store, ty, 0).unwrap(), 0);
        assert_eq!(field_offset(&store, ty, 1).unwrap(), 8);
    }

    #[test]
    fn array_layout_is_contiguous() {
        let store = store_with(|s| {
            s.array(TypeId::I32, 4);
        });
        let ty = TypeId::new(14);
        assert_eq!(size_of(&store, ty).unwrap(), 16);
        assert_eq!(align_of(&store, ty).unwrap(), 4);
        assert_eq!(field_offset(&store, ty, 3).unwrap(), 12);
        assert_eq!(field_type(&store, ty, 3).unwrap(), TypeId::I32);
    }

    #[test]
    fn pointer_is_eight_bytes() {
        let store = store_with(|s| {
            s.ptr(TypeId::I64);
        });
        let ty = TypeId::new(14);
        assert_eq!(size_of(&store, ty).unwrap(), 8);
        assert_eq!(align_of(&store, ty).unwrap(), 8);
    }

    #[test]
    fn i128_is_unsupported_recursively() {
        let store = store_with(|s| {
            s.struct_of(&[TypeId::I64, TypeId::I128]);
        });
        let ty = TypeId::new(14);
        assert_eq!(
            check_supported(&store, TypeId::I128, "test").unwrap_err().code(),
            "A-NASM-001"
        );
        assert_eq!(
            check_supported(&store, ty, "test").unwrap_err().code(),
            "A-NASM-001"
        );
    }

    #[test]
    fn align_up_rounds_to_power_of_two() {
        assert_eq!(align_up(0, 8), Some(0));
        assert_eq!(align_up(1, 8), Some(8));
        assert_eq!(align_up(8, 8), Some(8));
        assert_eq!(align_up(9, 16), Some(16));
        assert_eq!(align_up(u64::MAX, 8), None);
    }
}
