//! Interned symbol names (§11 Symbol Interning).
//!
//! Function names and external symbol names are interned once per module.
//! IR structures reference symbols by [`SymbolId`] and never copy `String`s.

use std::collections::HashMap;

use crate::id::SymbolId;

/// Interning table for symbol names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SymbolStore {
    names: Vec<String>,
    lookup: HashMap<String, SymbolId>,
}

impl SymbolStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        SymbolStore {
            names: Vec::new(),
            lookup: HashMap::new(),
        }
    }

    /// Interns `name`, returning its canonical [`SymbolId`].
    pub fn intern(&mut self, name: impl Into<String>) -> SymbolId {
        let name: String = name.into();
        if let Some(&id) = self.lookup.get(&name) {
            return id;
        }
        let id = SymbolId::new(self.names.len() as u32);
        self.names.push(name.clone());
        self.lookup.insert(name, id);
        id
    }

    /// Returns the interned ID for `name` if it already exists.
    pub fn resolve(&self, name: &str) -> Option<SymbolId> {
        self.lookup.get(name).copied()
    }

    /// Returns the name for `id`, or `None` if the ID is invalid.
    pub fn name(&self, id: SymbolId) -> Option<&str> {
        self.names.get(id.index()).map(String::as_str)
    }

    /// Number of interned symbols.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns true if no symbols are interned.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_deduplicates() {
        let mut syms = SymbolStore::new();
        let a = syms.intern("printf");
        let b = syms.intern("printf");
        assert_eq!(a, b);
        assert_eq!(syms.name(a), Some("printf"));
        assert_eq!(syms.resolve("printf"), Some(a));
        assert_eq!(syms.resolve("scanf"), None);
        assert_eq!(syms.len(), 1);
    }
}
