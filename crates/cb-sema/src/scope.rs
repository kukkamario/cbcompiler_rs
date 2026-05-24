//! Symbol table and scope management for CoolBasic's scoping rules.

use std::collections::HashMap;

use cb_diagnostics::{Span, Symbol};

use crate::types::Type;

/// Index into the `SymbolTable`'s scope list.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ScopeId(pub(crate) u32);

/// Scope tree holding all declarations.
pub struct SymbolTable {
    scopes: Vec<Scope>,
}

struct Scope {
    parent: Option<ScopeId>,
    kind: ScopeKind,
    symbols: HashMap<Symbol, Declaration>,
}

/// What kind of scope this is — affects name-lookup visibility rules.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ScopeKind {
    TopLevel,
    Function,
}

/// A single declared name in a scope.
#[derive(Clone, Debug)]
pub struct Declaration {
    pub kind: DeclKind,
    pub ty: Type,
    pub span: Span,
    pub is_global: bool,
}

/// What a declaration refers to.
#[derive(Clone, Debug)]
pub enum DeclKind {
    Variable,
    Constant { value: ConstValue },
    Function { params: Vec<ParamInfo>, return_ty: Type, scope: Option<ScopeId> },
    TypeDef { fields: Vec<FieldInfo> },
    StructDef { fields: Vec<FieldInfo> },
    Label,
    RuntimeFn { params: Vec<ParamInfo>, return_ty: Type, c_symbol: String },
    OverloadSet { variants: Vec<OverloadVariant> },
}

/// One variant of an overloaded runtime function.
#[derive(Clone, Debug)]
pub struct OverloadVariant {
    pub params: Vec<ParamInfo>,
    pub return_ty: Type,
    pub c_symbol: String,
}

/// Compile-time constant value.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(std::string::String),
}

/// Parameter metadata for a function declaration.
#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: Symbol,
    pub ty: Type,
    pub has_default: bool,
}

/// Field metadata for a Type or Struct definition.
#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: Symbol,
    pub ty: Type,
    pub span: Span,
}

impl SymbolTable {
    pub(crate) fn new() -> Self {
        Self { scopes: Vec::new() }
    }

    /// Create a new scope and return its id.
    pub(crate) fn push_scope(&mut self, kind: ScopeKind, parent: Option<ScopeId>) -> ScopeId {
        let id = ScopeId(self.scopes.len() as u32);
        self.scopes.push(Scope {
            parent,
            kind,
            symbols: HashMap::new(),
        });
        id
    }

    /// Insert a declaration into a scope. Returns `Err` with the existing
    /// declaration's span if the name is already declared in this scope.
    pub(crate) fn declare(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        decl: Declaration,
    ) -> Result<(), Span> {
        let s = &mut self.scopes[scope.0 as usize];
        if let Some(existing) = s.symbols.get(&name) {
            return Err(existing.span);
        }
        s.symbols.insert(name, decl);
        Ok(())
    }

    /// Look up a name following CoolBasic's visibility rules.
    ///
    /// From a function scope:
    /// - Local symbols (this scope)
    /// - Globals (top-level symbols with `is_global == true`)
    /// - Hoisted items: Functions, TypeDefs, StructDefs from the top-level scope
    /// - NOT ordinary top-level variables
    pub(crate) fn lookup(&self, scope: ScopeId, name: Symbol) -> Option<&Declaration> {
        let s = &self.scopes[scope.0 as usize];

        // Check the current scope first.
        if let Some(decl) = s.symbols.get(&name) {
            return Some(decl);
        }

        // Walk parents.
        let mut parent = s.parent;
        let from_function = s.kind == ScopeKind::Function;

        while let Some(pid) = parent {
            let ps = &self.scopes[pid.0 as usize];
            if let Some(decl) = ps.symbols.get(&name) {
                if from_function && ps.kind == ScopeKind::TopLevel {
                    // From a function scope looking into top-level:
                    // only globals and hoisted items are visible.
                    let visible = decl.is_global
                        || matches!(
                            decl.kind,
                            DeclKind::Function { .. }
                                | DeclKind::TypeDef { .. }
                                | DeclKind::StructDef { .. }
                                | DeclKind::Constant { .. }
                                | DeclKind::Label
                                | DeclKind::RuntimeFn { .. }
                                | DeclKind::OverloadSet { .. }
                        );
                    if visible {
                        return Some(decl);
                    }
                } else {
                    return Some(decl);
                }
            }
            parent = ps.parent;
        }

        None
    }

    /// Iterate over all symbols declared directly in a scope.
    pub(crate) fn iter_scope(
        &self,
        scope: ScopeId,
    ) -> impl Iterator<Item = (Symbol, &Declaration)> {
        self.scopes[scope.0 as usize]
            .symbols
            .iter()
            .map(|(&sym, decl)| (sym, decl))
    }

    /// Store the scope ID in a function declaration (set during pass 2).
    pub(crate) fn update_function_scope(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        fn_scope: ScopeId,
    ) {
        let s = &mut self.scopes[scope.0 as usize];
        if let Some(decl) = s.symbols.get_mut(&name)
            && let DeclKind::Function { scope: ref mut s, .. } = decl.kind
        {
            *s = Some(fn_scope);
        }
    }

    /// Update the ConstValue of an existing Constant declaration.
    pub(crate) fn update_const_value(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        value: ConstValue,
    ) {
        let s = &mut self.scopes[scope.0 as usize];
        if let Some(decl) = s.symbols.get_mut(&name)
            && let DeclKind::Constant { value: ref mut v } = decl.kind
        {
            *v = value;
        }
    }

}
