//! Case-insensitive string interning for CoolBasic identifiers.
//!
//! Identifier identity follows `docs/cb_syntax.md` §1.3: names are compared
//! using **Unicode simple case folding**, not `str::to_lowercase`. The two
//! genuinely differ for some characters (e.g. Greek final sigma `ς`, which
//! folds to `σ` but is left unchanged by lowercasing), and this interner is
//! *the* definition of identifier identity for the whole compiler. The
//! original (first-seen) spelling is preserved so diagnostics echo what the
//! user actually wrote.

use crate::source::alloc_id;
use std::collections::HashMap;

/// Fold one `char` to its Unicode simple-case-folding form.
///
/// Returns the input unchanged when the scalar has no simple fold — Unicode's
/// `case_folded` yields `None` for characters that already are their own fold
/// (ASCII lowercase, `ß`, …).
fn fold_char(c: char) -> char {
    match unicode_case_mapping::case_folded(c) {
        Some(folded) => char::from_u32(folded.get()).unwrap_or(c),
        None => c,
    }
}

/// Compute the Unicode simple-case-fold key for a name.
///
/// This is the canonical form that defines identifier identity: two names are
/// the same identifier iff their folds are equal. Call sites that match a
/// *resolved* name against a fixed spelling (e.g. intrinsic dispatch) must
/// fold it first, since [`Interner::resolve`] returns the original casing, not
/// the fold key.
///
/// Simple case folding is a per-scalar mapping (one `char` in, one `char`
/// out), so folding char-by-char and reconcatenating is correct — there are
/// no cross-character expansions like full folding's `ß` → `ss`.
pub fn fold(name: &str) -> String {
    name.chars().map(fold_char).collect()
}

/// True when folding `name` would change at least one scalar.
///
/// When this is `false`, `name` already equals its own fold key, so a lookup
/// can borrow `name` directly without allocating a folded `String`.
fn needs_fold(name: &str) -> bool {
    name.chars().any(|c| fold_char(c) != c)
}

/// Interned string identifier — a lightweight, copyable handle.
///
/// Two `Symbol`s compare equal iff they were interned from strings that are
/// identical under Unicode simple case folding (CoolBasic identifiers are
/// case-insensitive — `docs/cb_syntax.md` §1.3).
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
/// Names are deduplicated by their Unicode simple-case-fold key, so `"Foo"`
/// and `"foo"` produce the same [`Symbol`]. The **first-seen original
/// spelling** is retained for display via [`Interner::resolve`].
#[derive(Debug, Default)]
pub struct Interner {
    /// Maps a fold key to its symbol.
    map: HashMap<String, Symbol>,
    /// First-seen original spelling for each symbol, indexed by `Symbol.0`.
    strings: Vec<String>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a name, returning a stable [`Symbol`] handle.
    ///
    /// Lookup and deduplication use the name's Unicode simple-case-fold key;
    /// the original spelling of the first occurrence is stored for display.
    ///
    /// # Panics
    ///
    /// Panics if more than `u32::MAX - 1` distinct names are interned — the
    /// last `u32` value is reserved for [`Symbol::DUMMY`], mirroring the
    /// `SourceMap` sentinel discipline.
    pub fn intern(&mut self, name: &str) -> Symbol {
        // Hot path: if `name` is already its own fold key (e.g. all-lowercase
        // ASCII identifiers, the common case), `name == fold(name)`, so we can
        // look it up by borrowing `name` directly — no throwaway `String`. Only
        // genuinely cased input allocates a fold key here.
        if !needs_fold(name) {
            if let Some(&sym) = self.map.get(name) {
                return sym;
            }
            // Miss on a name that needs no folding: the owned key equals the name.
            return self.insert_new(name, name.to_string());
        }
        let key = fold(name);
        if let Some(&sym) = self.map.get(&key) {
            return sym;
        }
        self.insert_new(name, key)
    }

