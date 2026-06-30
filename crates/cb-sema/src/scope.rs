//! Symbol table and scope management for CoolBasic's scoping rules.

use std::collections::HashMap;

use cb_diagnostics::{FileId, Span, Symbol};
use cb_frontend::NodeId;

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
    /// Meaningful only for `DeclKind::Variable`: marks a `Global` declaration
    /// (visible inside functions). Other decl kinds are global by their kind
    /// (see `lookup`), so this flag is not consulted for them.
    pub is_global: bool,
}

/// What a declaration refers to.
#[derive(Clone, Debug)]
pub enum DeclKind {
    Variable,
    Constant {
        value: ConstValue,
    },
    Function {
        params: Vec<ParamInfo>,
        return_ty: Type,
        scope: Option<ScopeId>,
        /// `NodeId` of this function's `Stmt::Function` declaration — the stable
        /// identity carried across analysis and lowering so a name shared by
        /// several overloads still maps each call/address-of to one definition.
        def: NodeId,
    },
    TypeDef {
        fields: Vec<FieldInfo>,
    },
    StructDef {
        fields: Vec<FieldInfo>,
    },
    Label,
    RuntimeFn {
        params: Vec<ParamInfo>,
        return_ty: Type,
        c_symbol: String,
    },
    OverloadSet {
        variants: Vec<OverloadVariant>,
    },
    RuntimeTypeDef,
}

impl DeclKind {
    /// Whether a top-level declaration of this kind is visible inside function
    /// bodies regardless of the `is_global` flag — the "hoisted" kinds. Only
    /// `Variable` is *not* hoisted (it needs an explicit `Global`). Drives the
    /// visibility filter in [`SymbolTable::lookup`].
    pub(crate) fn is_hoisted(&self) -> bool {
        matches!(
            self,
            DeclKind::Function { .. }
                | DeclKind::TypeDef { .. }
                | DeclKind::StructDef { .. }
                | DeclKind::Constant { .. }
                | DeclKind::Label
                | DeclKind::RuntimeFn { .. }
                | DeclKind::OverloadSet { .. }
                // Runtime opaque types (e.g. `Object`) are global like runtime
                // functions, so a function body using `As Object` resolves them.
                | DeclKind::RuntimeTypeDef
        )
    }
}

/// One variant of an overload set — either a runtime command or a user
/// function/sub. Variants of one set share a name but differ in signature.
#[derive(Clone, Debug)]
pub struct OverloadVariant {
    pub params: Vec<ParamInfo>,
    pub return_ty: Type,
    pub target: OverloadTarget,
    /// For a user-function variant, its body scope (set in pass 2, as for a
    /// single [`DeclKind::Function`]); `None` for runtime variants.
    pub scope: Option<ScopeId>,
}

/// What an [`OverloadVariant`] dispatches to — the per-variant identity used
/// when a call resolves to it. Runtime commands carry their C symbol; user
/// functions carry the `NodeId` of their declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverloadTarget {
    Runtime { c_symbol: String },
    User { def: NodeId },
}

/// Outcome of [`SymbolTable::declare_user_function`].
pub(crate) enum DeclareFnOutcome {
    /// First definition of this name — inserted as a single function.
    Declared,
    /// Merged with prior same-named definition(s) into an overload set.
    Merged,
    /// A prior definition has an identical signature; the span is the
    /// previous definition's (caller emits the duplicate diagnostic).
    Duplicate(Span),
    /// The name is already taken by a non-function declaration; the span is
    /// the existing declaration's.
    Conflict(Span),
}

/// The `Type` stored on a function declaration: its return type, or `Void`
/// for a sub. Mirrors the rule the checker applies to a single function.
fn fn_decl_ty(return_ty: &Type) -> Type {
    match return_ty {
        Type::Void => Type::Void,
        t => t.clone(),
    }
}

/// Whether two function signatures are indistinguishable for overloading:
/// same parameter types in order and the same return type. Parameter names
/// and defaults are not part of the signature.
fn same_signature(
    a_params: &[ParamInfo],
    a_ret: &Type,
    b_params: &[ParamInfo],
    b_ret: &Type,
) -> bool {
    a_params.len() == b_params.len()
        && a_params.iter().zip(b_params).all(|(x, y)| x.ty == y.ty)
        && a_ret == b_ret
}

