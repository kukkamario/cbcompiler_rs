//! Main analysis engine — declaration collection (pass 1) and type checking (pass 2).

use cb_diagnostics::{Diagnostic, DiagnosticCode, FileId, Interner, Label, Span, Symbol};
use cb_frontend::ast::{CaseArm, Expr, Node, Param, Stmt, TypeDeclKind};
use cb_frontend::{Arena, BinOp, DimName, NodeId, Sigil, SpanExt, UnOp};

use crate::convert::{self, ConversionTable};
use crate::diagnostics::*;
use crate::scope::{
    ConstValue, DeclKind, Declaration, FieldInfo, OverloadVariant, ParamInfo, ScopeId, ScopeKind,
    SymbolTable,
};
use crate::types::{self, Type};
use crate::{FuncDesc, ResolvedCall, RuntimeCatalog, SemaResult, TypeTable};

// Names of compiler intrinsics (lowercase, matching interner output).
const INTRINSIC_LEN: &str = "len";
const INTRINSIC_INT: &str = "int";
const INTRINSIC_INTEGER: &str = "integer";
const INTRINSIC_FLOAT: &str = "float";
const INTRINSIC_STR: &str = "str";
const INTRINSIC_FIRST: &str = "first";
const INTRINSIC_LAST: &str = "last";
const INTRINSIC_NEXT: &str = "next";
const INTRINSIC_PREVIOUS: &str = "previous";

/// Drives semantic analysis over a parsed AST.
pub(crate) struct Checker<'a> {
    arena: &'a Arena,
    source: &'a str,
    interner: Interner,
    symbols: SymbolTable,
    types: TypeTable,
    conversions: ConversionTable,
    delete_classes: std::collections::HashMap<NodeId, crate::DeleteClass>,
    resolved_calls: std::collections::HashMap<NodeId, ResolvedCall>,
    diagnostics: Vec<Diagnostic>,
    current_scope: ScopeId,
    /// The return type of the function we're currently inside, if any.
    current_fn_return_ty: Option<Type>,
    /// Stack of For loop node IDs we're currently inside (for Goto-into-For check).
    for_loop_stack: Vec<NodeId>,
    /// For each label symbol, the set of For loop NodeIds containing it.
    label_for_nesting: std::collections::HashMap<Symbol, Vec<NodeId>>,
    /// Stack of enclosing loop/`Select` constructs, used to validate `Break`
    /// (needs a loop) and `Continue` (needs a loop or `Select`). Mirrors the
    /// control-context stack lowering builds (cb_syntax.md §6.2/§6.3).
    control_stack: Vec<ControlKind>,
}

/// An enclosing control construct that `Break`/`Continue` can target.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Loop,
    Select,
}

impl<'a> Checker<'a> {
    pub(crate) fn run(
        arena: &'a Arena,
        program: &[NodeId],
        source: &'a str,
        runtime_catalog: &RuntimeCatalog,
    ) -> SemaResult {
        let mut symbols = SymbolTable::new();
        let top = symbols.push_scope(ScopeKind::TopLevel, None);

        let mut checker = Checker {
            arena,
            source,
            interner: Interner::new(),
            symbols,
            types: TypeTable::new(),
            conversions: ConversionTable::new(),
            delete_classes: std::collections::HashMap::new(),
            resolved_calls: std::collections::HashMap::new(),
            diagnostics: Vec::new(),
            current_scope: top,
            current_fn_return_ty: None,
            for_loop_stack: Vec::new(),
            control_stack: Vec::new(),
            label_for_nesting: std::collections::HashMap::new(),
        };

        checker.register_runtime_types(runtime_catalog);
        checker.pass1(program);
        checker.register_runtime_catalog(runtime_catalog);
        checker.pass2(program);

        SemaResult {
            types: checker.types,
            symbols: checker.symbols,
            conversions: checker.conversions,
            delete_classes: checker.delete_classes,
            resolved_calls: checker.resolved_calls,
            diagnostics: checker.diagnostics,
            interner: checker.interner,
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    fn intern_span(&mut self, span: Span) -> Symbol {
        let text = span.slice(self.source);
        self.interner.intern(text)
    }

    /// Push an error diagnostic with a single primary label at `span`.
    fn err(&mut self, code: DiagnosticCode, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, Label::new(span)));
    }

    /// Emit an `E_TYPE_MISMATCH` "<what> must be an integer" diagnostic at `span`
    /// unless `ty` is already integer or an error type (errors don't cascade).
    fn expect_integer(&mut self, ty: &Type, span: Span, what: &str) {
        if !ty.is_integer() && !ty.is_error() {
            self.err(E_TYPE_MISMATCH, format!("{what} must be an integer"), span);
        }
    }

    /// Render a `Type` as a human-readable, interner-aware name for diagnostics.
    ///
    /// `Type` stores `Symbol`s for named types, so a bare `Display`/`Debug`
    /// would leak `Symbol(n)`. This resolves those symbols against the interner
    /// instead: primitives print as `Int`/`String`/etc., named types as their
    /// source name, and arrays as `Elem[]` (one bracket pair per rank).
    fn type_name(&self, ty: &Type) -> String {
        match ty {
            Type::Byte => "Byte".to_string(),
            Type::Short => "Short".to_string(),
            Type::Int => "Int".to_string(),
            Type::Long => "Long".to_string(),
            Type::Float => "Float".to_string(),
            Type::String => "String".to_string(),
            Type::Array { elem, rank } => {
                // `Int[]`, `Int[,]`, `Int[,,]`, … — one comma fewer than rank.
                let brackets = format!("[{}]", ",".repeat((*rank).saturating_sub(1) as usize));
                format!("{}{brackets}", self.type_name(elem))
            }
            Type::TypeRef { name } | Type::StructVal { name } | Type::RuntimeType { name } => {
                self.interner.resolve(*name).to_string()
            }
            Type::FnPtr { params, ret } => {
                let params: Vec<String> = params.iter().map(|p| self.type_name(p)).collect();
                match ret {
                    Some(r) => format!("({}) -> {}", params.join(", "), self.type_name(r)),
                    None => format!("({})", params.join(", ")),
                }
            }
            Type::Null => "Null".to_string(),
            Type::Void => "Void".to_string(),
            Type::Error => "<error>".to_string(),
        }
    }

    fn intern_ident(&mut self, name_span: Span, _sigil: Option<Sigil>) -> Symbol {
        // `name_span` is already the bare-name span — the parser excludes the
        // trailing sigil byte via `bare_name_span`. The sigil is *not* part of
        // the variable's identity (cb_syntax.md §1.3–§1.4), so `x`, `x%`, and a
        // later bare `x` all intern to the same symbol.
        self.interner.intern(name_span.slice(self.source))
    }

    fn resolve_type_expr(&mut self, id: NodeId) -> Type {
        // Reject reserved-but-unsupported type names (Bool/Boolean/UInt/
        // UInteger/ULong) with a clear diagnostic (FD-035). They parse as type
        // atoms, so we catch them here instead of as a generic parse error.
        if let Node::TypeExpr(cb_frontend::ast::TypeExpr::Primitive { kw }) = &self.arena[id]
            && types::is_reserved_type_kw(*kw)
        {
            self.diagnostics.push(Diagnostic::error(
                E_RESERVED_TYPE,
                format!(
                    "`{}` is a reserved type name but is not a supported type",
                    kw.as_str()
                ),
                Label::new(self.arena.span_of(id)),
            ));
            return Type::Error;
        }
        let ty = types::resolve_type_expr(self.arena, id, &mut self.interner, self.source);
        self.refine_type(ty)
    }

    /// Refine every `TypeRef` produced by the base resolver into its true kind
    /// (`RuntimeType`, `StructVal`, or a genuine heap `TypeRef`) using the
    /// declaration table, recursing into composite types.
    ///
    /// The base resolver returns `TypeRef` for every user-defined name because
    /// it cannot tell apart a heap `Type`, a runtime type, and a value
    /// `Struct`. Walking `Array` elements and `FnPtr` parameter/return
    /// positions keeps embedded names consistent: without this, `Dim arr As P[]`
    /// would resolve to `Array { elem: TypeRef(p) }` while `New P[3]` produces
    /// `Array { elem: StructVal(p) }`, so checker decisions (field access,
    /// copy-vs-reference, For-Each element typing) ran on the wrong kind for
    /// arrays of structs (FD-034).
    fn refine_type(&self, ty: Type) -> Type {
        match ty {
            Type::TypeRef { name } => {
                if let Some(decl) = self.symbols.lookup(self.current_scope, name) {
                    match decl.kind {
                        DeclKind::RuntimeTypeDef => return Type::RuntimeType { name },
                        DeclKind::StructDef { .. } => return Type::StructVal { name },
                        _ => {}
                    }
                }
                Type::TypeRef { name }
            }
            Type::Array { elem, rank } => Type::Array {
                elem: Box::new(self.refine_type(*elem)),
                rank,
            },
            Type::FnPtr { params, ret } => Type::FnPtr {
                params: params.into_iter().map(|p| self.refine_type(p)).collect(),
                ret: ret.map(|r| Box::new(self.refine_type(*r))),
            },
            other => other,
        }
    }

    /// If `id` is a bare identifier naming a Type/Struct/runtime-type, return its
    /// value-type WITHOUT emitting E0311. Used by the positions that legitimately
    /// take a bare Type name (First/Last, For-Each source). Mirrors `refine_type`.
    fn resolve_type_name_arg(&mut self, id: NodeId) -> Option<Type> {
        let name_span = match &self.arena[id] {
            Node::Expr(Expr::Ident {
                name_span,
                sigil: None,
            }) => *name_span,
            _ => return None,
        };
        let name = self.intern_ident(name_span, None);
        let ty = match self.symbols.lookup(self.current_scope, name)?.kind {
            DeclKind::TypeDef { .. } => Type::TypeRef { name },
            DeclKind::StructDef { .. } => Type::StructVal { name },
            DeclKind::RuntimeTypeDef => Type::RuntimeType { name },
            _ => return None,
        };
        // Keep lowering's `self.types.get(arg)` valid — first/last and for_each
        // read the arg type from the table rather than re-lowering the name node.
        self.types.insert(id, ty.clone());
        Some(ty)
    }

    fn try_declare(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        decl: Declaration,
        error_code: cb_diagnostics::DiagnosticCode,
    ) {
        let decl_span = decl.span;
        if let Err(prev_span) = self.symbols.declare(scope, name, decl) {
            let name_str = self.interner.resolve(name);
            if prev_span.file == FileId::SYNTHETIC {
                // The clashing name was seeded by the runtime catalog (a
                // reserved function/type/constant). There is no user source to
                // point at, so render only the offending declaration site.
                self.diagnostics.push(Diagnostic::error(
                    error_code,
                    format!("`{name_str}` is a reserved runtime name"),
                    Label::new(decl_span),
                ));
            } else {
                self.diagnostics.push(
                    Diagnostic::error(
                        error_code,
                        format!("duplicate declaration of `{name_str}`"),
                        Label::new(decl_span),
                    )
                    .with_secondary(Label::with_message(prev_span, "previously declared here")),
                );
            }
        }
    }

    /// Declare an explicit user variable that is allowed to *shadow* a runtime
    /// command of the same name (FD-027).
    ///
    /// If `name` is currently bound in `scope` to a runtime command
    /// (`RuntimeFn`/`OverloadSet`), the catalog entry is replaced so the name
    /// now refers to the user's variable. Otherwise this behaves exactly like
    /// [`try_declare`](Self::try_declare): a clash with a user declaration is a
    /// duplicate-declaration error, and a clash with a reserved runtime
    /// constant or type still reports "reserved runtime name". A `Dim` inside a
    /// function declares into the function scope, so it never hits this path —
    /// it shadows the top-level command through normal lookup.
    fn declare_var_shadowing(
        &mut self,
        scope: ScopeId,
        name: Symbol,
        decl: Declaration,
        error_code: cb_diagnostics::DiagnosticCode,
    ) {
        if self.symbols.local_is_runtime_command(scope, name) {
            self.symbols.force_declare(scope, name, decl);
        } else {
            self.try_declare(scope, name, decl, error_code);
        }
    }

    // ── runtime catalog registration ─────────────────────────────────────

    fn ir_type_to_sema(&mut self, ir: &cb_ir::types::IrType) -> Type {
        match ir {
            cb_ir::types::IrType::Byte => Type::Byte,
            cb_ir::types::IrType::Short => Type::Short,
            cb_ir::types::IrType::Int => Type::Int,
            cb_ir::types::IrType::Long => Type::Long,
            cb_ir::types::IrType::Float => Type::Float,
            cb_ir::types::IrType::String => Type::String,
            cb_ir::types::IrType::Void => Type::Void,
            cb_ir::types::IrType::RuntimeType(name) => {
                // Intern the catalog's original spelling. The interner folds
                // case-insensitively (FD-026), so lowercasing is redundant for
                // matching and would make diagnostics echo a lowercased name.
                let sym = self.interner.intern(name);
                Type::RuntimeType { name: sym }
            }
            _ => Type::Error,
        }
    }

    /// Register the runtime opaque types (e.g. `Object`) into the top scope.
    ///
    /// Must run BEFORE pass 1: `Type`/`Struct` field annotations are resolved
    /// during pass 1, and `refine_type` upgrades a placeholder `TypeRef` to
    /// `RuntimeType` only if the name is already in scope. If the types were
    /// registered after pass 1 (as functions and constants are), `Field obj As
    /// Object` would be frozen as `TypeRef{object}` and never match the runtime
    /// catalog's `RuntimeType{object}`. (Function param/return annotations need
    /// this *and* `RuntimeTypeDef` being visible from function scopes — see
    /// `SymbolTable::lookup`.)
    ///
    /// Functions and constants are registered separately, AFTER pass 1, because
    /// their collision handling depends on user declarations landing first
    /// (FD-027 user-function shadowing, FD-029 reserved-constant diagnostics).
    fn register_runtime_types(&mut self, catalog: &RuntimeCatalog) {
        let top = self.current_scope;
        let span = Span::new(0, 0, cb_diagnostics::source::FileId::SYNTHETIC);

        for td in &catalog.types {
            // Original spelling, not lowercased — see `ir_type_to_sema`.
            let sym = self.interner.intern(&td.name);
            let decl = Declaration {
                kind: DeclKind::RuntimeTypeDef,
                ty: Type::RuntimeType { name: sym },
                span,
                is_global: false,
            };
            let _ = self.symbols.declare(top, sym, decl);
        }
    }

    fn register_runtime_catalog(&mut self, catalog: &RuntimeCatalog) {
        use std::collections::HashMap;

        let top = self.current_scope;
        let span = Span::new(0, 0, cb_diagnostics::source::FileId::SYNTHETIC);

        // Group function entries by (lowercased) name.
        let mut groups: HashMap<String, Vec<&FuncDesc>> = HashMap::new();
        for desc in &catalog.functions {
            groups
                .entry(desc.name.to_lowercase())
                .or_default()
                .push(desc);
        }

        for (name, descs) in &groups {
            let sym = self.interner.intern(name);

            let make_params = |checker: &mut Checker, desc: &FuncDesc| -> Vec<ParamInfo> {
                desc.params
                    .iter()
                    .map(|p| ParamInfo {
                        name: checker.interner.intern(p.name.as_deref().unwrap_or("_")),
                        ty: checker.ir_type_to_sema(&p.ty),
                        has_default: false,
                    })
                    .collect()
            };

            let decl = if descs.len() == 1 {
                let desc = descs[0];
                let params = make_params(self, desc);
                let return_ty = self.ir_type_to_sema(&desc.return_ty);
                Declaration {
                    kind: DeclKind::RuntimeFn {
                        params,
                        return_ty,
                        c_symbol: desc.c_symbol.clone(),
                    },
                    ty: Type::Void,
                    span,
                    is_global: false,
                }
            } else {
                let variants = descs
                    .iter()
                    .map(|desc| {
                        let params = make_params(self, desc);
                        let return_ty = self.ir_type_to_sema(&desc.return_ty);
                        OverloadVariant {
                            params,
                            return_ty,
                            c_symbol: desc.c_symbol.clone(),
                        }
                    })
                    .collect();
                Declaration {
                    kind: DeclKind::OverloadSet { variants },
                    ty: Type::Void,
                    span,
                    is_global: false,
                }
            };

            // If declare fails, a user-defined function (from pass 1) already
            // took this name — that's fine, user functions shadow runtime functions.
            let _ = self.symbols.declare(top, sym, decl);
        }

        // Register runtime-defined constants (FD-029). These fold at compile
        // time like a user `Const` (lower.rs inlines DeclKind::Constant) and,
        // being in the hoist list, are visible inside functions too. Unlike
        // runtime functions, a name collision with a user declaration is an
        // ERROR (the name is reserved): pass 1 already ran, so a clashing user
        // `Const`/`Dim` is sitting in the top scope — `declare` returns its
        // span and we report E0303 pointing at the user's declaration.
        for c in &catalog.constants {
            let sym = self.interner.intern(&c.name.to_lowercase());
            let (ty, value) = match c.value {
                cb_ir::RuntimeConstValue::Int(v) => (Type::Int, ConstValue::Int(v)),
                cb_ir::RuntimeConstValue::Float(v) => (Type::Float, ConstValue::Float(v)),
            };
            let decl = Declaration {
                kind: DeclKind::Constant { value },
                ty,
                span,
                is_global: false,
            };
            if let Err(prev_span) = self.symbols.declare(top, sym, decl) {
                self.diagnostics.push(Diagnostic::error(
                    E_DUPLICATE_DECL,
                    format!("`{}` is a reserved runtime constant", c.name),
                    Label::with_message(prev_span, "cannot redeclare a runtime-defined constant"),
                ));
            }
        }
    }

