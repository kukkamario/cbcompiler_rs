//! Main analysis engine — declaration collection (pass 1) and type checking (pass 2).

use cb_diagnostics::{Diagnostic, FileId, Interner, Label, Span, Symbol};
use cb_frontend::ast::{Node, Param, Stmt};
use cb_frontend::{Arena, NodeId, SpanExt};

use crate::convert::ConversionTable;
use crate::diagnostics::*;
use crate::scope::{
    ConstValue, DeclKind, Declaration, FieldInfo, ParamInfo, ScopeId, ScopeKind, SymbolTable,
};
use crate::types::{self, Type};
use crate::{SemaResult, TypeTable};

/// Drives semantic analysis over a parsed AST.
pub(crate) struct Checker<'a> {
    arena: &'a Arena,
    source: &'a str,
    #[allow(dead_code)]
    file_id: FileId,
    interner: Interner,
    symbols: SymbolTable,
    types: TypeTable,
    conversions: ConversionTable,
    diagnostics: Vec<Diagnostic>,
    current_scope: ScopeId,
}

impl<'a> Checker<'a> {
    pub(crate) fn run(
        arena: &'a Arena,
        program: &[NodeId],
        source: &'a str,
        file_id: FileId,
    ) -> SemaResult {
        let mut symbols = SymbolTable::new();
        let top = symbols.push_scope(ScopeKind::TopLevel, None);

        let mut checker = Checker {
            arena,
            source,
            file_id,
            interner: Interner::new(),
            symbols,
            types: TypeTable::new(),
            conversions: ConversionTable::new(),
            diagnostics: Vec::new(),
            current_scope: top,
        };

        checker.pass1(program);
        // Pass 2 will be implemented in M3.

        SemaResult {
            types: checker.types,
            symbols: checker.symbols,
            conversions: checker.conversions,
            diagnostics: checker.diagnostics,
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// Intern a name from a span. The span must NOT include a sigil byte.
    fn intern_span(&mut self, span: Span) -> Symbol {
        let text = span.slice(self.source);
        self.interner.intern(text)
    }

    /// Intern an identifier name, stripping the trailing sigil character if present.
    fn intern_ident(&mut self, name_span: Span, sigil: Option<cb_frontend::Sigil>) -> Symbol {
        let raw = name_span.slice(self.source);
        let bare = if sigil.is_some() {
            &raw[..raw.len() - 1]
        } else {
            raw
        };
        self.interner.intern(bare)
    }

    /// Resolve a `TypeExpr` node to a semantic `Type`.
    fn resolve_type_expr(&mut self, id: NodeId) -> Type {
        types::resolve_type_expr(self.arena, id, &mut self.interner, self.source)
    }

    /// Try to declare a name in the given scope, emitting a diagnostic on duplicate.
    fn try_declare(&mut self, scope: ScopeId, name: Symbol, decl: Declaration, error_code: cb_diagnostics::DiagnosticCode) {
        let decl_span = decl.span;
        if let Err(prev_span) = self.symbols.declare(scope, name, decl) {
            let name_str = self.interner.resolve(name);
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

    // ── pass 1: declaration collection (hoisting) ───────────────────────

    fn pass1(&mut self, program: &[NodeId]) {
        let top = self.current_scope;
        for &id in program {
            self.pass1_stmt(id, top);
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
            Node::Stmt(Stmt::Type { name_span, fields }) => {
                self.pass1_type_def(scope, name_span, &fields, span);
            }
            Node::Stmt(Stmt::Struct { name_span, fields }) => {
                self.pass1_struct_def(scope, name_span, &fields, span);
            }
            Node::Stmt(Stmt::Global { names, ty, init: _ }) => {
                self.pass1_global(scope, &names, ty);
            }
            Node::Stmt(Stmt::Label { name_span }) => {
                self.pass1_label(scope, name_span);
            }
            Node::Stmt(Stmt::Const {
                name_span,
                sigil,
                ty,
                value: _,
                is_global,
            }) => {
                self.pass1_const(scope, name_span, sigil, ty, is_global);
            }
            _ => {}
        }
    }

    fn pass1_function(
        &mut self,
        scope: ScopeId,
        name_span: Span,
        return_sigil: Option<cb_frontend::Sigil>,
        params: &[NodeId],
        return_ty_node: Option<NodeId>,
        _full_span: Span,
    ) {
        let name = self.intern_span(name_span);

        // Resolve parameters.
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
                let (pty, _disagree) = types::resolve_var_type(*sigil, as_ty.as_ref());
                param_infos.push(ParamInfo {
                    name: pname_sym,
                    ty: pty,
                    has_default: default.is_some(),
                });
            }
        }

        // Resolve return type.
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
            },
            ty: fn_type,
            span: name_span,
            is_global: false,
        };

        self.try_declare(scope, name, decl, E_DUPLICATE_DEFINITION);
    }

