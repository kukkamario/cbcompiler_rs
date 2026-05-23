//! Main analysis engine — declaration collection (pass 1) and type checking (pass 2).

use cb_diagnostics::{Diagnostic, FileId, Interner, Label, Span, Symbol};
use cb_frontend::ast::{CaseArm, Expr, Node, Param, Stmt};
use cb_frontend::{Arena, BinOp, NodeId, Sigil, SpanExt, UnOp};

use crate::convert::ConversionTable;
use crate::diagnostics::*;
use crate::scope::{
    ConstValue, DeclKind, Declaration, FieldInfo, ParamInfo, ScopeId, ScopeKind, SymbolTable,
};
use crate::types::{self, Type};
use crate::{SemaResult, TypeTable};

// Names of compiler intrinsics (lowercase, matching interner output).
const INTRINSIC_LEN: &str = "len";
const INTRINSIC_INT: &str = "int";
const INTRINSIC_INTEGER: &str = "integer";
const INTRINSIC_FLOAT: &str = "float";
const INTRINSIC_STR: &str = "str";
const INTRINSIC_BOOL: &str = "bool";
// Type-list intrinsics are parsed as keywords and have dedicated AST nodes,
// but First/Last/Next/Previous are called like regular functions when the user
// writes them in expression position. We don't handle those yet — they'll be
// recognized once the runtime/intrinsic call infrastructure is in place.

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
    /// The return type of the function we're currently inside, if any.
    current_fn_return_ty: Option<Type>,
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
            current_fn_return_ty: None,
        };

        checker.pass1(program);
        checker.pass2(program);

        SemaResult {
            types: checker.types,
            symbols: checker.symbols,
            conversions: checker.conversions,
            diagnostics: checker.diagnostics,
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    fn intern_span(&mut self, span: Span) -> Symbol {
        let text = span.slice(self.source);
        self.interner.intern(text)
    }

    fn intern_ident(&mut self, name_span: Span, sigil: Option<Sigil>) -> Symbol {
        let raw = name_span.slice(self.source);
        let bare = if sigil.is_some() {
            &raw[..raw.len() - 1]
        } else {
            raw
        };
        self.interner.intern(bare)
    }

    fn resolve_type_expr(&mut self, id: NodeId) -> Type {
        types::resolve_type_expr(self.arena, id, &mut self.interner, self.source)
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
                let (pty, _disagree) = types::resolve_var_type(*sigil, as_ty.as_ref());
                param_infos.push(ParamInfo {
                    name: pname_sym,
                    ty: pty,
                    has_default: default.is_some(),
                });
            }
        }
        let as_ret = return_ty_node.map(|tid| self.resolve_type_expr(tid));
        let (ret_ty, sigil_as_disagree) =
            types::resolve_return_type(return_sigil, as_ret.as_ref());
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
            Node::Expr(Expr::BoolLit(_)) => Type::Bool,
            Node::Expr(Expr::StrLit { .. }) => Type::String,
            Node::Expr(Expr::NullLit) => Type::Null,

            Node::Expr(Expr::Ident { name_span, sigil }) => {
                self.check_ident(name_span, sigil, false)
            }

            Node::Expr(Expr::Binary { op, lhs, rhs }) => {
                self.check_binary(op, lhs, rhs, span)
            }

            Node::Expr(Expr::Unary { op, operand }) => {
                self.check_unary(op, operand, span)
            }

            Node::Expr(Expr::Call { callee, args }) => {
                self.check_call(callee, &args, span)
            }

            Node::Expr(Expr::Index { array, indices }) => {
                self.check_index(array, &indices, span)
            }

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
            // Sigil enforcement: if this use has a sigil, it must match the declaration type.
            if let Some(s) = sigil {
                let sigil_ty = types::sigil_to_type(s);
                if sigil_ty != decl_ty && !decl_ty.is_error() {
                    self.diagnostics.push(Diagnostic::error(
                        E_SIGIL_CONFLICT,
                        format!(
                            "sigil `{}` conflicts with declared type",
                            sigil_char(s),
                        ),
                        Label::new(name_span),
                    ));
                }
            }
            decl_ty
        } else {
            // Undeclared — will be handled as implicit declaration when
            // encountered as assignment target in check_stmt. When encountered
            // as a read, this is an error.
            self.diagnostics.push(Diagnostic::error(
                E_UNDECLARED_IDENT,
                format!(
                    "undeclared identifier `{}`",
                    self.interner.resolve(name)
                ),
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
        types::binary_result_type(op, &lty, &rty).unwrap_or_else(|| {
            self.diagnostics.push(Diagnostic::error(
                E_TYPE_MISMATCH,
                format!(
                    "operator `{:?}` cannot be applied to `{:?}` and `{:?}`",
                    op, lty, rty
                ),
                Label::new(span),
            ));
            Type::Error
        })
    }

    fn check_unary(&mut self, op: UnOp, operand: NodeId, span: Span) -> Type {
        let oty = self.check_expr(operand);
        if oty.is_error() {
            return Type::Error;
        }
        types::unary_result_type(op, &oty).unwrap_or_else(|| {
            self.diagnostics.push(Diagnostic::error(
                E_TYPE_MISMATCH,
                format!("operator `{op:?}` cannot be applied to `{oty:?}`"),
                Label::new(span),
            ));
            Type::Error
        })
    }

    fn check_call(&mut self, callee: NodeId, args: &[NodeId], span: Span) -> Type {
        // Check if callee is an identifier that names an intrinsic.
        if let Node::Expr(Expr::Ident { name_span, sigil: None }) = &self.arena[callee] {
            let name = self.intern_ident(*name_span, None);
            let name_str = self.interner.resolve(name).to_owned();

            if let Some(ty) = self.check_intrinsic_call(&name_str, args, span) {
                return ty;
            }
        }

        // Regular function call.
        let callee_ty = self.check_expr(callee);
        if callee_ty.is_error() {
            for &a in args {
                self.check_expr(a);
            }
            return Type::Error;
        }

        // Check arg expressions regardless.
        let arg_types: Vec<Type> = args.iter().map(|&a| self.check_expr(a)).collect();

        // Look up the callee in the scope if it's an ident.
        if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[callee] {
            let name = self.intern_ident(*name_span, *sigil);
            if let Some(decl) = self.symbols.lookup(self.current_scope, name) {
                if let DeclKind::Function { params, return_ty } = &decl.kind {
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
                    }
                    return return_ty.clone();
                }
            }
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
        self.diagnostics.push(Diagnostic::error(
            E_CALL_NON_FUNCTION,
            format!("cannot call value of type `{callee_ty:?}`"),
            Label::new(span),
        ));
        Type::Error
    }

    fn check_intrinsic_call(
        &mut self,
        name: &str,
        args: &[NodeId],
        span: Span,
    ) -> Option<Type> {
        match name {
            INTRINSIC_LEN => {
                if args.is_empty() || args.len() > 2 {
                    self.diagnostics.push(Diagnostic::error(
                        E_WRONG_ARG_COUNT,
                        format!("Len expects 1 or 2 arguments, got {}", args.len()),
                        Label::new(span),
                    ));
                    return Some(Type::Error);
                }
                let arr_ty = self.check_expr(args[0]);
                if !matches!(arr_ty, Type::Array { .. }) && !arr_ty.is_error() {
                    self.diagnostics.push(Diagnostic::error(
                        E_TYPE_MISMATCH,
                        "first argument to Len must be an array",
                        Label::new(self.arena.span_of(args[0])),
                    ));
                }
                if args.len() == 2 {
                    let dim_ty = self.check_expr(args[1]);
                    if !dim_ty.is_integer() && !dim_ty.is_error() {
                        self.diagnostics.push(Diagnostic::error(
                            E_TYPE_MISMATCH,
                            "second argument to Len must be an integer",
                            Label::new(self.arena.span_of(args[1])),
                        ));
                    }
                }
                Some(Type::Int)
            }
            INTRINSIC_INT | INTRINSIC_INTEGER => {
                self.check_conversion_intrinsic(args, span, Type::Int)
            }
            INTRINSIC_FLOAT => self.check_conversion_intrinsic(args, span, Type::Float),
            INTRINSIC_STR => self.check_conversion_intrinsic(args, span, Type::String),
            INTRINSIC_BOOL => self.check_conversion_intrinsic(args, span, Type::Bool),
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
                format!("conversion intrinsic expects 1 argument, got {}", args.len()),
                Label::new(span),
            ));
            return Some(Type::Error);
        }
        self.check_expr(args[0]);
        Some(target)
    }

    fn check_index(&mut self, array: NodeId, indices: &[NodeId], span: Span) -> Type {
        let arr_ty = self.check_expr(array);
        let _idx_types: Vec<Type> = indices.iter().map(|&i| self.check_expr(i)).collect();

        if arr_ty.is_error() {
            return Type::Error;
        }

        if let Type::Array { elem, rank } = &arr_ty {
            if indices.len() != *rank as usize {
                self.diagnostics.push(Diagnostic::error(
                    E_RANK_MISMATCH,
                    format!(
                        "array has {} dimension(s), but {} index/indices provided",
                        rank,
                        indices.len()
                    ),
                    Label::new(span),
                ));
            }
            *elem.clone()
        } else {
            self.diagnostics.push(Diagnostic::error(
                E_INDEX_NON_ARRAY,
                format!("cannot index value of type `{arr_ty:?}`"),
                Label::new(span),
            ));
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
            Type::TypeRef { name } | Type::StructVal { name } => {
                self.symbols
                    .lookup(self.current_scope, *name)
                    .and_then(|decl| match &decl.kind {
                        DeclKind::TypeDef { fields } | DeclKind::StructDef { fields } => {
                            Some(fields.clone())
                        }
                        _ => None,
                    })
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    E_FIELD_ON_NON_TYPE,
                    format!("cannot access fields on `{target_ty:?}`"),
                    Label::new(span),
                ));
                return Type::Error;
            }
        };

        if let Some(fields) = fields {
            for f in &fields {
                if f.name == field_name {
                    return f.ty.clone();
                }
            }
            self.diagnostics.push(Diagnostic::error(
                E_NO_SUCH_FIELD,
                format!(
                    "no field `{}` on type",
                    self.interner.resolve(field_name)
                ),
                Label::new(name_span),
            ));
            Type::Error
        } else {
            Type::Error
        }
    }

    fn check_new(&mut self, kind: &cb_frontend::NewKind, span: Span) -> Type {
        match kind {
            cb_frontend::NewKind::Type(type_expr_id) => {
                let ty = self.resolve_type_expr(*type_expr_id);
                if let Type::TypeRef { .. } = &ty {
                    ty
                } else if ty.is_error() {
                    Type::Error
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        E_TYPE_MISMATCH,
                        "New requires a Type name",
                        Label::new(span),
                    ));
                    Type::Error
                }
            }
            cb_frontend::NewKind::Array { elem, dims } => {
                let elem_ty = self.resolve_type_expr(*elem);
                for &d in dims {
                    let dty = self.check_expr(d);
                    if !dty.is_integer() && !dty.is_error() {
                        self.diagnostics.push(Diagnostic::error(
                            E_TYPE_MISMATCH,
                            "array dimension must be an integer",
                            Label::new(self.arena.span_of(d)),
                        ));
                    }
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
                self.check_expr(expr);
            }
            Node::Stmt(Stmt::Dim { names, ty, init }) => {
                self.check_dim(&names, ty, init);
            }
            Node::Stmt(Stmt::Global { names: _, ty: _, init }) => {
                // Globals are already hoisted in pass 1; just check the initializer.
                if let Some(init_id) = init {
                    self.check_expr(init_id);
                }
            }
            Node::Stmt(Stmt::Const { value, .. }) => {
                // Const is already hoisted; check the value expression.
                self.check_expr(value);
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
                for &s in &body {
                    self.check_stmt(s);
                }
            }
            Node::Stmt(Stmt::RepeatForever { body }) => {
                for &s in &body {
                    self.check_stmt(s);
                }
            }
            Node::Stmt(Stmt::RepeatWhile { body, cond }) => {
                for &s in &body {
                    self.check_stmt(s);
                }
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
                self.check_for(var, from, to, step, &body);
            }
            Node::Stmt(Stmt::ForEach {
                var, source, body, ..
            }) => {
                self.check_for_each(var, source, &body);
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
            Node::Stmt(Stmt::Goto { label_span }) => {
                self.check_goto(label_span);
            }
            Node::Stmt(Stmt::Delete { operand }) => {
                self.check_delete(operand);
            }
            Node::Stmt(Stmt::Redim {
                target,
                elem_ty,
                dims,
            }) => {
                self.check_redim(target, elem_ty, &dims);
            }
            // Statements that are already handled or require no type checking:
            Node::Stmt(Stmt::Type { .. })
            | Node::Stmt(Stmt::Struct { .. })
            | Node::Stmt(Stmt::FieldDecl { .. })
            | Node::Stmt(Stmt::Label { .. })
            | Node::Stmt(Stmt::Break { .. })
            | Node::Stmt(Stmt::Continue)
            | Node::Stmt(Stmt::Include { .. })
            | Node::Stmt(Stmt::Error) => {}
            _ => {}
        }
    }

    fn check_assign(&mut self, target: NodeId, value: NodeId) {
        // If the target is an undeclared identifier, create an implicit declaration.
        let target_ty = if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[target] {
            let name = self.intern_ident(*name_span, *sigil);
            if self.symbols.lookup(self.current_scope, name).is_none() {
                // Implicit declaration.
                let (var_ty, _) = types::resolve_var_type(*sigil, None);
                let decl = Declaration {
                    kind: DeclKind::Variable,
                    ty: var_ty.clone(),
                    span: *name_span,
                    is_global: false,
                };
                self.try_declare(self.current_scope, name, decl, E_DUPLICATE_DECL);
                self.types.insert(target, var_ty.clone());
                var_ty
            } else {
                self.check_expr(target)
            }
        } else {
            self.check_expr(target)
        };

        let value_ty = self.check_expr(value);
        if !target_ty.is_error() && !value_ty.is_error() && target_ty != value_ty {
            // For now, just flag clear mismatches. M4 will add conversion logic.
            if !types::is_implicitly_convertible(&value_ty, &target_ty) {
                self.diagnostics.push(Diagnostic::error(
                    E_TYPE_MISMATCH,
                    format!(
                        "cannot assign `{value_ty:?}` to `{target_ty:?}`",
                    ),
                    Label::new(self.arena.span_of(value)),
                ));
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
            self.try_declare(self.current_scope, name, decl, E_DUPLICATE_DECL);
        }

        if let Some(init_id) = init {
            self.check_expr(init_id);
        }
    }

    fn check_condition(&mut self, cond: NodeId) {
        let cty = self.check_expr(cond);
        if !cty.is_error() && cty != Type::Bool && !cty.is_numeric() {
            self.diagnostics.push(Diagnostic::error(
                E_TYPE_MISMATCH,
                format!("condition must be Bool or numeric, got `{cty:?}`"),
                Label::new(self.arena.span_of(cond)),
            ));
        }
    }

    fn check_for(
        &mut self,
        var: NodeId,
        from: NodeId,
        to: NodeId,
        step: Option<NodeId>,
        body: &[NodeId],
    ) {
        // The loop variable may be an implicit declaration.
        let var_ty = if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[var] {
            let name = self.intern_ident(*name_span, *sigil);
            if self.symbols.lookup(self.current_scope, name).is_none() {
                let (vt, _) = types::resolve_var_type(*sigil, None);
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
        if let Some(step_id) = step {
            let step_ty = self.check_expr(step_id);
            if !step_ty.is_numeric() && !step_ty.is_error() {
                self.diagnostics.push(Diagnostic::error(
                    E_TYPE_MISMATCH,
                    "For `step` value must be numeric",
                    Label::new(self.arena.span_of(step_id)),
                ));
            }
        }

        for &s in body {
            self.check_stmt(s);
        }
    }

    fn check_for_each(&mut self, var: NodeId, source: NodeId, body: &[NodeId]) {
        // The iteration variable is implicitly declared.
        if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[var] {
            let name = self.intern_ident(*name_span, *sigil);
            if self.symbols.lookup(self.current_scope, name).is_none() {
                let (vt, _) = types::resolve_var_type(*sigil, None);
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

        // Source should be an array variable or a Type name.
        self.check_expr(source);

        for &s in body {
            self.check_stmt(s);
        }
    }

    fn check_select(&mut self, scrutinee: NodeId, arms: &[NodeId]) {
        self.check_expr(scrutinee);
        for &arm_id in arms {
            match &self.arena[arm_id] {
                Node::CaseArm(CaseArm::Case { values, body }) => {
                    let values = values.clone();
                    let body = body.clone();
                    for &v in &values {
                        self.check_expr(v);
                    }
                    for &s in &body {
                        self.check_stmt(s);
                    }
                }
                Node::CaseArm(CaseArm::Default { body }) => {
                    let body = body.clone();
                    for &s in &body {
                        self.check_stmt(s);
                    }
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

        // Also hoist labels inside this function body.
        for &s in body {
            if let Node::Stmt(Stmt::Label { name_span: lspan }) = &self.arena[s] {
                self.pass1_label(fn_scope, *lspan);
            }
        }

        // Check function body.
        for &s in body {
            self.check_stmt(s);
        }

        // Restore.
        let _name = self.intern_span(name_span);
        self.current_scope = prev_scope;
        self.current_fn_return_ty = prev_fn_ret;
    }

    fn check_return(&mut self, value: Option<NodeId>, span: Span) {
        match &self.current_fn_return_ty {
            None => {
                self.diagnostics.push(Diagnostic::error(
                    E_RETURN_OUTSIDE_FN,
                    "Return statement outside of a function",
                    Label::new(span),
                ));
            }
            Some(ret_ty) => {
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
                    self.check_expr(val_id);
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
                    format!(
                        "`{}` is not a label",
                        self.interner.resolve(name)
                    ),
                    Label::new(label_span),
                ));
            }
        } else {
            self.diagnostics.push(Diagnostic::error(
                E_UNDECLARED_LABEL,
                format!(
                    "undeclared label `{}`",
                    self.interner.resolve(name)
                ),
                Label::new(label_span),
            ));
        }
    }

    fn check_delete(&mut self, operand: NodeId) {
        let op_ty = self.check_expr(operand);
        if !op_ty.is_error() && !matches!(op_ty, Type::TypeRef { .. }) {
            self.diagnostics.push(Diagnostic::error(
                E_DELETE_NON_TYPEREF,
                format!("Delete requires a Type reference, got `{op_ty:?}`"),
                Label::new(self.arena.span_of(operand)),
            ));
        }
    }

    fn check_redim(&mut self, target: NodeId, elem_ty_node: NodeId, dims: &[NodeId]) {
        self.check_expr(target);
        self.resolve_type_expr(elem_ty_node);
        for &d in dims {
            let dty = self.check_expr(d);
            if !dty.is_integer() && !dty.is_error() {
                self.diagnostics.push(Diagnostic::error(
                    E_TYPE_MISMATCH,
                    "Redim dimension must be an integer",
                    Label::new(self.arena.span_of(d)),
                ));
            }
        }
    }
}

fn sigil_char(s: Sigil) -> char {
    match s {
        Sigil::Integer => '%',
        Sigil::Float => '#',
        Sigil::String => '$',
        Sigil::Bool => '!',
    }
}

#[cfg(test)]
mod tests {
    use cb_diagnostics::FileId;
    use cb_frontend::{parse, tokenize, LexerOptions};

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
        let result = analyze_src("Function move(distance As Float, count = 1) As Float\nReturn distance\nEndFunction\n");
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
    fn pass2_comparison_returns_bool() {
        let result = analyze_src("Dim x As Bool\nx = 1 > 2\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_logical_and_or() {
        let result = analyze_src("Dim x As Bool\nx = True And False\n");
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

    // ── pass 2 tests: field access ──────────────────────────────────────

    #[test]
    fn pass2_field_access_ok() {
        let result = analyze_src(
            "Type MyObj\nField x As Integer\nEndType\nDim obj As MyObj\nobj.x = 42\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_field_not_found_e0308() {
        let result = analyze_src(
            "Type MyObj\nField x As Integer\nEndType\nDim obj As MyObj\nobj.y = 42\n",
        );
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

    // ── pass 2 tests: scope visibility ──────────────────────────────────

    #[test]
    fn pass2_function_sees_global() {
        let result = analyze_src(
            "Global g As Integer\nFunction f() As Integer\nReturn g\nEndFunction\n",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn pass2_function_cannot_see_toplevel_var() {
        let result = analyze_src(
            "Dim x As Integer\nFunction f() As Integer\nReturn x\nEndFunction\n",
        );
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
}