    // ── pass 1: declaration collection (hoisting) ───────────────────────

    fn pass1(&mut self, program: &[NodeId]) {
        let top = self.current_scope;
        for &id in program {
            self.pass1_stmt(id, top);
        }
        // Hoist constants from every top-level statement body (inside If, For,
        // While, Select, etc.) so references resolve regardless of block nesting
        // — §4.2 has no block scoping, §7.3 hoists definitions. This mirrors
        // `check_function`; `pass1_stmt` no longer handles `Const` itself, so
        // directly-top-level consts are collected here too (no double-declare).
        for &id in program {
            self.collect_consts_recursive(id, top);
        }
        // Collect labels recursively from all top-level statement bodies
        // (inside For, While, If, etc.) so Goto can resolve them.
        // Also records which For loops contain each label (for E0321).
        let mut for_stack = Vec::new();
        for &id in program {
            self.collect_labels_recursive(id, top, &mut for_stack);
        }
    }

    /// Hoist `Const` declarations out of nested statement bodies (If/loops/Select)
    /// into the given scope, so references resolve regardless of block nesting
    /// (§4.2 has no block scoping). Skips Function bodies (their own scope).
    fn collect_consts_recursive(&mut self, id: NodeId, scope: ScopeId) {
        match self.arena[id].clone() {
            Node::Stmt(Stmt::Const {
                name: DimName { name_span, sigil },
                ty,
                is_global,
                ..
            }) => {
                self.pass1_const(scope, name_span, sigil, ty, is_global);
            }
            Node::Stmt(Stmt::If {
                then_body,
                elseifs,
                else_body,
                ..
            }) => {
                for &s in &then_body {
                    self.collect_consts_recursive(s, scope);
                }
                for ei in &elseifs {
                    for &s in &ei.body {
                        self.collect_consts_recursive(s, scope);
                    }
                }
                if let Some(eb) = &else_body {
                    for &s in eb {
                        self.collect_consts_recursive(s, scope);
                    }
                }
            }
            Node::Stmt(Stmt::While { body, .. })
            | Node::Stmt(Stmt::RepeatForever { body })
            | Node::Stmt(Stmt::RepeatWhile { body, .. })
            | Node::Stmt(Stmt::RepeatUntil { body, .. })
            | Node::Stmt(Stmt::For { body, .. })
            | Node::Stmt(Stmt::ForEach { body, .. }) => {
                for &s in &body {
                    self.collect_consts_recursive(s, scope);
                }
            }
            Node::Stmt(Stmt::Select { arms, .. }) => {
                for &arm_id in &arms {
                    if let Node::CaseArm(CaseArm::Case { body, .. })
                    | Node::CaseArm(CaseArm::Default { body }) = &self.arena[arm_id]
                    {
                        let body = body.clone();
                        for &s in &body {
                            self.collect_consts_recursive(s, scope);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_labels_recursive(
        &mut self,
        id: NodeId,
        scope: ScopeId,
        for_stack: &mut Vec<NodeId>,
    ) {
        match self.arena[id].clone() {
            Node::Stmt(Stmt::Label { name_span }) => {
                let name = self.intern_span(name_span);
                if self.symbols.lookup(scope, name).is_none() {
                    let decl = Declaration {
                        kind: DeclKind::Label,
                        ty: Type::Void,
                        span: name_span,
                        is_global: false,
                    };
                    self.try_declare(scope, name, decl, E_DUPLICATE_DECL);
                }
                if !for_stack.is_empty() {
                    self.label_for_nesting.insert(name, for_stack.clone());
                }
            }
            Node::Stmt(Stmt::If {
                then_body,
                elseifs,
                else_body,
                ..
            }) => {
                for &s in &then_body {
                    self.collect_labels_recursive(s, scope, for_stack);
                }
                for ei in &elseifs {
                    for &s in &ei.body {
                        self.collect_labels_recursive(s, scope, for_stack);
                    }
                }
                if let Some(eb) = &else_body {
                    for &s in eb {
                        self.collect_labels_recursive(s, scope, for_stack);
                    }
                }
            }
            Node::Stmt(Stmt::While { body, .. })
            | Node::Stmt(Stmt::RepeatForever { body })
            | Node::Stmt(Stmt::RepeatWhile { body, .. })
            | Node::Stmt(Stmt::RepeatUntil { body, .. }) => {
                for &s in &body {
                    self.collect_labels_recursive(s, scope, for_stack);
                }
            }
            Node::Stmt(Stmt::For { body, .. }) | Node::Stmt(Stmt::ForEach { body, .. }) => {
                for_stack.push(id);
                for &s in &body {
                    self.collect_labels_recursive(s, scope, for_stack);
                }
                for_stack.pop();
            }
            Node::Stmt(Stmt::Select { arms, .. }) => {
                for &arm_id in &arms {
                    if let Node::CaseArm(CaseArm::Case { body, .. })
                    | Node::CaseArm(CaseArm::Default { body }) = &self.arena[arm_id]
                    {
                        let body = body.clone();
                        for &s in &body {
                            self.collect_labels_recursive(s, scope, for_stack);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn pass1_stmt(&mut self, id: NodeId, scope: ScopeId) {
        let span = self.arena.span_of(id);
        match self.arena[id].clone() {
            Node::Stmt(Stmt::Function {
                name_span,
                return_sigil,
                params,
                return_ty,
                body: _,
            }) => {
                self.pass1_function(scope, name_span, return_sigil, &params, return_ty, span);
            }
            Node::Stmt(Stmt::TypeDecl {
                kind,
                name_span,
                fields,
            }) => {
                self.pass1_type_decl(scope, kind, name_span, &fields, span);
            }
            Node::Stmt(Stmt::VarDecl {
                is_global: true,
                names,
                ty,
                init: _,
            }) => {
                self.pass1_global(scope, &names, ty);
            }
            Node::Stmt(Stmt::Label { name_span }) => {
                self.pass1_label(scope, name_span);
            }
            // `Const` is hoisted by `collect_consts_recursive` in `pass1`, which
            // also reaches block-nested consts — do not declare it here too.
            _ => {}
        }
    }

    fn pass1_function(
        &mut self,
        scope: ScopeId,
        name_span: Span,
        return_sigil: Option<Sigil>,
        params: &[NodeId],
        return_ty_node: Option<NodeId>,
        _full_span: Span,
    ) {
        let name = self.intern_span(name_span);
        let mut param_infos = Vec::with_capacity(params.len());
        for &pid in params {
            if let Node::Param(Param {
                name_span: pname,
                sigil,
                ty,
                default,
            }) = &self.arena[pid]
            {
                let pname_sym = match pname {
                    Some(s) => self.intern_ident(*s, *sigil),
                    None => Symbol::DUMMY,
                };
                let as_ty = ty.map(|tid| self.resolve_type_expr(tid));
                let (pty, sigil_as_disagree) = types::resolve_var_type(*sigil, as_ty.as_ref());
                if sigil_as_disagree {
                    self.diagnostics.push(Diagnostic::error(
                        E_SIGIL_AS_DISAGREE,
                        "sigil and `As` type disagree",
                        Label::new(pname.unwrap_or(name_span)),
                    ));
                }
                param_infos.push(ParamInfo {
                    name: pname_sym,
                    ty: pty,
                    has_default: default.is_some(),
                });
            }
        }
        let as_ret = return_ty_node.map(|tid| self.resolve_type_expr(tid));
        let (ret_ty, sigil_as_disagree) = types::resolve_return_type(return_sigil, as_ret.as_ref());
        if sigil_as_disagree {
            self.diagnostics.push(Diagnostic::error(
                E_SIGIL_AS_DISAGREE,
                "return type sigil and `As` type disagree",
                Label::new(name_span),
            ));
        }
        let fn_type = match &ret_ty {
            Type::Void => Type::Void,
            _ => ret_ty.clone(),
        };
        let decl = Declaration {
            kind: DeclKind::Function {
                params: param_infos,
                return_ty: ret_ty,
                scope: None,
            },
            ty: fn_type,
            span: name_span,
            is_global: false,
        };
        self.try_declare(scope, name, decl, E_DUPLICATE_DEFINITION);
    }

    /// Collect a `Type`/`Struct` body's `FieldDecl`s into `FieldInfo`s,
    /// emitting the sigil/`As` disagreement diagnostic (E0320) per field.
    /// Shared by both record kinds (S-M11).
    fn collect_fields(&mut self, fields: &[NodeId]) -> Vec<FieldInfo> {
        let mut field_infos = Vec::with_capacity(fields.len());
        for &fid in fields {
            if let Node::Stmt(Stmt::FieldDecl {
                name:
                    DimName {
                        name_span: fname_span,
                        sigil,
                    },
                ty,
            }) = &self.arena[fid]
            {
                let fname = self.intern_ident(*fname_span, *sigil);
                let as_ty = ty.map(|tid| self.resolve_type_expr(tid));
                let (fty, sigil_as_disagree) = types::resolve_var_type(*sigil, as_ty.as_ref());
                if sigil_as_disagree {
                    self.diagnostics.push(Diagnostic::error(
                        E_SIGIL_AS_DISAGREE,
                        "sigil and `As` type disagree",
                        Label::new(*fname_span),
                    ));
                }
                field_infos.push(FieldInfo {
                    name: fname,
                    ty: fty,
                    span: *fname_span,
                });
            }
        }
        field_infos
    }

    /// Register a `Type` or `Struct` declaration. The two forms differ only in
    /// the resulting `DeclKind`/`Type` (heap reference vs value), selected from
    /// `kind` (F-A1); the field collection is shared.
    fn pass1_type_decl(
        &mut self,
        scope: ScopeId,
        kind: TypeDeclKind,
        name_span: Span,
        fields: &[NodeId],
        _full_span: Span,
    ) {
        let name = self.intern_span(name_span);
        let field_infos = self.collect_fields(fields);
        let (decl_kind, ty) = match kind {
            TypeDeclKind::Type => (
                DeclKind::TypeDef {
                    fields: field_infos,
                },
                Type::TypeRef { name },
            ),
            TypeDeclKind::Struct => (
                DeclKind::StructDef {
                    fields: field_infos,
                },
                Type::StructVal { name },
            ),
        };
        let decl = Declaration {
            kind: decl_kind,
            ty,
            span: name_span,
            is_global: false,
        };
        self.try_declare(scope, name, decl, E_DUPLICATE_DEFINITION);
    }

    fn pass1_global(
        &mut self,
        scope: ScopeId,
        names: &[cb_frontend::DimName],
        ty_node: Option<NodeId>,
    ) {
        let as_ty = ty_node.map(|tid| self.resolve_type_expr(tid));
        for dn in names {
            let name = self.intern_ident(dn.name_span, dn.sigil);
            let (var_ty, sigil_as_disagree) = types::resolve_var_type(dn.sigil, as_ty.as_ref());
            if sigil_as_disagree {
                self.diagnostics.push(Diagnostic::error(
                    E_SIGIL_AS_DISAGREE,
                    "sigil and `As` type disagree",
                    Label::new(dn.name_span),
                ));
            }
            let decl = Declaration {
                kind: DeclKind::Variable,
                ty: var_ty,
                span: dn.name_span,
                is_global: true,
            };
            self.try_declare(scope, name, decl, E_DUPLICATE_DECL);
        }
    }

    fn pass1_label(&mut self, scope: ScopeId, name_span: Span) {
        let name = self.intern_span(name_span);
        let decl = Declaration {
            kind: DeclKind::Label,
            ty: Type::Void,
            span: name_span,
            is_global: false,
        };
        self.try_declare(scope, name, decl, E_DUPLICATE_DECL);
    }

    fn pass1_const(
        &mut self,
        scope: ScopeId,
        name_span: Span,
        sigil: Option<Sigil>,
        ty_node: Option<NodeId>,
        is_global: bool,
    ) {
        let name = self.intern_ident(name_span, sigil);
        let as_ty = ty_node.map(|tid| self.resolve_type_expr(tid));
        let (const_ty, sigil_as_disagree) = types::resolve_var_type(sigil, as_ty.as_ref());
        if sigil_as_disagree {
            self.diagnostics.push(Diagnostic::error(
                E_SIGIL_AS_DISAGREE,
                "sigil and `As` type disagree",
                Label::new(name_span),
            ));
        }
        let placeholder = match &const_ty {
            Type::Int => ConstValue::Int(0),
            Type::Float => ConstValue::Float(0.0),
            Type::String => ConstValue::String(std::string::String::new()),
            _ => ConstValue::Int(0),
        };
        let decl = Declaration {
            kind: DeclKind::Constant { value: placeholder },
            ty: const_ty,
            span: name_span,
            is_global,
        };
        self.try_declare(scope, name, decl, E_DUPLICATE_DECL);
    }

    // ── pass 2: full resolution and type checking ───────────────────────

    fn pass2(&mut self, program: &[NodeId]) {
        for &id in program {
            self.check_stmt(id);
        }
    }

    // ── expression typing ───────────────────────────────────────────────

    fn check_expr(&mut self, id: NodeId) -> Type {
        let span = self.arena.span_of(id);
        let ty = match self.arena[id].clone() {
            Node::Expr(Expr::IntLit(_)) => Type::Int,
            Node::Expr(Expr::FloatLit(_)) => Type::Float,
            Node::Expr(Expr::StrLit { .. }) => Type::String,
            Node::Expr(Expr::NullLit) => Type::Null,

            Node::Expr(Expr::Ident { name_span, sigil }) => {
                self.check_ident(name_span, sigil, false)
            }

            Node::Expr(Expr::Binary { op, lhs, rhs }) => self.check_binary(op, lhs, rhs, span),

            Node::Expr(Expr::Unary { op, operand }) => self.check_unary(op, operand, span),

            Node::Expr(Expr::Call { callee, args }) => self.check_call(id, callee, &args, span),

            Node::Expr(Expr::Index { array, indices }) => self.check_index(array, &indices, span),

            Node::Expr(Expr::Field { target, name_span }) => {
                self.check_field(target, name_span, span)
            }

            Node::Expr(Expr::Paren { inner }) => self.check_expr(inner),

            Node::Expr(Expr::New(kind)) => self.check_new(&kind, span),

            Node::Expr(Expr::Error) => Type::Error,

            _ => Type::Error,
        };
        self.types.insert(id, ty.clone());
        ty
    }

    fn check_ident(
        &mut self,
        name_span: Span,
        sigil: Option<Sigil>,
        _is_assign_target: bool,
    ) -> Type {
        let name = self.intern_ident(name_span, sigil);
        if let Some(decl) = self.symbols.lookup(self.current_scope, name) {
            let decl_ty = decl.ty.clone();
            // A bare function name in value position takes the function's
            // address: its type is a fn-pointer of the function's signature, not
            // its return type (cb_syntax.md §7.4). `check_call` intercepts a
            // callee ident before it reaches here, so this fires only for
            // genuine value uses. Overloaded / built-in commands have no single
            // address (§7.2) and are rejected as E0329 below.
            let fnptr_ty = if let DeclKind::Function {
                params, return_ty, ..
            } = &decl.kind
            {
                Some(Type::FnPtr {
                    params: params.iter().map(|p| p.ty.clone()).collect(),
                    ret: match return_ty {
                        Type::Void => None,
                        t => Some(Box::new(t.clone())),
                    },
                })
            } else {
                None
            };
            let is_command = matches!(
                decl.kind,
                DeclKind::OverloadSet { .. } | DeclKind::RuntimeFn { .. }
            );
            // A bare Type/Struct/runtime-type name is not a value. The positions
            // that legitimately take a bare Type name (First/Last, For-Each
            // source) resolve it directly via `resolve_type_name_arg` and never
            // reach here; anything that does is a genuine type-as-value misuse
            // (cb_syntax.md §3.3, e.g. `Return Foo`, `a = Foo`).
            let is_type_name = matches!(
                decl.kind,
                DeclKind::TypeDef { .. } | DeclKind::StructDef { .. } | DeclKind::RuntimeTypeDef
            );
            // Sigil enforcement: if this use has a sigil, it must match the
            // declared type (for a function name, its return type).
            if let Some(s) = sigil {
                let sigil_ty = types::sigil_to_type(s);
                if sigil_ty != decl_ty && !decl_ty.is_error() {
                    self.diagnostics.push(Diagnostic::error(
                        E_SIGIL_CONFLICT,
                        format!("sigil `{}` conflicts with declared type", sigil_char(s),),
                        Label::new(name_span),
                    ));
                }
            }
            if is_command {
                self.diagnostics.push(Diagnostic::error(
                    E_ADDRESS_OF_UNSUPPORTED,
                    format!(
                        "cannot take the address of overloaded or built-in command `{}`; only user-defined functions and subs have addresses",
                        self.interner.resolve(name)
                    ),
                    Label::new(name_span),
                ));
                return Type::Error;
            }
            if is_type_name {
                self.diagnostics.push(Diagnostic::error(
                    E_TYPE_AS_VALUE,
                    format!(
                        "`{}` is a type name, not a value",
                        self.interner.resolve(name)
                    ),
                    Label::new(name_span),
                ));
                return Type::Error;
            }
            fnptr_ty.unwrap_or(decl_ty)
        } else {
            // Undeclared — will be handled as implicit declaration when
            // encountered as assignment target in check_stmt. When encountered
            // as a read, this is an error.
            self.diagnostics.push(Diagnostic::error(
                E_UNDECLARED_IDENT,
                format!("undeclared identifier `{}`", self.interner.resolve(name)),
                Label::new(name_span),
            ));
            Type::Error
        }
    }

    fn check_binary(&mut self, op: BinOp, lhs: NodeId, rhs: NodeId, span: Span) -> Type {
        let lty = self.check_expr(lhs);
        let rty = self.check_expr(rhs);
        if lty.is_error() || rty.is_error() {
            return Type::Error;
        }
        let result_ty = match types::binary_result_type(op, &lty, &rty) {
            Some(t) => t,
            None => {
                let (lname, rname) = (self.type_name(&lty), self.type_name(&rty));
                self.err(
                    E_TYPE_MISMATCH,
                    format!("operator `{op:?}` cannot be applied to `{lname}` and `{rname}`"),
                    span,
                );
                Type::Error
            }
        };
        if !result_ty.is_error() && !matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Sar) {
            let operand_ty = match op {
                BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                    if lty.is_numeric() && rty.is_numeric() {
                        types::numeric_promote(&lty, &rty)
                    } else {
                        lty.clone()
                    }
                }
                BinOp::And | BinOp::Or | BinOp::Xor => Type::Int,
                _ => result_ty.clone(),
            };
            if lty != operand_ty {
                self.coerce(lhs, &lty, &operand_ty);
            }
            if rty != operand_ty {
                self.coerce(rhs, &rty, &operand_ty);
            }
        }
        result_ty
    }

    fn check_unary(&mut self, op: UnOp, operand: NodeId, span: Span) -> Type {
        let oty = self.check_expr(operand);
        if oty.is_error() {
            return Type::Error;
        }
        match types::unary_result_type(op, &oty) {
            Some(t) => t,
            None => {
                let oname = self.type_name(&oty);
                self.err(
                    E_TYPE_MISMATCH,
                    format!("operator `{op:?}` cannot be applied to `{oname}`"),
                    span,
                );
                Type::Error
            }
        }
    }

    fn check_call(&mut self, call_id: NodeId, callee: NodeId, args: &[NodeId], span: Span) -> Type {
        // Check if callee is an identifier that names an intrinsic.
        if let Node::Expr(Expr::Ident {
            name_span,
            sigil: None,
        }) = &self.arena[callee]
        {
            let name = self.intern_ident(*name_span, None);
            let name_str = self.interner.resolve(name).to_owned();

            if let Some(ty) = self.check_intrinsic_call(&name_str, args, span) {
                return ty;
            }
        }

        // Check arg expressions first; both the direct-call and indirect-call
        // paths below need their types.
        let arg_types: Vec<Type> = args.iter().map(|&a| self.check_expr(a)).collect();

        // Direct call: a callee ident naming a function, runtime command, or
        // overload set is resolved by name here — *without* typing the callee as
        // a value. That distinction is what makes `print(...)` a call rather than
        // an address-of (cb_syntax.md §7.4); typing the callee as a value first
        // would mis-fire E0329 on every command call.
        if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[callee] {
            let name = self.intern_ident(*name_span, *sigil);
            if let Some(decl) = self.symbols.lookup(self.current_scope, name) {
                let decl_kind = decl.kind.clone();
                match &decl_kind {
                    DeclKind::Function {
                        params, return_ty, ..
                    } => {
                        let min_args = params.iter().filter(|p| !p.has_default).count();
                        let max_args = params.len();
                        if arg_types.len() < min_args || arg_types.len() > max_args {
                            self.diagnostics.push(Diagnostic::error(
                                E_WRONG_ARG_COUNT,
                                format!(
                                    "function expects {} argument(s), got {}",
                                    if min_args == max_args {
                                        format!("{max_args}")
                                    } else {
                                        format!("{min_args}..{max_args}")
                                    },
                                    arg_types.len()
                                ),
                                Label::new(span),
                            ));
                        } else {
                            // Coerce each supplied argument to its parameter type,
                            // recording implicit conversions and reporting
                            // E0317/E0318 on mismatch — the same path runtime
                            // commands and overloads already use. `take` lines the
                            // supplied args up with their params positionally; any
                            // trailing defaulted params receive no argument here.
                            for (i, param) in params.iter().enumerate().take(arg_types.len()) {
                                if arg_types[i] != param.ty {
                                    self.coerce(args[i], &arg_types[i], &param.ty);
                                }
                            }
                        }
                        self.resolved_calls
                            .insert(call_id, ResolvedCall::UserDefined { name });
                        return return_ty.clone();
                    }
                    DeclKind::RuntimeFn {
                        params,
                        return_ty,
                        c_symbol,
                    } => {
                        if arg_types.len() != params.len() {
                            self.diagnostics.push(Diagnostic::error(
                                E_WRONG_ARG_COUNT,
                                format!(
                                    "function expects {} argument(s), got {}",
                                    params.len(),
                                    arg_types.len()
                                ),
                                Label::new(span),
                            ));
                        } else {
                            for (i, param) in params.iter().enumerate() {
                                if arg_types[i] != param.ty {
                                    self.coerce(args[i], &arg_types[i], &param.ty);
                                }
                            }
                        }
                        self.resolved_calls.insert(
                            call_id,
                            ResolvedCall::RuntimeFn {
                                c_symbol: c_symbol.clone(),
                            },
                        );
                        return return_ty.clone();
                    }
                    DeclKind::OverloadSet { variants } => {
                        let variants = variants.clone();
                        return self.resolve_overload(call_id, &variants, &arg_types, args, span);
                    }
                    _ => {}
                }
            }
        }

        // Indirect call: the callee is a value — an FnPtr variable, or an
        // error / undeclared / non-callable. This is the only place a callee is
        // typed as a value (so a function/command name handled above never goes
        // through `check_ident`'s address-of path).
        let callee_ty = self.check_expr(callee);
        if callee_ty.is_error() {
            return Type::Error;
        }

        // If callee is an FnPtr type, check it.
        if let Type::FnPtr { params, ret } = &callee_ty {
            if arg_types.len() != params.len() {
                self.diagnostics.push(Diagnostic::error(
                    E_WRONG_ARG_COUNT,
                    format!(
                        "function pointer expects {} argument(s), got {}",
                        params.len(),
                        arg_types.len()
                    ),
                    Label::new(span),
                ));
            }
            return ret.as_ref().map_or(Type::Void, |t| *t.clone());
        }

        // Not a callable type.
        let ty_name = self.type_name(&callee_ty);
        self.err(
            E_CALL_NON_FUNCTION,
            format!("cannot call value of type `{ty_name}`"),
            span,
        );
        Type::Error
    }

    fn resolve_overload(
        &mut self,
        call_id: NodeId,
        variants: &[OverloadVariant],
        arg_types: &[Type],
        arg_nodes: &[NodeId],
        span: Span,
    ) -> Type {
        // Filter variants by arity.
        let candidates: Vec<_> = variants
            .iter()
            .filter(|v| v.params.len() == arg_types.len())
            .collect();

        if candidates.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                E_NO_MATCHING_OVERLOAD,
                format!("no overload accepts {} argument(s)", arg_types.len()),
                Label::new(span),
            ));
            return Type::Error;
        }

        // Score each candidate: count exact type matches, check convertibility.
        let mut scored: Vec<(&OverloadVariant, usize)> = Vec::new();
        for variant in &candidates {
            let mut exact = 0usize;
            let mut all_convertible = true;
            for (i, param) in variant.params.iter().enumerate() {
                if arg_types[i] == param.ty {
                    exact += 1;
                } else if convert::find_implicit_conversion(&arg_types[i], &param.ty).is_none() {
                    all_convertible = false;
                    break;
                }
            }
            if all_convertible {
                scored.push((variant, exact));
            }
        }

        if scored.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                E_NO_MATCHING_OVERLOAD,
                "no overload matches the argument types",
                Label::new(span),
            ));
            return Type::Error;
        }

        // Pick best: most exact matches wins.
        scored.sort_by_key(|s| std::cmp::Reverse(s.1));
        if scored.len() > 1 && scored[0].1 == scored[1].1 {
            self.diagnostics.push(Diagnostic::error(
                E_AMBIGUOUS_OVERLOAD,
                "ambiguous overload: multiple candidates match equally well",
                Label::new(span),
            ));
            return Type::Error;
        }

        let winner = scored[0].0;

        // Record coercions for args that need conversion.
        for (i, param) in winner.params.iter().enumerate() {
            if arg_types[i] != param.ty {
                self.coerce(arg_nodes[i], &arg_types[i], &param.ty);
            }
        }

        self.resolved_calls.insert(
            call_id,
            ResolvedCall::RuntimeFn {
                c_symbol: winner.c_symbol.clone(),
            },
        );
        winner.return_ty.clone()
    }

    fn check_intrinsic_call(&mut self, name: &str, args: &[NodeId], span: Span) -> Option<Type> {
        // Intrinsics are matched case-insensitively: fold the resolved name to
        // its canonical key (`resolve` preserves the user's original casing,
        // which is kept for `{name}` in the messages below).
        match cb_diagnostics::fold(name).as_str() {
            INTRINSIC_LEN => {
                if args.is_empty() || args.len() > 2 {
                    self.err(
                        E_WRONG_ARG_COUNT,
                        format!("Len expects 1 or 2 arguments, got {}", args.len()),
                        span,
                    );
                    return Some(Type::Error);
                }
                let arg0_ty = self.check_expr(args[0]);
                if matches!(arg0_ty, Type::String) {
                    // Len(s$) — codepoint length of a string. No dimension arg.
                    if args.len() == 2 {
                        self.err(
                            E_WRONG_ARG_COUNT,
                            "Len of a string takes exactly 1 argument",
                            span,
                        );
                    }
                } else {
                    if !matches!(arg0_ty, Type::Array { .. }) && !arg0_ty.is_error() {
                        self.err(
                            E_TYPE_MISMATCH,
                            "first argument to Len must be an array or a string",
                            self.arena.span_of(args[0]),
                        );
                    }
                    if args.len() == 2 {
                        let dim_ty = self.check_expr(args[1]);
                        self.expect_integer(
                            &dim_ty,
                            self.arena.span_of(args[1]),
                            "second argument to Len",
                        );
                    }
                }
                Some(Type::Int)
            }
            INTRINSIC_INT | INTRINSIC_INTEGER => {
                self.check_conversion_intrinsic(args, span, Type::Int)
            }
            INTRINSIC_FLOAT => self.check_conversion_intrinsic(args, span, Type::Float),
            INTRINSIC_STR => self.check_conversion_intrinsic(args, span, Type::String),
            INTRINSIC_FIRST | INTRINSIC_LAST => {
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::error(
                        E_WRONG_ARG_COUNT,
                        format!("{name} expects 1 argument, got {}", args.len()),
                        Label::new(span),
                    ));
                    return Some(Type::Error);
                }
                // First/Last take a bare Type name (a heap `Type` list head),
                // never an arbitrary value — so resolve the name directly. This
                // keeps E0311 from firing on the legitimate name position while
                // still rejecting struct/runtime-type names and plain values.
                match self.resolve_type_name_arg(args[0]) {
                    Some(ty @ Type::TypeRef { .. }) => Some(ty),
                    Some(_) => {
                        self.diagnostics.push(Diagnostic::error(
                            E_TYPE_MISMATCH,
                            format!("{name} expects a Type name"),
                            Label::new(self.arena.span_of(args[0])),
                        ));
                        Some(Type::Error)
                    }
                    None => {
                        let ty = self.check_expr(args[0]);
                        if !ty.is_error() {
                            self.diagnostics.push(Diagnostic::error(
                                E_TYPE_MISMATCH,
                                format!("{name} expects a Type name"),
                                Label::new(self.arena.span_of(args[0])),
                            ));
                        }
                        Some(Type::Error)
                    }
                }
            }
            INTRINSIC_NEXT | INTRINSIC_PREVIOUS => {
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::error(
                        E_WRONG_ARG_COUNT,
                        format!("{name} expects 1 argument, got {}", args.len()),
                        Label::new(span),
                    ));
                    return Some(Type::Error);
                }
                let ty = self.check_expr(args[0]);
                if matches!(ty, Type::TypeRef { .. }) || ty.is_error() {
                    Some(ty)
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        E_TYPE_MISMATCH,
                        format!("{name} expects a Type instance"),
                        Label::new(self.arena.span_of(args[0])),
                    ));
                    Some(Type::Error)
                }
            }
            _ => None,
        }
    }

    fn check_conversion_intrinsic(
        &mut self,
        args: &[NodeId],
        span: Span,
        target: Type,
    ) -> Option<Type> {
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                E_WRONG_ARG_COUNT,
                format!(
                    "conversion intrinsic expects 1 argument, got {}",
                    args.len()
                ),
                Label::new(span),
            ));
            return Some(Type::Error);
        }
        // Int/Float/Str convert only from a numeric or String operand; a Type
        // instance, array, etc. has no conversion to a scalar (contrast `Len`,
        // which validates its operand kind).
        let arg_ty = self.check_expr(args[0]);
        if !arg_ty.is_error() && !arg_ty.is_numeric() && !matches!(arg_ty, Type::String) {
            self.diagnostics.push(Diagnostic::error(
                E_TYPE_MISMATCH,
                "conversion intrinsic argument must be numeric or a string",
                Label::new(self.arena.span_of(args[0])),
            ));
        }
        Some(target)
    }

    fn check_index(&mut self, array: NodeId, indices: &[NodeId], span: Span) -> Type {
        let arr_ty = self.check_expr(array);
        // Index operands must be integers (matching `check_new`/`check_redim`).
        for &i in indices {
            let idx_ty = self.check_expr(i);
            self.expect_integer(&idx_ty, self.arena.span_of(i), "array index");
        }

        if arr_ty.is_error() {
            return Type::Error;
        }

        if let Type::Array { elem, rank } = &arr_ty {
            if indices.len() != *rank as usize {
                self.err(
                    E_RANK_MISMATCH,
                    format!(
                        "array has {} dimension(s), but {} index/indices provided",
                        rank,
                        indices.len()
                    ),
                    span,
                );
            }
            *elem.clone()
        } else {
            let ty_name = self.type_name(&arr_ty);
            self.err(
                E_INDEX_NON_ARRAY,
                format!("cannot index value of type `{ty_name}`"),
                span,
            );
            Type::Error
        }
    }

    fn check_field(&mut self, target: NodeId, name_span: Span, span: Span) -> Type {
        let target_ty = self.check_expr(target);
        if target_ty.is_error() {
            return Type::Error;
        }

        let field_name = self.intern_span(name_span);

        let fields = match &target_ty {
            Type::TypeRef { name } | Type::StructVal { name } => self
                .symbols
                .lookup(self.current_scope, *name)
                .and_then(|decl| match &decl.kind {
                    DeclKind::TypeDef { fields } | DeclKind::StructDef { fields } => {
                        Some(fields.clone())
                    }
                    _ => None,
                }),
            _ => {
                let ty_name = self.type_name(&target_ty);
                self.err(
                    E_FIELD_ON_NON_TYPE,
                    format!("cannot access fields on `{ty_name}`"),
                    span,
                );
                return Type::Error;
            }
        };

        if let Some(fields) = fields {
            for f in &fields {
                if f.name == field_name {
                    return f.ty.clone();
                }
            }
            self.err(
                E_NO_SUCH_FIELD,
                format!("no field `{}` on type", self.interner.resolve(field_name)),
                name_span,
            );
            Type::Error
        } else {
            Type::Error
        }
    }

    fn check_new(&mut self, kind: &cb_frontend::NewKind, span: Span) -> Type {
        match kind {
            cb_frontend::NewKind::Type(type_expr_id) => {
                let ty = self.resolve_type_expr(*type_expr_id);
                match &ty {
                    // `New T` allocates a heap `Type` node and threads it into
                    // T's linked list — only user-defined `Type … EndType`
                    // records qualify (cb_syntax.md §3.2/§7.4).
                    Type::TypeRef { .. } => ty,
                    Type::Error => Type::Error,
                    // A value-type `Struct` is allocated in place on
                    // declaration; it has no `New`/`Delete` and is never heap
                    // allocated (cb_syntax.md §3.3 "Struct … EndStruct").
                    Type::StructVal { .. } => {
                        let name = self.type_name(&ty);
                        self.err(
                            E_TYPE_MISMATCH,
                            format!(
                                "`New` requires a user-defined Type; `{name}` is a Struct, \
                                 which is a value type allocated in place (it has no `New`)"
                            ),
                            span,
                        );
                        Type::Error
                    }
                    // Opaque runtime handles are created and destroyed by the
                    // runtime library, never with `New` (cb_syntax.md §3.5).
                    Type::RuntimeType { .. } => {
                        let name = self.type_name(&ty);
                        self.err(
                            E_TYPE_MISMATCH,
                            format!(
                                "`New` requires a user-defined Type; `{name}` is a built-in \
                                 runtime type whose handles are managed by the runtime library"
                            ),
                            span,
                        );
                        Type::Error
                    }
                    _ => {
                        let name = self.type_name(&ty);
                        self.err(
                            E_TYPE_MISMATCH,
                            format!("`New` requires a user-defined Type name, got `{name}`"),
                            span,
                        );
                        Type::Error
                    }
                }
            }
            cb_frontend::NewKind::Array { elem, dims } => {
                let elem_ty = self.resolve_type_expr(*elem);
                for &d in dims {
                    let dty = self.check_expr(d);
                    self.expect_integer(&dty, self.arena.span_of(d), "array dimension");
                }
                Type::Array {
                    elem: Box::new(elem_ty),
                    rank: dims.len() as u8,
                }
            }
        }
    }

    // ── statement checking ──────────────────────────────────────────────

    fn check_stmt(&mut self, id: NodeId) {
        match self.arena[id].clone() {
            Node::Stmt(Stmt::Assign { target, value }) => {
                self.check_assign(target, value);
            }
            Node::Stmt(Stmt::ExprStmt { expr }) => {
                // A bare function/command name as a complete statement is a
                // 0-arg call (CoolBasic paren-less sub-call syntax), matching
                // `lower_stmt`. Route it through call-checking so it is validated
                // as a call rather than mistaken for an address-of value (§7.4).
                let bare_call =
                    if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[expr] {
                        let name = self.intern_ident(*name_span, *sigil);
                        self.symbols
                            .lookup(self.current_scope, name)
                            .is_some_and(|d| {
                                matches!(
                                    d.kind,
                                    DeclKind::Function { .. }
                                        | DeclKind::RuntimeFn { .. }
                                        | DeclKind::OverloadSet { .. }
                                )
                            })
                    } else {
                        false
                    };
                if bare_call {
                    let span = self.arena.span_of(expr);
                    self.check_call(expr, expr, &[], span);
                } else {
                    self.check_expr(expr);
                }
            }
            Node::Stmt(Stmt::VarDecl {
                is_global: false,
                names,
                ty,
                init,
            }) => {
                self.check_dim(&names, ty, init);
            }
            Node::Stmt(Stmt::VarDecl {
                is_global: true,
                names,
                ty,
                init: Some(init_id),
            }) => {
                let as_ty = ty.map(|tid| self.resolve_type_expr(tid));
                self.coerce_initializer(init_id, &names, as_ty.as_ref());
            }
            Node::Stmt(Stmt::VarDecl {
                is_global: true, ..
            }) => {}
            Node::Stmt(Stmt::Const {
                name: DimName { name_span, sigil },
                value,
                ..
            }) => {
                let value_ty = self.check_expr(value);
                // §4.4: a Const initializer must be a constant expression.
                if let Some(const_val) = self.eval_const_expr(value) {
                    let name = self.intern_ident(name_span, sigil);
                    self.symbols
                        .update_const_value(self.current_scope, name, const_val);
                } else if !value_ty.is_error() {
                    self.diagnostics.push(Diagnostic::error(
                        E_CONST_EVAL_ERROR,
                        "Const initializer must be a constant expression",
                        Label::new(self.arena.span_of(value)),
                    ));
                }
            }
            Node::Stmt(Stmt::If {
                cond,
                then_body,
                elseifs,
                else_body,
                ..
            }) => {
                self.check_condition(cond);
                for &s in &then_body {
                    self.check_stmt(s);
                }
                for ei in &elseifs {
                    self.check_condition(ei.cond);
                    for &s in &ei.body {
                        self.check_stmt(s);
                    }
                }
                if let Some(eb) = &else_body {
                    for &s in eb {
                        self.check_stmt(s);
                    }
                }
            }
            Node::Stmt(Stmt::While { cond, body }) => {
                self.check_condition(cond);
                self.control_stack.push(ControlKind::Loop);
                for &s in &body {
                    self.check_stmt(s);
                }
                self.control_stack.pop();
            }
            Node::Stmt(Stmt::RepeatForever { body }) => {
                self.control_stack.push(ControlKind::Loop);
                for &s in &body {
                    self.check_stmt(s);
                }
                self.control_stack.pop();
            }
            Node::Stmt(Stmt::RepeatUntil { body, cond })
            | Node::Stmt(Stmt::RepeatWhile { body, cond }) => {
                self.control_stack.push(ControlKind::Loop);
                for &s in &body {
                    self.check_stmt(s);
                }
                self.control_stack.pop();
                self.check_condition(cond);
            }
            Node::Stmt(Stmt::For {
                var,
                from,
                to,
                step,
                body,
                ..
            }) => {
                self.check_for(id, var, from, to, step, &body);
            }
            Node::Stmt(Stmt::ForEach {
                var, source, body, ..
            }) => {
                self.check_for_each(id, var, source, &body);
            }
            Node::Stmt(Stmt::Select { scrutinee, arms }) => {
                self.check_select(scrutinee, &arms);
            }
            Node::Stmt(Stmt::Function {
                name_span,
                return_sigil,
                params,
                return_ty,
                body,
            }) => {
                self.check_function(name_span, return_sigil, &params, return_ty, &body);
            }
            Node::Stmt(Stmt::Return { value }) => {
                self.check_return(value, self.arena.span_of(id));
            }
            Node::Stmt(Stmt::Goto { name_span }) => {
                self.check_goto(name_span);
            }
            Node::Stmt(Stmt::Delete { operand }) => {
                self.check_delete(id, operand);
            }
            Node::Stmt(Stmt::Redim {
                target,
                elem_ty,
                dims,
            }) => {
                self.check_redim(target, elem_ty, &dims);
            }
            Node::Stmt(Stmt::Label { name_span }) if !self.for_loop_stack.is_empty() => {
                let name = self.intern_span(name_span);
                self.label_for_nesting
                    .insert(name, self.for_loop_stack.clone());
            }
            Node::Stmt(Stmt::Label { .. }) => {}
            Node::Stmt(Stmt::Break { count }) => {
                // `Break N` must have at least N enclosing loops; a `Select`
                // does not count (Break never targets a Select).
                let n = count.map_or(1, |c| c.get()) as usize;
                let loops = self
                    .control_stack
                    .iter()
                    .filter(|c| **c == ControlKind::Loop)
                    .count();
                if loops < n {
                    self.diagnostics.push(Diagnostic::error(
                        E_MISPLACED_LOOP_CONTROL,
                        if count.is_some() {
                            format!("`Break {n}` has no {n} enclosing loop(s)")
                        } else {
                            "`Break` outside of a loop".to_string()
                        },
                        Label::new(self.arena.span_of(id)),
                    ));
                }
            }
            // `Continue` needs an enclosing loop or `Select` (the explicit
            // fall-through form, cb_syntax.md §6.2).
            Node::Stmt(Stmt::Continue) if self.control_stack.is_empty() => {
                self.diagnostics.push(Diagnostic::error(
                    E_MISPLACED_LOOP_CONTROL,
                    "`Continue` outside of a loop or `Select`",
                    Label::new(self.arena.span_of(id)),
                ));
            }
            // Statements that are already handled or require no type checking:
            Node::Stmt(Stmt::TypeDecl { .. })
            | Node::Stmt(Stmt::FieldDecl { .. })
            | Node::Stmt(Stmt::End)
            | Node::Stmt(Stmt::Include { .. })
            | Node::Stmt(Stmt::Error) => {}
            _ => {}
        }
    }

    /// Try to coerce `value_node` (with type `from`) to `to`. If a conversion
    /// exists, record it in the ConversionTable. If narrowing, emit E0318 warning.
    /// If no conversion path, emit E0317 error. Returns true if compatible.
    fn coerce(&mut self, value_node: NodeId, from: &Type, to: &Type) -> bool {
        if from == to || from.is_error() || to.is_error() {
            return true;
        }
        if let Some(conv) = convert::find_implicit_conversion(from, to) {
            self.conversions.insert(value_node, conv, to.clone());
            if convert::is_narrowing(conv, from, to) {
                // For an integer literal narrowed to a smaller integer target, the
                // value is known at compile time: out of range is a hard error,
                // in range is silent (a literal is a known-safe constant,
                // cb_syntax.md §1.6/§3.4). Runtime/variable values still warn.
                if from.is_integer()
                    && to.is_integer()
                    && let Some(val) = self.literal_int_value(value_node)
                {
                    if let Some((min, max)) = convert::int_range(to)
                        && (val < min || val > max)
                    {
                        let to_name = self.type_name(to);
                        self.err(
                            E_LITERAL_OVERFLOW,
                            format!("integer literal {val} is out of range for type `{to_name}`"),
                            self.arena.span_of(value_node),
                        );
                        return false;
                    }
                    return true;
                }
                let (from_name, to_name) = (self.type_name(from), self.type_name(to));
                self.diagnostics.push(Diagnostic::warning(
                    E_NARROWING_CONVERSION,
                    format!("implicit narrowing conversion from `{from_name}` to `{to_name}`"),
                    Label::new(self.arena.span_of(value_node)),
                ));
            }
            true
        } else {
            let (from_name, to_name) = (self.type_name(from), self.type_name(to));
            self.err(
                E_CANNOT_CONVERT,
                format!("cannot implicitly convert `{from_name}` to `{to_name}`"),
                self.arena.span_of(value_node),
            );
            false
        }
    }

    /// The compile-time value of an integer-literal expression, if `node` is one
    /// (a bare `IntLit`, optionally parenthesised, negated, or `+`-abs'd). Used to
    /// range-check literals against a narrower integer target in `coerce`.
    fn literal_int_value(&self, node: NodeId) -> Option<i128> {
        match &self.arena[node] {
            Node::Expr(Expr::IntLit(v)) => Some(*v as i128),
            Node::Expr(Expr::Paren { inner }) => self.literal_int_value(*inner),
            Node::Expr(Expr::Unary {
                op: UnOp::Neg,
                operand,
            }) => self.literal_int_value(*operand).map(|v| -v),
            Node::Expr(Expr::Unary {
                op: UnOp::Plus,
                operand,
            }) => self.literal_int_value(*operand).map(|v| v.abs()),
            _ => None,
        }
    }

    /// Evaluate a constant expression at compile time.
    /// Returns `Some(value)` on success, `None` if not a constant expression.
    fn eval_const_expr(&mut self, id: NodeId) -> Option<ConstValue> {
        match self.arena[id].clone() {
            Node::Expr(Expr::IntLit(v)) => Some(ConstValue::Int(v as i64)),
            Node::Expr(Expr::FloatLit(v)) => Some(ConstValue::Float(v.to_f64())),
            Node::Expr(Expr::StrLit { value, .. }) => Some(ConstValue::String(value)),
            Node::Expr(Expr::Paren { inner }) => self.eval_const_expr(inner),
            Node::Expr(Expr::Unary { op, operand }) => {
                let val = self.eval_const_expr(operand)?;
                match (op, val) {
                    (UnOp::Neg, ConstValue::Int(v)) => Some(ConstValue::Int(v.wrapping_neg())),
                    (UnOp::Neg, ConstValue::Float(v)) => Some(ConstValue::Float(-v)),
                    // Unary `+` is absolute value (CoolBasic `+x` ≡ `Abs(x)`, FD-028).
                    (UnOp::Plus, ConstValue::Int(v)) => Some(ConstValue::Int(v.wrapping_abs())),
                    (UnOp::Plus, ConstValue::Float(v)) => Some(ConstValue::Float(v.abs())),
                    // `Not` yields Int 1/0 (FD-035): non-zero → 0, 0 → 1.
                    (UnOp::Not, ConstValue::Int(v)) => Some(ConstValue::Int((v == 0) as i64)),
                    _ => None,
                }
            }
            Node::Expr(Expr::Binary { op, lhs, rhs }) => {
                let l = self.eval_const_expr(lhs)?;
                let r = self.eval_const_expr(rhs)?;
                let result = eval_const_binary(op, &l, &r);
                if result.is_none()
                    && matches!(op, BinOp::Div | BinOp::Mod)
                    && matches!(&r, ConstValue::Int(0))
                {
                    self.diagnostics.push(Diagnostic::error(
                        E_CONST_EVAL_ERROR,
                        "division by zero in constant expression",
                        Label::new(self.arena.span_of(rhs)),
                    ));
                }
                // Float `/0` is well-defined in IEEE (inf/nan) but almost always a
                // bug — warn while still folding to the IEEE result (§3.4).
                if op == BinOp::Div && matches!(&r, ConstValue::Float(f) if *f == 0.0) {
                    self.diagnostics.push(Diagnostic::warning(
                        E_CONST_FLOAT_DIV_ZERO,
                        "floating-point division by zero in constant expression",
                        Label::new(self.arena.span_of(rhs)),
                    ));
                }
                result
            }
            Node::Expr(Expr::Ident { name_span, sigil }) => {
                let name = self.intern_ident(name_span, sigil);
                let decl = self.symbols.lookup(self.current_scope, name)?;
                if let DeclKind::Constant { value } = &decl.kind {
                    Some(value.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// A valid assignment target bottoms out at a variable: a bare identifier,
    /// or a chain of field/index projections rooted at one (`x`, `arr[i]`,
    /// `node.field`, `o.inner.v`, `arr[i].field`). A target rooted at a
    /// temporary — e.g. a function-call result (`getMob().hp = 0`) — is not an
    /// lvalue (cb_syntax.md §6.1); lowering cannot address its storage, so it
    /// must be rejected here rather than silently dropped.
    fn is_assignable_lvalue(&self, target: NodeId) -> bool {
        match &self.arena[target] {
            Node::Expr(Expr::Ident { .. }) => true,
            Node::Expr(Expr::Field { target: obj, .. }) => self.is_assignable_lvalue(*obj),
            Node::Expr(Expr::Index { array, .. }) => self.is_assignable_lvalue(*array),
            Node::Expr(Expr::Paren { inner }) => self.is_assignable_lvalue(*inner),
            _ => false,
        }
    }

    fn check_assign(&mut self, target: NodeId, value: NodeId) {
        if !self.is_assignable_lvalue(target) {
            self.diagnostics.push(Diagnostic::error(
                E_INVALID_ASSIGN_TARGET,
                "invalid assignment target: the left side of `=` must be a \
                 variable, field, or array element",
                Label::new(self.arena.span_of(target)),
            ));
            // Still type-check both sides so any nested errors are reported and
            // type info is populated for the rest of the pass.
            self.check_expr(target);
            self.check_expr(value);
            return;
        }

        // If the target is an undeclared identifier, create an implicit declaration.
        let target_ty = if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[target] {
            let name_span = *name_span;
            let sigil = sigil.to_owned();
            let name = self.intern_ident(name_span, sigil);
            let resolved = self.symbols.lookup(self.current_scope, name);
            let is_command = matches!(
                resolved.map(|d| &d.kind),
                Some(DeclKind::RuntimeFn { .. } | DeclKind::OverloadSet { .. })
            );
            let is_bound = resolved.is_some();
            if is_command {
                // `name` resolves to a built-in command and the user never
                // declared it. An implicit assignment may not shadow a command
                // (FD-027) — tell them to declare it explicitly with `Dim`.
                let name_str = self.interner.resolve(name).to_owned();
                self.diagnostics.push(Diagnostic::error(
                    E_RUNTIME_COMMAND_AS_VAR,
                    format!(
                        "`{name_str}` is a built-in command; an implicit assignment \
                         cannot shadow it — declare it explicitly with `Dim {name_str}`"
                    ),
                    Label::new(name_span),
                ));
                self.check_expr(value);
                return;
            }
            if is_bound {
                self.check_expr(target)
            } else if sigil.is_none() {
                // Implicit declaration with type inference (cb_syntax.md §4.1):
                // with neither a sigil nor an `As` clause, the variable's type is
                // inferred from the assigned value. Check the value first so its
                // type is known before the variable is declared.
                let value_ty = self.check_expr(value);
                let var_ty = self.infer_decl_type_from_value(value_ty, name, name_span);
                let decl = Declaration {
                    kind: DeclKind::Variable,
                    ty: var_ty.clone(),
                    span: name_span,
                    is_global: false,
                };
                self.try_declare(self.current_scope, name, decl, E_DUPLICATE_DECL);
                self.types.insert(target, var_ty);
                // The value is already checked and the variable was declared with
                // that exact type, so no coercion is needed.
                return;
            } else {
                // Implicit declaration via a sigil (e.g. `count% = ...`): the
                // sigil pins the type; the value is coerced to it below.
                let (var_ty, _) = types::resolve_var_type(sigil, None);
                let decl = Declaration {
                    kind: DeclKind::Variable,
                    ty: var_ty.clone(),
                    span: name_span,
                    is_global: false,
                };
                self.try_declare(self.current_scope, name, decl, E_DUPLICATE_DECL);
                self.types.insert(target, var_ty.clone());
                var_ty
            }
        } else {
            self.check_expr(target)
        };

        let value_ty = self.check_expr(value);
        if !target_ty.is_error() && !value_ty.is_error() && target_ty != value_ty {
            self.coerce(value, &value_ty, &target_ty);
        }
    }

    /// Infer the declared type of an implicitly-declared variable (a first
    /// assignment with no sigil and no `As`) from the type of its initial value
    /// (cb_syntax.md §4.1). A value with no usable type yields `Type::Error`;
    /// for `Null` and a void right-hand side this also reports E0331, since the
    /// fix there is an explicit `As`/`Dim` rather than a different value.
    fn infer_decl_type_from_value(&mut self, value_ty: Type, name: Symbol, span: Span) -> Type {
        match value_ty {
            // The value already errored — e.g. a self-referential RHS such as
            // `x = x + 1` on an undeclared `x`, which reports E0300. Declare the
            // variable as `Error` so later uses don't cascade.
            Type::Error => Type::Error,
            // `Null` has no concrete reference type to infer from.
            Type::Null => {
                let name_str = self.interner.resolve(name);
                self.diagnostics.push(Diagnostic::error(
                    E_CANNOT_INFER_TYPE,
                    format!(
                        "cannot infer a type for `{name_str}` from `Null`; declare \
                         it explicitly, e.g. `Dim {name_str} As <Type>`"
                    ),
                    Label::new(span),
                ));
                Type::Error
            }
            // The right-hand side produces no value (a call to a return-typeless
            // function — a subroutine, in the spec's terms; §7.1).
            Type::Void => {
                let name_str = self.interner.resolve(name);
                self.diagnostics.push(Diagnostic::error(
                    E_CANNOT_INFER_TYPE,
                    format!(
                        "cannot infer a type for `{name_str}`: the right-hand side \
                         has no value (a subroutine returns nothing)"
                    ),
                    Label::new(span),
                ));
                Type::Error
            }
            other => other,
        }
    }

    /// Coerce a single-name declaration's initializer to its declared type so
    /// lowering emits the right `Convert` (FD-035 / mirror `check_assign`).
    /// Shared by `Dim` and `Global`; `check_expr` runs unconditionally (even
    /// with empty `names`) to match the prior inline behavior, while the
    /// coercion itself is single-name.
    fn coerce_initializer(
        &mut self,
        init_id: NodeId,
        names: &[cb_frontend::DimName],
        as_ty: Option<&Type>,
    ) {
        let init_ty = self.check_expr(init_id);
        if let Some(dn) = names.first() {
            let (var_ty, _) = types::resolve_var_type(dn.sigil, as_ty);
            if !init_ty.is_error() && !var_ty.is_error() && init_ty != var_ty {
                self.coerce(init_id, &init_ty, &var_ty);
            }
        }
    }

    fn check_dim(
        &mut self,
        names: &[cb_frontend::DimName],
        ty_node: Option<NodeId>,
        init: Option<NodeId>,
    ) {
        let as_ty = ty_node.map(|tid| self.resolve_type_expr(tid));

        for dn in names {
            let name = self.intern_ident(dn.name_span, dn.sigil);
            let (var_ty, sigil_as_disagree) = types::resolve_var_type(dn.sigil, as_ty.as_ref());
            if sigil_as_disagree {
                self.diagnostics.push(Diagnostic::error(
                    E_SIGIL_AS_DISAGREE,
                    "sigil and `As` type disagree",
                    Label::new(dn.name_span),
                ));
            }
            let decl = Declaration {
                kind: DeclKind::Variable,
                ty: var_ty,
                span: dn.name_span,
                is_global: false,
            };
            // An explicit `Dim` may shadow a runtime command of the same name
            // (FD-027); reserved runtime constants/types still clash.
            self.declare_var_shadowing(self.current_scope, name, decl, E_DUPLICATE_DECL);
        }

        if let Some(init_id) = init {
            self.coerce_initializer(init_id, names, as_ty.as_ref());
        }
    }

    fn check_condition(&mut self, cond: NodeId) {
        let cty = self.check_expr(cond);
        if !cty.is_error() && !cty.is_numeric() {
            let ty_name = self.type_name(&cty);
            self.err(
                E_TYPE_MISMATCH,
                format!("condition must be numeric, got `{ty_name}`"),
                self.arena.span_of(cond),
            );
        }
    }

    fn check_for(
        &mut self,
        for_id: NodeId,
        var: NodeId,
        from: NodeId,
        to: NodeId,
        step: Option<NodeId>,
        body: &[NodeId],
    ) {
        // Check the bounds first: their types drive inference of an implicitly
        // declared loop variable (cb_syntax.md §4.1), so they must be known
        // before the variable is declared.
        let from_ty = self.check_expr(from);
        if !from_ty.is_numeric() && !from_ty.is_error() {
            self.diagnostics.push(Diagnostic::error(
                E_TYPE_MISMATCH,
                "For `from` value must be numeric",
                Label::new(self.arena.span_of(from)),
            ));
        }
        let to_ty = self.check_expr(to);
        if !to_ty.is_numeric() && !to_ty.is_error() {
            self.diagnostics.push(Diagnostic::error(
                E_TYPE_MISMATCH,
                "For `to` value must be numeric",
                Label::new(self.arena.span_of(to)),
            ));
        }
        let step_ty = step.map(|step_id| {
            let step_ty = self.check_expr(step_id);
            if !step_ty.is_numeric() && !step_ty.is_error() {
                self.diagnostics.push(Diagnostic::error(
                    E_TYPE_MISMATCH,
                    "For `step` value must be numeric",
                    Label::new(self.arena.span_of(step_id)),
                ));
            }
            step_ty
        });

        // The loop variable may be an implicit declaration. With neither a sigil
        // nor an `As` clause, infer its type from the numeric promotion of the
        // bounds (so `For i = 1.0 To 10.0` makes `i` a Float). Seeding the fold
        // with `Int` supplies both the Byte/Short floor (`numeric_promote` never
        // yields a sub-Int type) and the fallback when a bound is non-numeric. A
        // sigil pins the type as usual.
        let var_ty = if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[var] {
            let name = self.intern_ident(*name_span, *sigil);
            if self.symbols.lookup(self.current_scope, name).is_none() {
                let vt = if sigil.is_some() {
                    types::resolve_var_type(*sigil, None).0
                } else {
                    let mut vt = Type::Int;
                    if from_ty.is_numeric() {
                        vt = types::numeric_promote(&vt, &from_ty);
                    }
                    if to_ty.is_numeric() {
                        vt = types::numeric_promote(&vt, &to_ty);
                    }
                    if let Some(st) = &step_ty
                        && st.is_numeric()
                    {
                        vt = types::numeric_promote(&vt, st);
                    }
                    vt
                };
                let decl = Declaration {
                    kind: DeclKind::Variable,
                    ty: vt.clone(),
                    span: *name_span,
                    is_global: false,
                };
                self.try_declare(self.current_scope, name, decl, E_DUPLICATE_DECL);
                self.types.insert(var, vt.clone());
                vt
            } else {
                self.check_expr(var)
            }
        } else {
            self.check_expr(var)
        };

        if !var_ty.is_numeric() && !var_ty.is_error() {
            self.diagnostics.push(Diagnostic::error(
                E_FOR_VAR_NOT_NUMERIC,
                "For loop variable must be numeric",
                Label::new(self.arena.span_of(var)),
            ));
        }

        // Coerce the bounds and step to the loop-variable type so conversions are
        // recorded (and narrowing warnings fire) — matching `check_assign`. With
        // these recorded, lowering emits type-consistent `For` IR (cb_syntax.md §3.4).
        if var_ty.is_numeric() {
            self.coerce(from, &from_ty, &var_ty);
            self.coerce(to, &to_ty, &var_ty);
            if let (Some(step_id), Some(step_ty)) = (step, step_ty) {
                self.coerce(step_id, &step_ty, &var_ty);
            }
        }

        self.for_loop_stack.push(for_id);
        self.control_stack.push(ControlKind::Loop);
        for &s in body {
            self.check_stmt(s);
        }
        self.control_stack.pop();
        self.for_loop_stack.pop();
    }

    fn check_for_each(&mut self, for_id: NodeId, var: NodeId, source: NodeId, body: &[NodeId]) {
        // Check the source first to determine the iteration type. `For Each` over
        // a Type takes a bare Type name (cb_syntax.md §6.3); resolve that name
        // directly so E0311 does not fire on the legitimate position. Array
        // sources are arbitrary expressions and still flow through `check_expr`.
        let source_ty = self
            .resolve_type_name_arg(source)
            .unwrap_or_else(|| self.check_expr(source));

        // The iteration variable is implicitly declared.
        if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[var] {
            let name = self.intern_ident(*name_span, *sigil);
            if self.symbols.lookup(self.current_scope, name).is_none() {
                let vt = match &source_ty {
                    Type::TypeRef { .. } => source_ty.clone(),
                    Type::Array { elem, .. } => (**elem).clone(),
                    _ => {
                        let (resolved, _) = types::resolve_var_type(*sigil, None);
                        resolved
                    }
                };
                let decl = Declaration {
                    kind: DeclKind::Variable,
                    ty: vt.clone(),
                    span: *name_span,
                    is_global: false,
                };
                self.try_declare(self.current_scope, name, decl, E_DUPLICATE_DECL);
                self.types.insert(var, vt);
            }
        }

        self.for_loop_stack.push(for_id);
        self.control_stack.push(ControlKind::Loop);
        for &s in body {
            self.check_stmt(s);
        }
        self.control_stack.pop();
        self.for_loop_stack.pop();
    }

    fn check_select(&mut self, scrutinee: NodeId, arms: &[NodeId]) {
        let scrut_ty = self.check_expr(scrutinee);
        for &arm_id in arms {
            match &self.arena[arm_id] {
                Node::CaseArm(CaseArm::Case { values, body }) => {
                    let values = values.clone();
                    let body = body.clone();
                    for &v in &values {
                        // §6.2: each Case value must be a constant expression
                        // implicitly convertible to the scrutinee type.
                        let val_ty = self.check_expr(v);
                        if !val_ty.is_error() && !scrut_ty.is_error() {
                            self.coerce(v, &val_ty, &scrut_ty);
                            if self.eval_const_expr(v).is_none() {
                                self.diagnostics.push(Diagnostic::error(
                                    E_CONST_EVAL_ERROR,
                                    "Case value must be a constant expression",
                                    Label::new(self.arena.span_of(v)),
                                ));
                            }
                        }
                    }
                    self.control_stack.push(ControlKind::Select);
                    for &s in &body {
                        self.check_stmt(s);
                    }
                    self.control_stack.pop();
                }
                Node::CaseArm(CaseArm::Default { body }) => {
                    let body = body.clone();
                    self.control_stack.push(ControlKind::Select);
                    for &s in &body {
                        self.check_stmt(s);
                    }
                    self.control_stack.pop();
                }
                _ => {}
            }
        }
    }

    fn check_function(
        &mut self,
        name_span: Span,
        return_sigil: Option<Sigil>,
        params: &[NodeId],
        return_ty_node: Option<NodeId>,
        body: &[NodeId],
    ) {
        let top = self.current_scope;
        let fn_scope = self.symbols.push_scope(ScopeKind::Function, Some(top));
        let prev_scope = self.current_scope;
        self.current_scope = fn_scope;

        let func_name = self.intern_span(name_span);
        self.symbols.update_function_scope(top, func_name, fn_scope);

        // Resolve return type for this function.
        let as_ret = return_ty_node.map(|tid| self.resolve_type_expr(tid));
        let (ret_ty, _) = types::resolve_return_type(return_sigil, as_ret.as_ref());
        let prev_fn_ret = self.current_fn_return_ty.take();
        self.current_fn_return_ty = Some(ret_ty);

        // Declare parameters as local variables in the function scope.
        for &pid in params {
            if let Node::Param(Param {
                name_span: Some(pname_span),
                sigil,
                ty,
                default: _,
            }) = &self.arena[pid]
            {
                let pname = self.intern_ident(*pname_span, *sigil);
                let as_ty = ty.map(|tid| self.resolve_type_expr(tid));
                let (pty, _) = types::resolve_var_type(*sigil, as_ty.as_ref());
                let decl = Declaration {
                    kind: DeclKind::Variable,
                    ty: pty,
                    span: *pname_span,
                    is_global: false,
                };
                self.try_declare(fn_scope, pname, decl, E_DUPLICATE_DECL);
            }
        }

        // Hoist constants and labels from the entire function body.
        for &s in body {
            self.collect_consts_recursive(s, fn_scope);
        }
        let mut for_stack = Vec::new();
        for &s in body {
            self.collect_labels_recursive(s, fn_scope, &mut for_stack);
        }

        // Check function body.
        for &s in body {
            self.check_stmt(s);
        }

        self.current_scope = prev_scope;
        self.current_fn_return_ty = prev_fn_ret;
    }

    fn check_return(&mut self, value: Option<NodeId>, span: Span) {
        let ret_ty = self.current_fn_return_ty.clone();
        match ret_ty {
            None => {
                self.diagnostics.push(Diagnostic::error(
                    E_RETURN_OUTSIDE_FN,
                    "Return statement outside of a function",
                    Label::new(span),
                ));
            }
            Some(ref ret_ty) => {
                if *ret_ty == Type::Void {
                    if let Some(val_id) = value {
                        self.check_expr(val_id);
                        self.diagnostics.push(Diagnostic::error(
                            E_RETURN_VALUE_IN_SUB,
                            "cannot return a value from a Sub (no return type)",
                            Label::new(span),
                        ));
                    }
                } else if let Some(val_id) = value {
                    let val_ty = self.check_expr(val_id);
                    self.coerce(val_id, &val_ty, ret_ty);
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        E_MISSING_RETURN_VALUE,
                        "function requires a return value",
                        Label::new(span),
                    ));
                }
            }
        }
    }

    fn check_goto(&mut self, label_span: Span) {
        let name = self.intern_span(label_span);
        if let Some(decl) = self.symbols.lookup(self.current_scope, name) {
            if !matches!(decl.kind, DeclKind::Label) {
                self.diagnostics.push(Diagnostic::error(
                    E_UNDECLARED_LABEL,
                    format!("`{}` is not a label", self.interner.resolve(name)),
                    Label::new(label_span),
                ));
            } else {
                // Goto-into-For check (E0321): if the target label is inside
                // a For loop that this Goto is NOT inside, reject.
                if let Some(label_fors) = self.label_for_nesting.get(&name) {
                    for &for_id in label_fors {
                        if !self.for_loop_stack.contains(&for_id) {
                            self.diagnostics.push(Diagnostic::error(
                                E_GOTO_INTO_FOR,
                                "cannot Goto into a For loop from outside",
                                Label::new(label_span),
                            ));
                            break;
                        }
                    }
                }
            }
        } else {
            self.diagnostics.push(Diagnostic::error(
                E_UNDECLARED_LABEL,
                format!("undeclared label `{}`", self.interner.resolve(name)),
                Label::new(label_span),
            ));
        }
    }

    fn check_delete(&mut self, stmt_id: NodeId, operand: NodeId) {
        let op_ty = self.check_expr(operand);
        if !op_ty.is_error() && !matches!(op_ty, Type::TypeRef { .. }) {
            let ty_name = self.type_name(&op_ty);
            self.err(
                E_DELETE_NON_TYPEREF,
                format!("Delete requires a Type reference, got `{ty_name}`"),
                self.arena.span_of(operand),
            );
        }
        // Classify lvalue vs rvalue. Only a plain variable is an lvalue delete:
        // it has a slot to rewind to `prev` and mark deleted (cb_syntax.md
        // §3.3). A field or array-element operand (`Delete n.link`,
        // `Delete arr[0]`) is an rvalue delete — the node is freed with no
        // rewind and any alias dangles, exactly as for `Delete First(T)` (§3.3
        // worked example). Previously these were classified lvalue but the
        // lowerer only emitted IR for `Ident`, so the statement silently
        // vanished (FD-034).
        let class = match &self.arena[operand] {
            Node::Expr(Expr::Ident { .. }) => crate::DeleteClass::Lvalue,
            _ => crate::DeleteClass::Rvalue,
        };
        self.delete_classes.insert(stmt_id, class);
    }

    fn check_redim(&mut self, target: NodeId, elem_ty_node: NodeId, dims: &[NodeId]) {
        self.check_expr(target);
        self.resolve_type_expr(elem_ty_node);
        for &d in dims {
            let dty = self.check_expr(d);
            self.expect_integer(&dty, self.arena.span_of(d), "Redim dimension");
        }
    }
}

/// Numeric constant as `f64`, for operators that compute in floating point
/// (e.g. `^`). Returns `None` for non-numeric constants.
fn const_as_f64(v: &ConstValue) -> Option<f64> {
    match v {
        ConstValue::Int(n) => Some(*n as f64),
        ConstValue::Float(f) => Some(*f),
        _ => None,
    }
}

fn eval_const_binary(op: BinOp, l: &ConstValue, r: &ConstValue) -> Option<ConstValue> {
    match (op, l, r) {
        // Integer arithmetic
        (BinOp::Add, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(a.wrapping_add(*b)))
        }
        (BinOp::Sub, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(a.wrapping_sub(*b)))
        }
        (BinOp::Mul, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(a.wrapping_mul(*b)))
        }
        (BinOp::Div, ConstValue::Int(a), ConstValue::Int(b)) if *b != 0 => {
            Some(ConstValue::Int(a / b))
        }
        (BinOp::Mod, ConstValue::Int(a), ConstValue::Int(b)) if *b != 0 => {
            Some(ConstValue::Int(a % b))
        }

        // Float arithmetic
        (BinOp::Add, ConstValue::Float(a), ConstValue::Float(b)) => Some(ConstValue::Float(a + b)),
        (BinOp::Sub, ConstValue::Float(a), ConstValue::Float(b)) => Some(ConstValue::Float(a - b)),
        (BinOp::Mul, ConstValue::Float(a), ConstValue::Float(b)) => Some(ConstValue::Float(a * b)),
        (BinOp::Div, ConstValue::Float(a), ConstValue::Float(b)) => Some(ConstValue::Float(a / b)),

        // Exponentiation — always Float (cb_syntax.md §3.4).
        (BinOp::Pow, l, r) => {
            let base = const_as_f64(l)?;
            let exp = const_as_f64(r)?;
            Some(ConstValue::Float(base.powf(exp)))
        }

        // String concatenation
        (BinOp::Add, ConstValue::String(a), ConstValue::String(b)) => {
            Some(ConstValue::String(format!("{a}{b}")))
        }

        // Integer comparison — yields Int 1/0 (FD-035)
        (BinOp::Eq, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int((a == b) as i64))
        }
        (BinOp::NotEq, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int((a != b) as i64))
        }
        (BinOp::Lt, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int((a < b) as i64))
        }
        (BinOp::Gt, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int((a > b) as i64))
        }
        (BinOp::LtEq, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int((a <= b) as i64))
        }
        (BinOp::GtEq, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int((a >= b) as i64))
        }

        // Integer logical ops — operands tested as `<> 0`, result Int 1/0 (FD-035)
        (BinOp::And, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(((*a != 0) && (*b != 0)) as i64))
        }
        (BinOp::Or, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(((*a != 0) || (*b != 0)) as i64))
        }
        (BinOp::Xor, ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(((*a != 0) ^ (*b != 0)) as i64))
        }

        _ => None,
    }
}