    /// Mint a fresh symbol for `name`, storing `key` as its fold key.
    ///
    /// Caller must have already confirmed `key` is absent from the map.
    fn insert_new(&mut self, name: &str, key: String) -> Symbol {
        let sym = Symbol(alloc_id(self.strings.len(), "interner"));
        self.strings.push(name.to_string());
        self.map.insert(key, sym);
        sym
    }

    /// Resolve a symbol back to the original (first-seen) spelling of its name.
    ///
    /// Casing is preserved for diagnostics — interning `"PlayerHealth"` then
    /// resolving it yields `"PlayerHealth"`, not the folded `"playerhealth"`.
    /// Names that differ only by case share one symbol and resolve to whichever
    /// spelling was interned first.
    ///
    /// # Panics
    ///
    /// Panics if `sym` was not produced by this interner (including
    /// [`Symbol::DUMMY`], which [`Interner::intern`] never mints).
    pub fn resolve(&self, sym: Symbol) -> &str {
        // Explicit guards turn a bare slice-OOB panic into a clear message:
        // DUMMY (u32::MAX) is never minted, and a small index from another
        // interner would otherwise silently misresolve to the wrong string.
        assert!(sym != Symbol::DUMMY, "resolve called on Symbol::DUMMY");
        let idx = sym.0 as usize;
        assert!(
            idx < self.strings.len(),
            "resolve: {sym:?} out of bounds for this interner ({} symbols) — likely a cross-interner or stale symbol",
            self.strings.len(),
        );
        &self.strings[idx]
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
        // resolve() returns the original spelling, not the folded key.
        assert_eq!(i.resolve(sym), "MyVar");
    }

    #[test]
    fn resolve_preserves_first_seen_spelling() {
        let mut i = Interner::new();
        let a = i.intern("PlayerHealth");
        // A later, differently-cased spelling maps to the same symbol, but the
        // first-seen spelling is what diagnostics render.
        let b = i.intern("playerhealth");
        assert_eq!(a, b);
        assert_eq!(i.resolve(a), "PlayerHealth");
        assert_eq!(i.resolve(b), "PlayerHealth");
    }

    #[test]
    fn case_insensitive_nordic() {
        // Common Nordic letters must dedupe case-insensitively.
        let mut i = Interner::new();
        for (upper, lower) in [("Ä", "ä"), ("Ö", "ö"), ("Å", "å")] {
            assert_eq!(i.intern(upper), i.intern(lower), "{upper} vs {lower}");
        }
        let a = i.intern("Hämäläinen");
        let b = i.intern("HÄMÄLÄINEN");
        let c = i.intern("hämäläinen");
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn simple_fold_differs_from_lowercasing() {
        // Greek final sigma `ς` (U+03C2) simple-case-folds to `σ` (U+03C3), as
        // does capital `Σ` (U+03A3) — so all three intern to one symbol.
        // `str::to_lowercase` leaves `ς` as `ς`, so the old lowercasing rule
        // would have treated `ς` and `σ` as distinct names. This pins the fix.
        let mut i = Interner::new();
        let final_sigma = i.intern("ς");
        let small_sigma = i.intern("σ");
        let capital_sigma = i.intern("Σ");
        assert_eq!(final_sigma, small_sigma);
        assert_eq!(small_sigma, capital_sigma);
        // Sanity check that this pair genuinely diverges under lowercasing,
        // i.e. the test would fail against the previous `to_lowercase` rule.
        assert_ne!("ς".to_lowercase(), "σ".to_lowercase());
    }

    #[test]
    fn empty_string() {
        let mut i = Interner::new();
        let sym = i.intern("");
        assert_eq!(i.resolve(sym), "");
    }

    #[test]
    fn dummy_symbol_is_distinct() {
        // The first minted symbol is `Symbol(0)`; `DUMMY` is `Symbol(u32::MAX)`
        // and `intern` guards against ever minting it (the overflow assert is
        // unreachable in practice — 4 billion names — so it has no unit test).
        let mut i = Interner::new();
        let real = i.intern("x");
        assert_ne!(real, Symbol::DUMMY);
    }
}