    fn pass1_type_def(
        &mut self,
        scope: ScopeId,
        name_span: Span,
        fields: &[NodeId],
        _full_span: Span,
    ) {
        let name = self.intern_span(name_span);

        let mut field_infos = Vec::with_capacity(fields.len());
        for &fid in fields {
            if let Node::Stmt(Stmt::FieldDecl {
                name_span: fname_span,
                sigil,
                ty,
            }) = &self.arena[fid]
            {
                let fname = self.intern_ident(*fname_span, *sigil);
                let as_ty = ty.map(|tid| self.resolve_type_expr(tid));
                let (fty, _disagree) = types::resolve_var_type(*sigil, as_ty.as_ref());
                field_infos.push(FieldInfo {
                    name: fname,
                    ty: fty,
                    span: *fname_span,
                });
            }
        }

        let decl = Declaration {
            kind: DeclKind::TypeDef {
                fields: field_infos,
            },
            ty: Type::TypeRef { name },
            span: name_span,
            is_global: false,
        };

        self.try_declare(scope, name, decl, E_DUPLICATE_DEFINITION);
    }

    fn pass1_struct_def(
        &mut self,
        scope: ScopeId,
        name_span: Span,
        fields: &[NodeId],
        _full_span: Span,
    ) {
        let name = self.intern_span(name_span);

        let mut field_infos = Vec::with_capacity(fields.len());
        for &fid in fields {
            if let Node::Stmt(Stmt::FieldDecl {
                name_span: fname_span,
                sigil,
                ty,
            }) = &self.arena[fid]
            {
                let fname = self.intern_ident(*fname_span, *sigil);
                let as_ty = ty.map(|tid| self.resolve_type_expr(tid));
                let (fty, _disagree) = types::resolve_var_type(*sigil, as_ty.as_ref());
                field_infos.push(FieldInfo {
                    name: fname,
                    ty: fty,
                    span: *fname_span,
                });
            }
        }

        let decl = Declaration {
            kind: DeclKind::StructDef {
                fields: field_infos,
            },
            ty: Type::StructVal { name },
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
        sigil: Option<cb_frontend::Sigil>,
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

        // Placeholder value — const expression evaluation happens in M4.
        let placeholder = match &const_ty {
            Type::Int => ConstValue::Int(0),
            Type::Float => ConstValue::Float(0.0),
            Type::Bool => ConstValue::Bool(false),
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
}

#[cfg(test)]
mod tests {
    use cb_diagnostics::FileId;
    use cb_frontend::{tokenize, parse, LexerOptions};

    use crate::analyze;

    fn analyze_src(src: &str) -> crate::SemaResult {
        let file = FileId(0);
        let (tokens, _lex_diags) = tokenize(src, file, LexerOptions::default());
        let parsed = parse(&tokens, src, file);
        analyze(&parsed.arena, &parsed.program, src, file)
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
        let result = analyze_src("Function step(distance As Float, count = 1) As Float\nReturn distance\nEndFunction\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass1_non_hoisted_stmts_ignored() {
        // Dim, Assign, If, etc. are not collected in pass 1
        let result = analyze_src("Dim x As Integer\nx = 42\nIf True Then\nx = 1\nEndIf\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }
}