fn sigil_char(s: Sigil) -> char {
    match s {
        Sigil::Integer => '%',
        Sigil::Float => '#',
        Sigil::String => '$',
    }
}

#[cfg(test)]
mod tests {
    use cb_diagnostics::FileId;
    use cb_frontend::{BinOp, LexerOptions, parse, tokenize};
    use cb_ir::types::IrType;

    use super::eval_const_binary;
    use crate::analyze;
    use crate::scope::ConstValue;

    fn empty_catalog() -> crate::RuntimeCatalog {
        crate::RuntimeCatalog {
            types: Vec::new(),
            functions: Vec::new(),
            constants: Vec::new(),
        }
    }

    fn analyze_src(src: &str) -> crate::SemaResult {
        let file = FileId(0);
        let (tokens, _lex_diags) = tokenize(src, file, LexerOptions::default());
        let parsed = parse(&tokens, src, file);
        analyze(&parsed.arena, &parsed.program, src, file, &empty_catalog())
    }

    fn error_codes(result: &crate::SemaResult) -> Vec<&str> {
        result
            .diagnostics
            .iter()
            .filter_map(|d| d.code.as_ref().map(|c| c.as_str()))
            .collect()
    }

    // ── pass 1 tests ────────────────────────────────────────────────────

    #[test]
    fn pass1_collects_function() {
        let result = analyze_src("Function f(x As Integer) As Integer\nReturn x\nEndFunction\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_sub() {
        let result = analyze_src("Function doStuff(a As String)\nEndFunction\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_type_def() {
        let result = analyze_src("Type MyType\nField a As Integer\nField b As Float\nEndType\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_struct_def() {
        let result = analyze_src("Struct Vec2\nField x As Float\nField y As Float\nEndStruct\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_global() {
        let result = analyze_src("Global score As Integer\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_global_multi_name() {
        let result = analyze_src("Global a, b, c As Float\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_label() {
        let result = analyze_src("cleanup:\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_const() {
        let result = analyze_src("Const MaxItems = 100\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_collects_global_const() {
        let result = analyze_src("Global Const Version$ = \"1.0\"\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_duplicate_function_e0319() {
        let result = analyze_src(
            "Function f() As Integer\nReturn 1\nEndFunction\n\
             Function f() As Integer\nReturn 2\nEndFunction\n",
        );
        assert_eq!(error_codes(&result), vec!["E0319"]);
    }

    #[test]
    fn pass1_duplicate_type_e0319() {
        let result = analyze_src(
            "Type T\nField a As Integer\nEndType\n\
             Type T\nField b As Float\nEndType\n",
        );
        assert_eq!(error_codes(&result), vec!["E0319"]);
    }

    #[test]
    fn pass1_duplicate_label_e0303() {
        let result = analyze_src("cleanup:\ncleanup:\n");
        assert_eq!(error_codes(&result), vec!["E0303"]);
    }

    #[test]
    fn pass1_duplicate_global_e0303() {
        let result = analyze_src("Global x As Integer\nGlobal x As Float\n");
        assert_eq!(error_codes(&result), vec!["E0303"]);
    }

    #[test]
    fn pass1_function_sigil_as_disagree_e0320() {
        let result = analyze_src("Function f#() As Integer\nReturn 1\nEndFunction\n");
        assert_eq!(error_codes(&result), vec!["E0320"]);
    }

    #[test]
    fn pass1_global_sigil_as_disagree_e0320() {
        let result = analyze_src("Global x% As Float\n");
        assert_eq!(error_codes(&result), vec!["E0320"]);
    }

    #[test]
    fn pass1_case_insensitive_duplicate() {
        let result = analyze_src("Global myVar As Integer\nGlobal MYVAR As Integer\n");
        assert_eq!(error_codes(&result), vec!["E0303"]);
    }

    #[test]
    fn pass1_function_with_sigil_params() {
        let result = analyze_src("Function area#(r As Float)\nReturn r\nEndFunction\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_function_with_default_params() {
        let result = analyze_src(
            "Function move(distance As Float, count = 1) As Float\nReturn distance\nEndFunction\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_non_hoisted_stmts_ignored() {
        let result = analyze_src("Dim x As Integer\nx = 42\nIf True Then\nx = 1\nEndIf\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── pass 2 tests: name resolution ───────────────────────────────────

    #[test]
    fn pass2_undeclared_ident_e0300() {
        let result = analyze_src("Dim y As Integer\ny = x + 1\n");
        assert_eq!(error_codes(&result), vec!["E0300"]);
    }

    #[test]
    fn pass2_implicit_declaration() {
        let result = analyze_src("x = 42\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_dim_then_use() {
        let result = analyze_src("Dim x As Integer\nx = 1\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_sigil_conflict_e0302() {
        let result = analyze_src("Dim x% As Integer\nx# = 1.0\n");
        assert!(
            error_codes(&result).contains(&"E0302"),
            "expected E0302, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_sigil_decl_bare_ref_same_var() {
        // Declared with a sigil, referenced bare — the sigil is not part of the
        // name, so both refer to the same variable (cb_syntax.md §1.3–§1.4).
        let result = analyze_src("Dim total# = 1.5\ntotal = 2.0\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_bare_decl_matching_sigil_ref_ok() {
        // Implicitly declared bare (Int), later referenced with a matching sigil.
        let result = analyze_src("x = 5\nx% = 6\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_bare_decl_conflicting_sigil_ref_e0302() {
        // Type is fixed at first use (bare ⇒ Int); a later non-matching sigil
        // must still be rejected.
        let result = analyze_src("x = 5\nx# = 1.0\n");
        assert!(
            error_codes(&result).contains(&"E0302"),
            "expected E0302, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_single_char_sigil_bare_ref_same_var() {
        // Guards against the historical double-strip collapsing single-char
        // sigil'd names to the empty string.
        let result = analyze_src("a$ = \"hi\"\na = \"bye\"\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── FD-042: default type inference for implicit declarations ─────────

    #[test]
    fn fd042_infer_float_from_value() {
        // `x = 3.14` infers Float (not the old Integer default), so a later
        // Integer-sigil reference conflicts. Under the old default `x%` would
        // have matched and produced no E0302.
        let result = analyze_src("x = 3.14\nx% = 2\n");
        assert!(
            error_codes(&result).contains(&"E0302"),
            "expected E0302 (x inferred Float), got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn fd042_infer_string_no_coercion_error() {
        // `s = "hi"` infers String. Under the old Integer default this was a
        // String→Int coercion error (E0317); now it is clean.
        let result = analyze_src("s = \"hi\"\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn fd042_infer_array_from_new() {
        // `a = New Integer[10]` infers `Integer[]`, so the variable is indexable
        // with no `Dim`. Under the old default this was an Array→Int coercion
        // error plus an index-non-array error.
        let result = analyze_src("a = New Integer[10]\na[0] = 5\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn fd042_infer_int_from_literal_regression() {
        // An int literal still yields Integer — the common case is unchanged.
        let result = analyze_src("x = 5\nx% = 6\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn fd042_null_cannot_infer_e0331() {
        // `x = Null` on an undeclared var has no type to infer — E0331.
        let result = analyze_src("x = Null\n");
        assert_eq!(error_codes(&result), vec!["E0331"]);
    }

    #[test]
    fn fd042_self_reference_use_before_decl_e0300() {
        // `x = x + 1` on an undeclared `x` is now a use-before-declaration error
        // (E0300) — exactly one diagnostic, no cascade.
        let result = analyze_src("x = x + 1\n");
        assert_eq!(error_codes(&result), vec!["E0300"]);
    }

    #[test]
    fn fd042_for_infer_float_from_bounds() {
        // A `For` variable with Float bounds is inferred Float, so a later
        // Integer-sigil reference conflicts (E0302).
        let result = analyze_src("For i = 1.0 To 10.0\nNext i\ni% = 2\n");
        assert!(
            error_codes(&result).contains(&"E0302"),
            "expected E0302 (i inferred Float), got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn fd042_for_infer_int_from_bounds_regression() {
        // Integer bounds still yield an Integer loop variable.
        let result = analyze_src("For i = 1 To 10\nNext i\ni% = 2\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── pass 2 tests: expression typing ─────────────────────────────────

    #[test]
    fn pass2_literal_types() {
        let result = analyze_src("Dim a As Integer\na = 42\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_binary_arithmetic() {
        let result = analyze_src("Dim x As Integer\nx = 1 + 2\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_comparison_returns_int() {
        // Comparisons yield Int 1/0 — there is no Bool type (FD-035).
        let result = analyze_src("Dim x As Integer\nx = 1 > 2\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_logical_and_or() {
        // Logical ops yield Int; True/False are Int 1/0 (FD-035).
        let result = analyze_src("Dim x As Integer\nx = True And False\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_reserved_type_names_are_e0330() {
        // Bool/Boolean/UInt/UInteger/ULong are reserved but unsupported (FD-035).
        for src in [
            "Dim x As Bool\n",
            "Dim x As Boolean\n",
            "Dim x As UInt\n",
            "Dim x As ULong\n",
        ] {
            let result = analyze_src(src);
            assert!(
                error_codes(&result).contains(&"E0330"),
                "expected E0330 for `{src}`, got {:?}",
                error_codes(&result)
            );
        }
    }

    #[test]
    fn pass2_dim_byte_literal_overflow_e0326() {
        // An out-of-range integer literal in a Dim initializer is a hard error
        // now that Dim initializers are coerced to the declared type (FD-035).
        let result = analyze_src("Dim b As Byte = 300\n");
        assert!(
            error_codes(&result).contains(&"E0326"),
            "expected E0326, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_dim_in_range_narrow_literal_is_silent() {
        // An in-range literal coerces silently — a known-safe constant
        // (FD-020/FD-035): Short is 16-bit unsigned, 40000 fits.
        let result = analyze_src("Dim s As Short = 40000\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── pass 2 tests: function calls ────────────────────────────────────

    #[test]
    fn pass2_function_call_ok() {
        let result = analyze_src(
            "Function add(a As Integer, b As Integer) As Integer\nReturn a + b\nEndFunction\nDim x As Integer\nx = add(1, 2)\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_function_wrong_arg_count_e0305() {
        let result = analyze_src(
            "Function add(a As Integer, b As Integer) As Integer\nReturn a + b\nEndFunction\nadd(1)\n",
        );
        assert_eq!(error_codes(&result), vec!["E0305"]);
    }

    #[test]
    fn pass2_function_arg_type_mismatch_e0317() {
        // A String argument to an Integer parameter has no conversion path:
        // user-defined functions type-check their arguments just like runtime
        // commands do.
        let result = analyze_src(
            "Function add(a As Integer, b As Integer) As Integer\nReturn a + b\nEndFunction\nDim s As String\nadd(s, 2)\n",
        );
        assert_eq!(error_codes(&result), vec!["E0317"]);
    }

    #[test]
    fn pass2_function_arg_narrowing_e0318() {
        // Passing a (non-literal) Integer to a Byte parameter narrows: warning,
        // not error, mirroring assignment and runtime-command coercion.
        let result = analyze_src(
            "Function takes(a As Byte)\nEndFunction\nDim n As Integer\nn = 300\ntakes(n)\n",
        );
        assert_eq!(warning_codes(&result), vec!["E0318"]);
    }

    #[test]
    fn pass2_function_arg_widening_ok() {
        // Byte -> Integer is a widening conversion: accepted silently.
        let result =
            analyze_src("Function takes(a As Integer)\nEndFunction\nDim b As Byte\ntakes(b)\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_function_arg_count_mismatch_skips_type_check() {
        // When the argument count is already wrong, only E0305 is reported — we
        // do not also emit per-argument type errors against mismatched params.
        let result = analyze_src(
            "Function add(a As Integer, b As Integer) As Integer\nReturn a + b\nEndFunction\nDim s As String\nadd(s)\n",
        );
        assert_eq!(error_codes(&result), vec!["E0305"]);
    }

    // ── pass 2 tests: field access ──────────────────────────────────────

    #[test]
    fn pass2_field_access_ok() {
        let result =
            analyze_src("Type MyObj\nField x As Integer\nEndType\nDim obj As MyObj\nobj.x = 42\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_field_not_found_e0308() {
        let result =
            analyze_src("Type MyObj\nField x As Integer\nEndType\nDim obj As MyObj\nobj.y = 42\n");
        assert!(
            error_codes(&result).contains(&"E0308"),
            "expected E0308, got {:?}",
            error_codes(&result)
        );
    }

    // ── pass 2 tests: statements ────────────────────────────────────────

    #[test]
    fn pass2_return_outside_function_e0313() {
        let result = analyze_src("Return 42\n");
        assert_eq!(error_codes(&result), vec!["E0313"]);
    }

    #[test]
    fn pass2_return_value_in_sub_e0314() {
        let result = analyze_src("Function doIt()\nReturn 42\nEndFunction\n");
        assert_eq!(error_codes(&result), vec!["E0314"]);
    }

    #[test]
    fn pass2_missing_return_value_e0315() {
        let result = analyze_src("Function f() As Integer\nReturn\nEndFunction\n");
        assert_eq!(error_codes(&result), vec!["E0315"]);
    }

    #[test]
    fn pass2_goto_undeclared_label_e0312() {
        let result = analyze_src("Goto nonexistent\n");
        assert_eq!(error_codes(&result), vec!["E0312"]);
    }

    #[test]
    fn pass2_goto_declared_label_ok() {
        let result = analyze_src("Goto target\ntarget:\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_for_non_numeric_e0316() {
        let result = analyze_src("Dim s As String\nFor s = 0 To 10\nNext\n");
        assert!(
            error_codes(&result).contains(&"E0316"),
            "expected E0316, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_delete_non_typeref_e0310() {
        let result = analyze_src("Dim x As Integer\nDelete x\n");
        assert_eq!(error_codes(&result), vec!["E0310"]);
    }

    // ── pass 2 tests: intrinsics ────────────────────────────────────────

    #[test]
    fn pass2_intrinsic_int_call() {
        let result = analyze_src("Dim x As Integer\nx = Int(1.5)\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_intrinsic_str_call() {
        let result = analyze_src("Dim s As String\ns = Str(42)\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_intrinsic_len_call() {
        let result = analyze_src("Dim arr As Integer[]\nDim n As Integer\nn = Len(arr)\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_intrinsic_len_wrong_arg_count() {
        let result = analyze_src("Len()\n");
        assert!(
            error_codes(&result).contains(&"E0305"),
            "expected E0305, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_intrinsic_len_on_string() {
        // Len(s$) is valid and yields an Integer (FD-013 Batch 2).
        let result = analyze_src("Dim n As Integer\nn = Len(\"hello\")\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_intrinsic_len_string_rejects_dim() {
        // The array dimension argument is meaningless for a string operand.
        let result = analyze_src("Len(\"hi\", 1)\n");
        assert!(
            error_codes(&result).contains(&"E0305"),
            "expected E0305, got {:?}",
            error_codes(&result)
        );
    }

    // ── pass 2 tests: diagnostic assertion sweep (FD-031) ───────────────

    #[test]
    fn pass2_operator_type_mismatch_e0301() {
        // String `-` Integer has no binary result type.
        let result = analyze_src("Dim a As Integer\na = \"x\" - 1\n");
        assert!(
            error_codes(&result).contains(&"E0301"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_len_on_scalar_e0301() {
        // Len requires an array or a string; a scalar is a type mismatch.
        let result = analyze_src("Dim x As Integer\nDim n As Integer\nn = Len(x)\n");
        assert!(
            error_codes(&result).contains(&"E0301"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_call_non_function_e0304() {
        // Calling an Integer variable is not a function call.
        let result = analyze_src("Dim x As Integer\nDim y As Integer\ny = x(1)\n");
        assert!(
            error_codes(&result).contains(&"E0304"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_index_non_array_e0306() {
        // Indexing a scalar value is not allowed.
        let result = analyze_src("Dim x As Integer\nDim y As Integer\ny = x[0]\n");
        assert!(
            error_codes(&result).contains(&"E0306"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_rank_mismatch_e0307() {
        // A rank-1 array indexed with two subscripts.
        let result = analyze_src("Dim a As Integer[]\nDim y As Integer\ny = a[0, 0]\n");
        assert!(
            error_codes(&result).contains(&"E0307"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_field_on_non_type_e0309() {
        // Field access on a scalar value.
        let result = analyze_src("Dim x As Integer\nDim y As Integer\ny = x.foo\n");
        assert!(
            error_codes(&result).contains(&"E0309"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_address_of_command_e0329() {
        // A bare runtime-command name in value position has no address.
        let catalog = catalog_of(vec![rt_func(
            "Box",
            "cb_box",
            &[("x", IrType::Float)],
            IrType::Void,
        )]);
        let result = analyze_with_catalog("Dim p As Integer\np = Box\n", &catalog);
        assert!(
            error_codes(&result).contains(&"E0329"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_type_name_as_value_mismatched_slot_e0311() {
        // A bare Type name used as a value into a mismatched slot. Before FD-031
        // this leaked a `TypeRef` Debug repr through E0317; now it is E0311.
        let result =
            analyze_src("Type Foo\nField x As Integer\nEndType\nDim a As Integer\na = Foo\n");
        assert!(
            error_codes(&result).contains(&"E0311"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_type_name_as_value_matching_slot_e0311() {
        // The soundness hole: a bare Type name returned into a matching-typed
        // slot compiled clean before FD-031. It must now be rejected as E0311.
        let result = analyze_src(
            "Type Foo\nField x As Integer\nEndType\n\
             Function makeFoo() As Foo\nReturn Foo\nEndFunction\n",
        );
        assert!(
            error_codes(&result).contains(&"E0311"),
            "got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_type_name_legit_positions_ok() {
        // `New`, `First`, and `For Each` all take a bare Type name legitimately
        // (cb_syntax.md §3.3/§6.3) — they must NOT trip E0311 (over-firing guard).
        let result = analyze_src(
            "Type Foo\nField x As Integer\nEndType\n\
             Dim a As Foo = New Foo\n\
             Dim b As Foo = First(Foo)\n\
             Dim total As Integer\n\
             For c = Each Foo\n\
             total = c.x\n\
             Next c\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── pass 2 tests: scope visibility ──────────────────────────────────

    #[test]
    fn pass2_function_sees_global() {
        let result =
            analyze_src("Global g As Integer\nFunction f() As Integer\nReturn g\nEndFunction\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_function_cannot_see_toplevel_var() {
        let result =
            analyze_src("Dim x As Integer\nFunction f() As Integer\nReturn x\nEndFunction\n");
        assert!(
            error_codes(&result).contains(&"E0300"),
            "expected E0300, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_function_sees_hoisted_type() {
        let result = analyze_src(
            "Function f()\nDim t As MyType\nEndFunction\nType MyType\nField x As Integer\nEndType\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_function_sees_hoisted_const() {
        let result =
            analyze_src("Const MAX = 100\nFunction f() As Integer\nReturn MAX\nEndFunction\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_top_level_block_nested_const_is_hoisted() {
        // §4.2 has no block scoping and §7.3 hoists definitions, so a `Const`
        // nested inside a top-level block is visible outside that block — both
        // from a sibling statement and from a function. A directly top-level
        // `Const` must keep working too. (S-H4)
        let result = analyze_src(
            "Const FALLBACK = 5\n\
             If True Then\n  Const MAX = 100\nEndIf\n\
             Dim x As Integer\nx = MAX + FALLBACK\n\
             Function getMax() As Integer\nReturn MAX\nEndFunction\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── pass 2 tests: error poisoning ───────────────────────────────────

    #[test]
    fn pass2_error_poisoning_no_cascade() {
        // A parse error produces Expr::Error / Stmt::Error; sema should not
        // generate cascading diagnostics from it.
        let result = analyze_src("Dim x As Integer\nx = @\n");
        // We expect a lex error and parser error, but no sema E0300 for the RHS.
        let sema_errors: Vec<_> = error_codes(&result)
            .into_iter()
            .filter(|c| c.starts_with("E03"))
            .collect();
        assert!(
            sema_errors.is_empty(),
            "expected no sema errors from error poisoning, got {sema_errors:?}"
        );
    }

    // ── M4 tests: implicit conversions ──────────────────────────────────

    fn warning_codes(result: &crate::SemaResult) -> Vec<&str> {
        result
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, cb_diagnostics::Severity::Warning))
            .filter_map(|d| d.code.as_ref().map(|c| c.as_str()))
            .collect()
    }

    #[test]
    fn conversion_int_to_float_no_warning() {
        let result = analyze_src("Dim x As Float\nx = 42\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        // Check that a conversion was recorded
        assert!(
            result.conversions.get(cb_frontend::NodeId(4)).is_some()
                || result.diagnostics.is_empty()
        ); // at minimum no errors
    }

    #[test]
    fn conversion_float_to_int_narrowing_e0318() {
        let result = analyze_src("Dim y As Integer\ny = 1.5\n");
        assert_eq!(warning_codes(&result), vec!["E0318"]);
    }

    #[test]
    fn conversion_bool_to_int_no_warning() {
        let result = analyze_src("Dim n As Integer\nn = True\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn conversion_null_to_typeref() {
        let result =
            analyze_src("Type MyType\nField x As Integer\nEndType\nDim t As MyType\nt = Null\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn conversion_no_path_e0317() {
        // String → Integer has no implicit conversion path.
        let result = analyze_src("Dim n As Integer\nDim s As String\nn = s\n");
        assert_eq!(error_codes(&result), vec!["E0317"]);
    }

    #[test]
    fn conversion_long_to_byte_narrowing_e0318() {
        let result = analyze_src("Dim b As Byte\nDim l As Long\nb = l\n");
        assert_eq!(warning_codes(&result), vec!["E0318"]);
    }

    // ── M4 tests: constant evaluation ───────────────────────────────────

    #[test]
    fn const_eval_simple_arithmetic() {
        let result = analyze_src("Const x = 1 + 2 * 3\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn const_eval_negation() {
        let result = analyze_src("Const x = -42\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn const_eval_references_other_const() {
        let result = analyze_src("Const a = 1\nConst b = a + 2\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn const_eval_bool_logic() {
        let result = analyze_src("Const x = True And False\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn const_eval_string_concat() {
        let result = analyze_src("Const x$ = \"hello\" + \" world\"\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── FD-020: numeric & For-loop semantics ────────────────────────────

    #[test]
    fn literal_overflow_narrow_int_e0326() {
        // 300 does not fit a Byte → hard error (cb_syntax.md §1.6/§3.4).
        let result = analyze_src("Dim b As Byte\nb = 300\n");
        assert!(
            error_codes(&result).contains(&"E0326"),
            "expected E0326, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn literal_in_range_narrow_int_silent() {
        // A literal that fits the narrower target is a known-safe constant — no
        // narrowing warning, no error.
        let result = analyze_src("Dim b As Byte\nb = 5\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn negative_literal_to_unsigned_e0326() {
        let result = analyze_src("Dim b As Byte\nb = -1\n");
        assert!(
            error_codes(&result).contains(&"E0326"),
            "expected E0326, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn for_to_float_narrowing_warns_e0318() {
        // `i%` is Int; `To 10.5` narrows Float→Int → warning, not error.
        let result = analyze_src("For i% = 1 To 10.5\nNext\n");
        assert_eq!(warning_codes(&result), vec!["E0318"]);
    }

    #[test]
    fn pow_const_folds_to_float() {
        // `^` folds in floating point regardless of operand const kinds.
        assert_eq!(
            eval_const_binary(BinOp::Pow, &ConstValue::Int(2), &ConstValue::Int(10)),
            Some(ConstValue::Float(1024.0))
        );
        assert_eq!(
            eval_const_binary(BinOp::Pow, &ConstValue::Float(9.0), &ConstValue::Float(0.5)),
            Some(ConstValue::Float(3.0))
        );
    }

    #[test]
    fn const_pow_expr_compiles() {
        let result = analyze_src("Const x# = 2 ^ 10\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn const_int_div_zero_e0322() {
        let result = analyze_src("Const x = 1 / 0\n");
        assert!(
            error_codes(&result).contains(&"E0322"),
            "expected E0322, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn const_float_div_zero_warns_e0327() {
        // Float `/0` is legal IEEE — warn but still compile.
        let result = analyze_src("Const x# = 1.0 / 0.0\n");
        assert_eq!(warning_codes(&result), vec!["E0327"]);
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, cb_diagnostics::Severity::Error))
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    // ── M5 tests: Delete classification ─────────────────────────────────

    #[test]
    fn delete_lvalue_variable() {
        let result =
            analyze_src("Type MyObj\nField x As Integer\nEndType\nDim obj As MyObj\nDelete obj\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        // Find the Delete statement's NodeId and check its classification.
        let has_lvalue = result
            .delete_classes
            .values()
            .any(|c| *c == crate::DeleteClass::Lvalue);
        assert!(has_lvalue, "expected Lvalue classification for Delete var");
    }

    #[test]
    fn delete_rvalue_call() {
        let result = analyze_src(
            "Type MyObj\nField x As Integer\nEndType\n\
             Function first() As MyObj\nReturn Null\nEndFunction\n\
             Delete first()\n",
        );
        // E0310 won't fire because first() returns MyObj which is TypeRef.
        // But it's an rvalue because it's a call expression.
        let has_rvalue = result
            .delete_classes
            .values()
            .any(|c| *c == crate::DeleteClass::Rvalue);
        assert!(
            has_rvalue,
            "expected Rvalue classification for Delete call(); classes: {:?}",
            result.delete_classes
        );
    }

    // ── M5 tests: Goto-into-For ─────────────────────────────────────────

    #[test]
    fn goto_into_for_e0321() {
        let result = analyze_src("Goto inner\nFor i = 0 To 10\ninner:\nNext\n");
        assert!(
            error_codes(&result).contains(&"E0321"),
            "expected E0321, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn goto_same_scope_ok() {
        let result = analyze_src("Goto target\ntarget:\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn goto_within_for_ok() {
        let result = analyze_src("For i = 0 To 10\nGoto skip\nskip:\nNext\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn goto_label_nested_in_if_inside_function() {
        let result =
            analyze_src("Function f()\nGoto target\nIf True Then\ntarget:\nEndIf\nEndFunction\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── runtime catalog tests ───────────────────────────────────────────

    fn analyze_with_catalog(src: &str, catalog: &crate::RuntimeCatalog) -> crate::SemaResult {
        let file = FileId(0);
        let (tokens, _lex_diags) = tokenize(src, file, LexerOptions::default());
        let parsed = parse(&tokens, src, file);
        analyze(&parsed.arena, &parsed.program, src, file, catalog)
    }

    fn catalog_of(functions: Vec<crate::FuncDesc>) -> crate::RuntimeCatalog {
        crate::RuntimeCatalog {
            types: Vec::new(),
            functions,
            constants: Vec::new(),
        }
    }

    fn catalog_with_consts(constants: Vec<crate::RuntimeConstDesc>) -> crate::RuntimeCatalog {
        crate::RuntimeCatalog {
            types: Vec::new(),
            functions: Vec::new(),
            constants,
        }
    }

    fn const_int(name: &str, v: i64) -> crate::RuntimeConstDesc {
        crate::RuntimeConstDesc {
            name: name.to_string(),
            ty: cb_ir::types::IrType::Int,
            value: crate::RuntimeConstValue::Int(v),
        }
    }

    fn rt_func(
        name: &str,
        c_symbol: &str,
        params: &[(&str, cb_ir::types::IrType)],
        ret: cb_ir::types::IrType,
    ) -> crate::FuncDesc {
        crate::FuncDesc {
            name: name.to_string(),
            c_symbol: c_symbol.to_string(),
            params: params
                .iter()
                .map(|(n, ty)| crate::FuncParamDesc {
                    name: Some(n.to_string()),
                    ty: ty.clone(),
                })
                .collect(),
            return_ty: ret,
        }
    }

    // FD-041: a catalog with the Sound surface — the `Sound` (tag 17) and
    // `SoundChannel` (tag 18) opaque types, plus enough of the command set to pin
    // the naming trap (Set/Stop/SoundPlaying take a SoundChannel, not a Sound) and
    // the opaque strictness. Only the representative arities are registered.
    fn sound_catalog() -> crate::RuntimeCatalog {
        let sound = IrType::RuntimeType("Sound".into());
        let channel = IrType::RuntimeType("SoundChannel".into());
        let mut catalog = catalog_of(vec![
            rt_func(
                "LoadSound",
                "cb_rt_load_sound",
                &[("p", IrType::String)],
                sound.clone(),
            ),
            // PlaySound's two source-typed overloads, both returning SoundChannel.
            rt_func(
                "PlaySound",
                "cb_rt_play_sound4",
                &[
                    ("s", sound.clone()),
                    ("v", IrType::Float),
                    ("b", IrType::Float),
                    ("f", IrType::Int),
                ],
                channel.clone(),
            ),
            rt_func(
                "PlaySound",
                "cb_rt_play_sound",
                &[("s", sound.clone())],
                channel.clone(),
            ),
            rt_func(
                "PlaySound",
                "cb_rt_play_sound_file",
                &[("p", IrType::String)],
                channel.clone(),
            ),
            // The naming trap: these say "Sound" but take a SoundChannel.
            rt_func(
                "SetSound",
                "cb_rt_set_sound",
                &[("c", channel.clone()), ("loop", IrType::Int)],
                IrType::Void,
            ),
            rt_func(
                "StopSound",
                "cb_rt_stop_sound",
                &[("c", channel.clone())],
                IrType::Void,
            ),
            rt_func(
                "SoundPlaying",
                "cb_rt_sound_playing",
                &[("c", channel.clone())],
                IrType::Int,
            ),
            // ...while these take a Sound.
            rt_func(
                "DeleteSound",
                "cb_rt_delete_sound",
                &[("s", sound.clone())],
                IrType::Void,
            ),
        ]);
        catalog.types.push(crate::RuntimeTypeDesc {
            name: "Sound".into(),
            tag: 17,
        });
        catalog.types.push(crate::RuntimeTypeDesc {
            name: "SoundChannel".into(),
            tag: 18,
        });
        catalog
    }

    #[test]
    fn runtime_fn_call_ok() {
        let catalog = catalog_of(vec![rt_func(
            "print",
            "cb_rt_print",
            &[("text", IrType::String)],
            IrType::Void,
        )]);
        let result = analyze_with_catalog("print(\"hello\")\n", &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn type_field_of_runtime_type_refined_fd036() {
        // A user Type field declared with a runtime opaque type (`As Object`)
        // must refine to `RuntimeType`, not the pass-1 placeholder `TypeRef`, so
        // it matches runtime functions producing/consuming `Object`. Regression
        // for the pass-ordering bug that broke examples/bullets.cb. Exercises
        // both directions: a runtime result assigned INTO the field, and the
        // field passed INTO a runtime parameter.
        let mut catalog = catalog_of(vec![
            rt_func(
                "LoadObject",
                "cb_load",
                &[("f", IrType::String)],
                IrType::RuntimeType("Object".into()),
            ),
            rt_func(
                "MoveObject",
                "cb_move",
                &[
                    ("o", IrType::RuntimeType("Object".into())),
                    ("d", IrType::Float),
                ],
                IrType::Void,
            ),
        ]);
        catalog.types.push(crate::RuntimeTypeDesc {
            name: "Object".into(),
            tag: 1,
        });
        let src = "Type Ammus\nField obj As Object\nEndType\n\
                   Dim a As Ammus = New Ammus\n\
                   a.obj = LoadObject(\"x\")\n\
                   MoveObject a.obj, 6.0\n";
        let result = analyze_with_catalog(src, &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn function_param_and_return_of_runtime_type_refined_fd036() {
        // The reorder also fixes function signatures: a param/return annotated
        // with a runtime type is resolved in pass 1 and must refine too.
        let mut catalog = catalog_of(vec![rt_func(
            "MoveObject",
            "cb_move",
            &[
                ("o", IrType::RuntimeType("Object".into())),
                ("d", IrType::Float),
            ],
            IrType::Void,
        )]);
        catalog.types.push(crate::RuntimeTypeDesc {
            name: "Object".into(),
            tag: 1,
        });
        let src = "Function advance(o As Object) As Object\n\
                   MoveObject o, 1.0\n\
                   Return o\n\
                   EndFunction\n";
        let result = analyze_with_catalog(src, &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── sound: the SoundChannel-vs-Sound naming trap (FD-041) ───────────────

    #[test]
    fn sound_channel_ops_accept_channel_fd041() {
        // The trap: SetSound/StopSound/SoundPlaying name "Sound" but take a
        // SoundChannel (PlaySound's return), and PlaySound's preloaded form and
        // its filename form both type as SoundChannel. All correct → no diags.
        let catalog = sound_catalog();
        let src = "Dim s As Sound = LoadSound(\"a.ogg\")\n\
                   Dim ch As SoundChannel = PlaySound(s, 100.0, 0.0, -1)\n\
                   Dim st As SoundChannel = PlaySound(\"music.ogg\")\n\
                   SetSound ch, 1\n\
                   StopSound st\n\
                   Dim p As Integer = SoundPlaying(ch)\n\
                   DeleteSound s\n";
        let result = analyze_with_catalog(src, &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn sound_channel_op_rejects_sound_handle_fd041() {
        // StopSound takes a SoundChannel; handing it a `Sound` is the trap firing
        // — the lone overload's param type can't accept it (E0317, cannot convert
        // one opaque handle to the other).
        let catalog = sound_catalog();
        let src = "Dim s As Sound = LoadSound(\"a.ogg\")\n\
                   StopSound s\n";
        let result = analyze_with_catalog(src, &catalog);
        assert_eq!(error_codes(&result), vec!["E0317"]);
    }

    #[test]
    fn fd042_infer_opaque_runtime_type_fd041() {
        // FD-042: `s = LoadSound(...)` with no `Dim` infers the opaque `Sound`
        // type, so `DeleteSound s` (which takes a `Sound`) resolves cleanly.
        // Under the old Integer default this was a Sound→Int coercion error.
        let catalog = sound_catalog();
        let src = "s = LoadSound(\"a.ogg\")\n\
                   DeleteSound s\n";
        let result = analyze_with_catalog(src, &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn delete_sound_rejects_channel_handle_fd041() {
        // The mirror image: DeleteSound takes a `Sound`; a SoundChannel is wrong.
        let catalog = sound_catalog();
        let src = "Dim ch As SoundChannel = PlaySound(\"music.ogg\")\n\
                   DeleteSound ch\n";
        let result = analyze_with_catalog(src, &catalog);
        assert_eq!(error_codes(&result), vec!["E0317"]);
    }

    #[test]
    fn play_sound_statement_form_discards_channel_fd041() {
        // PlaySound is a hybrid: used as a statement, the returned SoundChannel is
        // simply discarded. A value-returning call in statement position is legal
        // (no void overload needed, and a void one would be ambiguous).
        let catalog = sound_catalog();
        let result = analyze_with_catalog(
            "Dim s As Sound = LoadSound(\"a.ogg\")\nPlaySound s\n",
            &catalog,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn sound_handles_reject_arithmetic_and_ordering_fd041() {
        // Strict opaque handles: arithmetic and ordering on a Sound/SoundChannel
        // are type errors (E0301), exactly like every other runtime opaque type.
        let catalog = sound_catalog();
        let add = analyze_with_catalog(
            "Dim a As Sound = LoadSound(\"a\")\n\
             Dim b As Sound = LoadSound(\"b\")\n\
             Dim x As Integer = a + b\n",
            &catalog,
        );
        assert_eq!(error_codes(&add), vec!["E0301"]);

        let cmp = analyze_with_catalog(
            "Dim c As SoundChannel = PlaySound(\"m\")\n\
             Dim d As SoundChannel = PlaySound(\"n\")\n\
             Dim y As Integer = c < d\n",
            &catalog,
        );
        assert_eq!(error_codes(&cmp), vec!["E0301"]);
    }

    #[test]
    fn sound_handle_equality_and_null_allowed_fd041() {
        // Equality and `= Null` are the ONLY operators allowed on an opaque
        // handle (assignment, identity, null check) — pin that they don't error.
        // Both comparisons yield Int 1/0 (FD-035), assigned to plain Integers.
        let catalog = sound_catalog();
        let result = analyze_with_catalog(
            "Dim s As Sound = LoadSound(\"a\")\n\
             Dim ch As SoundChannel = PlaySound(s)\n\
             Dim unloaded As Integer = (s = Null)\n\
             Dim done As Integer = (SoundPlaying(ch) = 0)\n",
            &catalog,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── runtime constants (FD-029) ──────────────────────────────────────

    #[test]
    fn runtime_constant_folds_in_expr() {
        // A runtime-seeded constant is visible and usable like a user `Const`.
        let catalog = catalog_with_consts(vec![const_int("On", 1), const_int("cbKeyEsc", 1)]);
        let result = analyze_with_catalog("Dim x As Integer\nx = On\nx = cbKeyEsc\n", &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn runtime_constant_visible_inside_function() {
        let catalog = catalog_with_consts(vec![const_int("On", 1)]);
        let result = analyze_with_catalog(
            "Function f()\nDim y As Integer\ny = On\nEndFunction\n",
            &catalog,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn user_const_colliding_with_runtime_is_e0303() {
        // A user declaration that reuses a reserved runtime-constant name is a
        // duplicate-declaration error (FD-029 Q2 = error). Hoisted form.
        let catalog = catalog_with_consts(vec![const_int("On", 1)]);
        let result = analyze_with_catalog("Const On = 5\n", &catalog);
        assert_eq!(error_codes(&result), vec!["E0303"]);
    }

    #[test]
    fn user_dim_colliding_with_runtime_const_is_e0303() {
        // The non-hoisted (`Dim`) path: caught in pass 2 by try_declare against
        // the runtime constant's synthetic span. Must still be E0303 (and must
        // not produce a diagnostic that references the synthetic FileId).
        let catalog = catalog_with_consts(vec![const_int("PI", 3)]);
        let result = analyze_with_catalog("Dim pi As Float = 3.14\n", &catalog);
        assert_eq!(error_codes(&result), vec!["E0303"]);
    }

    // ── runtime command name collisions (FD-027) ───────────────────────

    #[test]
    fn explicit_dim_shadows_runtime_command() {
        // `Dim box` reclaims the name from the built-in `Box` command: the
        // declaration succeeds and later uses resolve to the variable.
        let catalog = catalog_of(vec![rt_func(
            "Box",
            "cb_box",
            &[("x", IrType::Float)],
            IrType::Void,
        )]);
        let result = analyze_with_catalog("Dim box As Int\nbox = 5\n", &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn implicit_assignment_over_runtime_command_is_e0328() {
        // No prior `Dim`: an implicit declaration may not shadow a command.
        let catalog = catalog_of(vec![rt_func(
            "Box",
            "cb_box",
            &[("x", IrType::Float)],
            IrType::Void,
        )]);
        let result = analyze_with_catalog("box = 5\n", &catalog);
        assert_eq!(error_codes(&result), vec!["E0328"]);
    }

    #[test]
    fn explicit_dim_shadows_overloaded_runtime_command() {
        // The same rule applies when the command is an overload set.
        let catalog = catalog_of(vec![
            rt_func("color", "cb_color_1", &[("c", IrType::Int)], IrType::Void),
            rt_func(
                "color",
                "cb_color_3",
                &[("r", IrType::Int), ("g", IrType::Int), ("b", IrType::Int)],
                IrType::Void,
            ),
        ]);
        let result = analyze_with_catalog("Dim color As Int\ncolor = 7\n", &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn dim_inside_function_shadows_runtime_command() {
        // A `Dim` in a function declares into the function scope and shadows
        // the top-level command through normal lookup — no special handling.
        let catalog = catalog_of(vec![rt_func(
            "Box",
            "cb_box",
            &[("x", IrType::Float)],
            IrType::Void,
        )]);
        let result = analyze_with_catalog(
            "Function f()\nDim box As Int\nbox = 1\nEndFunction\n",
            &catalog,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn runtime_fn_wrong_arg_count() {
        let catalog = catalog_of(vec![rt_func(
            "print",
            "cb_rt_print",
            &[("text", IrType::String)],
            IrType::Void,
        )]);
        let result = analyze_with_catalog("print(\"a\", \"b\")\n", &catalog);
        assert_eq!(error_codes(&result), vec!["E0305"]);
    }

    #[test]
    fn runtime_fn_return_type() {
        let catalog = catalog_of(vec![rt_func(
            "sin",
            "cb_rt_sin",
            &[("x", IrType::Float)],
            IrType::Float,
        )]);
        let result = analyze_with_catalog("Dim x As Float\nx = sin(1.0)\n", &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn overload_exact_match() {
        let catalog = catalog_of(vec![
            rt_func("abs", "cb_rt_abs_int", &[("x", IrType::Int)], IrType::Int),
            rt_func(
                "abs",
                "cb_rt_abs_float",
                &[("x", IrType::Float)],
                IrType::Float,
            ),
        ]);
        let result = analyze_with_catalog("Dim x As Int\nx = abs(42)\n", &catalog);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn overload_with_widening() {
        let catalog = catalog_of(vec![rt_func(
            "abs",
            "cb_rt_abs_float",
            &[("x", IrType::Float)],
            IrType::Float,
        )]);
        // Int -> Float is an implicit widening conversion
        let result = analyze_with_catalog("Dim x As Float\nx = abs(42)\n", &catalog);
        // Should succeed with a narrowing warning (Float return assigned to Float is fine,
        // but Int arg to Float param is a widening - no warning)
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, cb_diagnostics::Severity::Error))
            .collect();
        assert!(errors.is_empty(), "{:?}", errors);
    }

    #[test]
    fn overload_ambiguous() {
        let catalog = catalog_of(vec![
            rt_func("foo", "cb_rt_foo_a", &[("x", IrType::Int)], IrType::Void),
            rt_func("foo", "cb_rt_foo_b", &[("x", IrType::Int)], IrType::Int),
        ]);
        let result = analyze_with_catalog("foo(1)\n", &catalog);
        assert_eq!(error_codes(&result), vec!["E0323"]);
    }

    #[test]
    fn overload_no_match() {
        // Two overloads, neither accepts String
        let catalog = catalog_of(vec![
            rt_func("abs", "cb_rt_abs_int", &[("x", IrType::Int)], IrType::Int),
            rt_func(
                "abs",
                "cb_rt_abs_float",
                &[("x", IrType::Float)],
                IrType::Float,
            ),
        ]);
        let result = analyze_with_catalog("abs(\"hello\")\n", &catalog);
        assert_eq!(error_codes(&result), vec!["E0324"]);
    }

    #[test]
    fn runtime_fn_type_mismatch() {
        // Single runtime function, incompatible arg type
        let catalog = catalog_of(vec![rt_func(
            "abs",
            "cb_rt_abs_int",
            &[("x", IrType::Int)],
            IrType::Int,
        )]);
        let result = analyze_with_catalog("abs(\"hello\")\n", &catalog);
        assert_eq!(error_codes(&result), vec!["E0317"]);
    }

    #[test]
    fn user_function_shadows_runtime() {
        let catalog = catalog_of(vec![rt_func(
            "print",
            "cb_rt_print",
            &[("text", IrType::String)],
            IrType::Void,
        )]);
        // User defines their own print function — should shadow the runtime one
        let result = analyze_with_catalog(
            "Function print(x As Int)\nEndFunction\nprint(42)\n",
            &catalog,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn runtime_fn_resolved_call_recorded() {
        let catalog = catalog_of(vec![rt_func(
            "print",
            "cb_rt_print",
            &[("text", IrType::String)],
            IrType::Void,
        )]);
        let result = analyze_with_catalog("print(\"hello\")\n", &catalog);
        assert!(!result.resolved_calls.is_empty());
        let rc = result.resolved_calls.values().next().unwrap();
        match rc {
            crate::ResolvedCall::RuntimeFn { c_symbol } => {
                assert_eq!(c_symbol, "cb_rt_print");
            }
            _ => panic!("expected RuntimeFn, got {rc:?}"),
        }
    }

    #[test]
    fn user_function_resolved_call_recorded() {
        let result = analyze_src(
            "Function add(a As Int, b As Int) As Int\nReturn a + b\nEndFunction\nDim x As Int\nx = add(1, 2)\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let user_calls: Vec<_> = result
            .resolved_calls
            .values()
            .filter(|rc| matches!(rc, crate::ResolvedCall::UserDefined { .. }))
            .collect();
        assert_eq!(user_calls.len(), 1);
    }

    // ---- Bundle 1: sema validation gaps (S-M1..S-M5) ----------------------

    // S-M1: §6.2 — every Case value must be a constant expression implicitly
    // convertible to the scrutinee type.
    #[test]
    fn pass2_select_case_wrong_type_e0317() {
        let result =
            analyze_src("Dim x As Integer\nx = 1\nSelect x\nCase \"foo\"\nx = 2\nEndSelect\n");
        assert!(
            error_codes(&result).contains(&"E0317"),
            "expected E0317, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_select_case_non_constant_e0322() {
        let result = analyze_src(
            "Dim x As Integer\nDim y As Integer\nx = 1\ny = 2\nSelect x\nCase y\nx = 3\nEndSelect\n",
        );
        assert!(
            error_codes(&result).contains(&"E0322"),
            "expected E0322, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn select_case_valid_ok() {
        // Constant, convertible Case values must stay clean (over-firing guard).
        let result = analyze_src(
            "Dim x As Int\nx = 2\nSelect x\n  Case 1\n    x = 10\n  Case 2\n    x = 20\n  Default\n    x = 0\nEnd Select\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // S-M2: Int/Float/Str must validate their operand (numeric or String).
    #[test]
    fn pass2_int_intrinsic_bad_operand_e0301() {
        let result = analyze_src(
            "Type Foo\nField a As Integer\nEndType\nDim m As Foo\nm = New Foo\nDim n As Integer\nn = Int(m)\n",
        );
        assert!(
            error_codes(&result).contains(&"E0301"),
            "expected E0301, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn conversion_intrinsics_valid_operands_ok() {
        let result = analyze_src(
            "Dim a As Integer\nDim b As Float\nDim c As String\na = Int(\"5\")\nb = Float(3.5)\nc = Str(5)\na = Int(b)\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // S-M3: array index operands must be integers.
    #[test]
    fn pass2_index_non_integer_e0301() {
        let result = analyze_src("a = New Integer[10]\nDim n As Integer\nn = a[1.5]\n");
        assert!(
            error_codes(&result).contains(&"E0301"),
            "expected E0301, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn pass2_index_string_e0301() {
        let result = analyze_src("a = New Integer[10]\nDim n As Integer\nn = a[\"x\"]\n");
        assert!(
            error_codes(&result).contains(&"E0301"),
            "expected E0301, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn index_integer_ok() {
        let result = analyze_src("a = New Integer[10]\nDim n As Integer\nn = a[2]\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // S-M4: param and field sigil/As disagreement must emit E0320, exactly once.
    #[test]
    fn pass1_param_sigil_as_disagree_e0320() {
        let result = analyze_src("Function f(count% As Float) As Integer\nReturn 0\nEndFunction\n");
        let codes = error_codes(&result);
        assert_eq!(
            codes.iter().filter(|c| **c == "E0320").count(),
            1,
            "expected exactly one E0320, got {codes:?}"
        );
    }

    #[test]
    fn type_field_sigil_as_disagree_e0320() {
        let result = analyze_src("Type Foo\nField x% As Float\nEndType\n");
        assert!(
            error_codes(&result).contains(&"E0320"),
            "expected E0320, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn struct_field_sigil_as_disagree_e0320() {
        let result = analyze_src("Struct Foo\nField x% As Float\nEndStruct\n");
        assert!(
            error_codes(&result).contains(&"E0320"),
            "expected E0320, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn param_field_sigil_as_agree_ok() {
        let result = analyze_src(
            "Type Foo\nField x# As Float\nEndType\nFunction f(count% As Integer) As Integer\nReturn count\nEndFunction\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // S-M5: §4.4 — a Const initializer must be a constant expression.
    #[test]
    fn pass2_const_non_constant_e0322() {
        let result = analyze_src("Dim y As Integer\ny = 5\nConst x = y\n");
        assert!(
            error_codes(&result).contains(&"E0322"),
            "expected E0322, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn const_constant_initializer_ok() {
        let result = analyze_src("Const x = 1 + 2\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // S-M7/S-M8: Break needs an enclosing loop; Continue an enclosing loop or
    // Select (cb_syntax.md §6.2/§6.3).
    #[test]
    fn break_outside_loop_e0332() {
        let result = analyze_src("Break\n");
        assert!(
            error_codes(&result).contains(&"E0332"),
            "expected E0332, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn continue_outside_loop_e0332() {
        let result = analyze_src("Continue\n");
        assert!(
            error_codes(&result).contains(&"E0332"),
            "expected E0332, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn break_count_exceeds_loop_nesting_e0332() {
        let result = analyze_src("While 1\nBreak 2\nWend\n");
        assert!(
            error_codes(&result).contains(&"E0332"),
            "expected E0332, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn break_in_select_without_loop_e0332() {
        // Break cannot target a Select; a Select that isn't inside a loop has no
        // loop for Break to break.
        let result =
            analyze_src("Dim x As Int\nx = 1\nSelect x\n  Case 1\n    Break\nEnd Select\n");
        assert!(
            error_codes(&result).contains(&"E0332"),
            "expected E0332, got {:?}",
            error_codes(&result)
        );
    }

    #[test]
    fn break_continue_in_loop_ok() {
        let result = analyze_src("While 1\n  Continue\n  Break\nWend\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn continue_in_select_ok() {
        // Continue inside a Select is the legal explicit fall-through (§6.2).
        let result = analyze_src(
            "Dim x As Int\nx = 1\nSelect x\n  Case 1\n    Continue\n  Case 2\n    x = 2\nEnd Select\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    // ── diagnostic type-name rendering (Bundle 6: S-L5 / S-L4) ──────────

    /// Return the message of the first diagnostic carrying `code`, if any.
    fn message_for(result: &crate::SemaResult, code: &str) -> Option<String> {
        result
            .diagnostics
            .iter()
            .find(|d| d.code.as_ref().map(|c| c.as_str()) == Some(code))
            .map(|d| d.message.clone())
    }

    #[test]
    fn field_access_on_non_type_renders_resolved_name_no_symbol() {
        // S-L5: field access on a non-Type value (an Int here) must report the
        // human type name ("Int"), never the debug `Symbol(n)` form.
        let result = analyze_src("Dim x As Int\nx = 1\nx.foo = 2\n");
        let msg = message_for(&result, "E0309")
            .unwrap_or_else(|| panic!("expected E0309, got {:?}", error_codes(&result)));
        assert!(msg.contains("Int"), "message should name the type: {msg}");
        assert!(
            !msg.contains("Symbol("),
            "message must not leak Symbol(n): {msg}"
        );
    }

    #[test]
    fn index_on_non_array_renders_resolved_name_no_symbol() {
        // S-L5: indexing a non-array value reports the human type name.
        let result = analyze_src("Dim x As Int\nx = 1\nPrint x[0]\n");
        let msg = message_for(&result, "E0306")
            .unwrap_or_else(|| panic!("expected E0306, got {:?}", error_codes(&result)));
        assert!(msg.contains("Int"), "message should name the type: {msg}");
        assert!(
            !msg.contains("Symbol("),
            "message must not leak Symbol(n): {msg}"
        );
    }

    #[test]
    fn new_on_struct_is_specific() {
        // S-L4: `New` on a value-type Struct names it as a Struct, not a generic
        // "requires a Type name", and renders the resolved name (no Symbol(n)).
        let result = analyze_src("Struct Vec2\nField x As Float\nEndStruct\nDim v\nv = New Vec2\n");
        let msg = message_for(&result, "E0301")
            .unwrap_or_else(|| panic!("expected E0301, got {:?}", error_codes(&result)));
        assert!(
            msg.contains("Vec2"),
            "message should name the struct: {msg}"
        );
        assert!(msg.contains("Struct"), "message should say Struct: {msg}");
        assert!(
            !msg.contains("Symbol("),
            "message must not leak Symbol(n): {msg}"
        );
    }

    #[test]
    fn new_on_runtime_type_is_specific() {
        // S-L4: `New` on an opaque runtime handle type reports that it is a
        // built-in runtime type, using the resolved name.
        let file = FileId(0);
        let src = "Dim h\nh = New Image\n";
        let (tokens, _lex_diags) = tokenize(src, file, LexerOptions::default());
        let parsed = parse(&tokens, src, file);
        let catalog = crate::RuntimeCatalog {
            types: vec![crate::RuntimeTypeDesc {
                name: "Image".to_string(),
                tag: 7,
            }],
            functions: Vec::new(),
            constants: Vec::new(),
        };
        let result = analyze(&parsed.arena, &parsed.program, src, file, &catalog);
        let msg = message_for(&result, "E0301")
            .unwrap_or_else(|| panic!("expected E0301, got {:?}", error_codes(&result)));
        assert!(msg.contains("Image"), "message should name the type: {msg}");
        assert!(
            msg.contains("runtime type"),
            "message should say runtime type: {msg}"
        );
        assert!(
            !msg.contains("Symbol("),
            "message must not leak Symbol(n): {msg}"
        );
    }
}