/// Compile-time constant value.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
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

    /// Insert `decl` into `scope`, replacing any existing entry for `name`.
    ///
    /// Unlike [`declare`](Self::declare) this never fails on a collision. It is
    /// used when an explicit user declaration is permitted to *shadow* a
    /// runtime-seeded command of the same name — the catalog entry is
    /// overwritten so the name now resolves to the user's declaration.
    pub(crate) fn force_declare(&mut self, scope: ScopeId, name: Symbol, decl: Declaration) {
        debug_assert!(
            self.local_is_runtime_command(scope, name),
            "force_declare is only for shadowing runtime commands"
        );
        self.scopes[scope.0 as usize].symbols.insert(name, decl);
    }

    /// Whether `name` is declared *directly in this scope* as a runtime
    /// *command* — a `RuntimeFn` or `OverloadSet` seeded from the catalog,
    /// identified by its synthetic span.
    ///
    /// Such names may be shadowed by an explicit user declaration. Runtime
    /// constants and opaque types are *not* commands and are intentionally
    /// excluded: those names are reserved (a colliding user declaration is an
    /// error). See [`force_declare`](Self::force_declare).
    pub(crate) fn local_is_runtime_command(&self, scope: ScopeId, name: Symbol) -> bool {
        self.scopes[scope.0 as usize]
            .symbols
            .get(&name)
            .is_some_and(|d| {
                d.span.file == FileId::SYNTHETIC
                    && matches!(
                        d.kind,
                        DeclKind::RuntimeFn { .. } | DeclKind::OverloadSet { .. }
                    )
            })
    }

    /// Declare a user function/sub, merging same-named definitions into an
    /// overload set (cb_syntax.md §7.2). The runtime catalog is registered
    /// *after* `pass1`, so any existing local entry here is another user
    /// declaration, never a runtime command.
    ///
    /// - No prior entry → inserted as a single [`DeclKind::Function`].
    /// - Prior user function/overload set with a *distinguishable* signature →
    ///   upgraded to / extended as a [`DeclKind::OverloadSet`].
    /// - Prior definition with an *identical* signature (same parameter types
    ///   and return type) → [`DeclareFnOutcome::Duplicate`] (caller emits E0319).
    /// - Name taken by any other kind → [`DeclareFnOutcome::Conflict`].
    pub(crate) fn declare_user_function(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        params: Vec<ParamInfo>,
        return_ty: Type,
        span: Span,
        def: NodeId,
    ) -> DeclareFnOutcome {
        let new_variant = OverloadVariant {
            params,
            return_ty,
            target: OverloadTarget::User { def },
            scope: None,
        };
        let s = &mut self.scopes[scope.0 as usize];
        match s.symbols.get_mut(&name) {
            None => {
                let OverloadVariant {
                    params, return_ty, ..
                } = new_variant;
                let ty = fn_decl_ty(&return_ty);
                s.symbols.insert(
                    name,
                    Declaration {
                        kind: DeclKind::Function {
                            params,
                            return_ty,
                            scope: None,
                            def,
                        },
                        ty,
                        span,
                        is_global: false,
                    },
                );
                DeclareFnOutcome::Declared
            }
            Some(existing) => match &mut existing.kind {
                DeclKind::Function {
                    params: ep,
                    return_ty: er,
                    def: edef,
                    ..
                } => {
                    if same_signature(ep, er, &new_variant.params, &new_variant.return_ty) {
                        return DeclareFnOutcome::Duplicate(existing.span);
                    }
                    let prev_variant = OverloadVariant {
                        params: std::mem::take(ep),
                        return_ty: er.clone(),
                        target: OverloadTarget::User { def: *edef },
                        scope: None,
                    };
                    existing.kind = DeclKind::OverloadSet {
                        variants: vec![prev_variant, new_variant],
                    };
                    existing.ty = Type::Void;
                    DeclareFnOutcome::Merged
                }
                DeclKind::OverloadSet { variants } if matches!(variants.first(), Some(v) if matches!(v.target, OverloadTarget::User { .. })) =>
                {
                    if variants.iter().any(|v| {
                        same_signature(
                            &v.params,
                            &v.return_ty,
                            &new_variant.params,
                            &new_variant.return_ty,
                        )
                    }) {
                        return DeclareFnOutcome::Duplicate(existing.span);
                    }
                    variants.push(new_variant);
                    DeclareFnOutcome::Merged
                }
                _ => DeclareFnOutcome::Conflict(existing.span),
            },
        }
    }

    /// Look up a name following CoolBasic's visibility rules.
    ///
    /// From a function scope:
    /// - Local symbols (this scope)
    /// - Globals (top-level symbols with `is_global == true`)
    /// - Hoisted items from the top-level scope (see [`DeclKind::is_hoisted`])
    /// - NOT ordinary top-level variables
    ///
    /// Load-bearing invariant: the scope tree is at most two levels deep —
    /// exactly one `TopLevel` root with `Function` children (functions cannot
    /// nest, §7.1; block constructs do not open scopes). Because of this,
    /// `from_function` is computed once from the *leaf* scope and the visibility
    /// filter only ever sees `TopLevel` parents. If a `Function` scope could
    /// appear as a parent, the `else` branch below would leak that intermediate
    /// function's locals into the inner one. The `debug_assert!` in the walk
    /// guards the invariant.
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
            // Every parent must be the TopLevel root (tree depth ≤ 2); see the
            // doc above for why the visibility filter relies on this.
            debug_assert!(
                ps.kind == ScopeKind::TopLevel,
                "scope tree deeper than TopLevel→Function: parent scope is a Function"
            );
            if let Some(decl) = ps.symbols.get(&name) {
                if from_function && ps.kind == ScopeKind::TopLevel {
                    // From a function scope looking into top-level:
                    // only globals and hoisted items are visible.
                    if decl.is_global || decl.kind.is_hoisted() {
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

    /// Store the body scope of the function defined at `def` (set during pass
    /// 2). For an overloaded name the entry is an `OverloadSet`; the matching
    /// variant is found by its definition `NodeId`.
    pub(crate) fn update_function_scope(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        def: NodeId,
        fn_scope: ScopeId,
    ) {
        let s = &mut self.scopes[scope.0 as usize];
        match s.symbols.get_mut(&name).map(|d| &mut d.kind) {
            Some(DeclKind::Function { scope: s, .. }) => {
                *s = Some(fn_scope);
            }
            Some(DeclKind::OverloadSet { variants }) => {
                if let Some(v) = variants
                    .iter_mut()
                    .find(|v| matches!(v.target, OverloadTarget::User { def: d } if d == def))
                {
                    v.scope = Some(fn_scope);
                }
            }
            _ => debug_assert!(
                false,
                "update_function_scope found no Function decl to update"
            ),
        }
    }

    /// Update the ConstValue of an existing Constant declaration.
    pub(crate) fn update_const_value(&mut self, scope: ScopeId, name: Symbol, value: ConstValue) {
        let s = &mut self.scopes[scope.0 as usize];
        let mut found = false;
        if let Some(decl) = s.symbols.get_mut(&name)
            && let DeclKind::Constant { value: ref mut v } = decl.kind
        {
            *v = value;
            found = true;
        }
        debug_assert!(found, "update_const_value found no Constant decl to update");
    }

    /// Fill in the fields of an existing `TypeDef`/`StructDef` declaration.
    ///
    /// Pass 1 declares every record's name (and kind) up front so forward
    /// references resolve, then resolves field types in a second step once all
    /// names are in scope; this writes the resolved fields back.
    pub(crate) fn update_record_fields(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        fields: Vec<FieldInfo>,
    ) {
        let s = &mut self.scopes[scope.0 as usize];
        let mut found = false;
        if let Some(decl) = s.symbols.get_mut(&name) {
            match &mut decl.kind {
                DeclKind::TypeDef { fields: f } | DeclKind::StructDef { fields: f } => {
                    *f = fields;
                    found = true;
                }
                _ => {}
            }
        }
        debug_assert!(
            found,
            "update_record_fields found no Type/Struct decl to update"
        );
    }
}
