//! Case-insensitive string interning for CoolBasic identifiers.

use std::collections::HashMap;

/// Interned string identifier — a lightweight, copyable handle.
///
/// Two `Symbol`s compare equal iff they were interned from strings that are
/// identical after Unicode-aware lowercasing (CoolBasic identifiers are
/// case-insensitive).
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Symbol(u32);

impl Symbol {
    /// Placeholder symbol for positions where a real symbol is not yet available.
    pub const DUMMY: Symbol = Symbol(u32::MAX);
}

impl std::fmt::Debug for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Symbol({})", self.0)
    }
}

/// Interns strings with case-insensitive deduplication.
///
/// All strings are normalized to lowercase before storage, so `"Foo"` and
/// `"foo"` produce the same [`Symbol`].
#[derive(Debug, Default)]
pub struct Interner {
    map: HashMap<String, Symbol>,
    strings: Vec<String>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a name, returning a stable [`Symbol`] handle.
    ///
    /// The name is lowercased before lookup/insertion.
    pub fn intern(&mut self, name: &str) -> Symbol {
        let key = name.to_lowercase();
        if let Some(&sym) = self.map.get(&key) {
            return sym;
        }
        let sym = Symbol(self.strings.len() as u32);
        self.strings.push(key.clone());
        self.map.insert(key, sym);
        sym
    }

    /// Resolve a symbol back to its canonical (lowercased) string.
    ///
    /// # Panics
    ///
    /// Panics if `sym` was not produced by this interner.
    pub fn resolve(&self, sym: Symbol) -> &str {
        &self.strings[sym.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive() {
        let mut i = Interner::new();
        let a = i.intern("Foo");
        let b = i.intern("foo");
        let c = i.intern("FOO");
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn distinct_names() {
        let mut i = Interner::new();
        let a = i.intern("alpha");
        let b = i.intern("beta");
        assert_ne!(a, b);
    }

    #[test]
    fn resolve_round_trip() {
        let mut i = Interner::new();
        let sym = i.intern("MyVar");
        assert_eq!(i.resolve(sym), "myvar");
    }

    #[test]
    fn empty_string() {
        let mut i = Interner::new();
        let sym = i.intern("");
        assert_eq!(i.resolve(sym), "");
    }

    #[test]
    fn dummy_symbol_is_distinct() {
        let mut i = Interner::new();
        let real = i.intern("x");
        assert_ne!(real, Symbol::DUMMY);
    }
}
