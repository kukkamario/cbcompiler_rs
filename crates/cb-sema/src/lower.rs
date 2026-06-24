//! AST→IR lowering pass.
//!
//! Consumes the typed AST (via [`SemaResult`]) and produces an [`ir::Program`]
//! with explicit basic blocks, branches, and type conversions.

use std::collections::HashMap;

use cb_diagnostics::{FileId, Interner, Span, Symbol};
use cb_frontend::ast::{CaseArm, Expr, Node, Stmt};
use cb_frontend::{Arena, BinOp, NewKind, NodeId, SpanExt, UnOp};

use cb_ir::inst::{InstKind, IrBinOp, IrUnOp, PlaceRoot, Projection, Terminator};
use cb_ir::types::{FnSig, IrType};
use cb_ir::{
    BasicBlock, BlockId, FuncDecl, FuncId, FuncKind, Function, Global, GlobalId, Inst, Local,
    LocalId, Program, Reg, StructDefInfo, TypeDefId, TypeDefInfo,
};

use crate::convert::ConversionTable;
use crate::scope::{ConstValue, DeclKind, ScopeId, SymbolTable};
use crate::types::Type;
use crate::{DeleteClass, ResolvedCall, SemaResult, TypeTable};

/// Lower a type-checked program to IR.
///
/// The `sema` result is taken mutably because the lowerer may intern new
/// symbols (e.g. the synthetic `@main` function name).
pub fn lower(arena: &Arena, program: &[NodeId], source: &str, sema: &mut SemaResult) -> Program {
    let SemaResult {
        types,
        symbols,
        conversions,
        delete_classes,
        resolved_calls,
        diagnostics: _,
        interner,
    } = sema;

    let mut lowerer = Lowerer {
        arena,
        source,
        interner,
        types,
        symbols,
        conversions,
        delete_classes,
        resolved_calls,

        current_scope: ScopeId(0),
        locals: Vec::new(),
        blocks: Vec::new(),
        current_block: BlockId(0),
        next_reg: 0,
        next_block_id: 0,
        local_map: HashMap::new(),
        context_stack: Vec::new(),
        label_blocks: HashMap::new(),
        next_temp: 0,

        func_table: Vec::new(),
        func_id_map: HashMap::new(),
        runtime_func_map: HashMap::new(),
        functions: Vec::new(),
        globals: Vec::new(),
        global_map: HashMap::new(),
        type_defs: Vec::new(),
        type_def_map: HashMap::new(),
        struct_defs: Vec::new(),
    };

    lowerer.lower_program(program)
}

// ── Control-flow context ────────────────────────────────────────────────

enum ControlContext {
    Loop {
        continue_block: BlockId,
        exit_block: BlockId,
    },
    Select {
        next_arm_body: Option<BlockId>,
    },
}

// ── Variable reference ──────────────────────────────────────────────────

#[derive(Copy, Clone)]
enum VarRef {
    Local(LocalId),
    Global(GlobalId),
}

// ── Lowerer ─────────────────────────────────────────────────────────────

struct Lowerer<'a> {
    arena: &'a Arena,
    source: &'a str,
    interner: &'a mut Interner,
    types: &'a TypeTable,
    symbols: &'a SymbolTable,
    conversions: &'a ConversionTable,
    delete_classes: &'a HashMap<NodeId, DeleteClass>,
    resolved_calls: &'a HashMap<NodeId, ResolvedCall>,

    // Per-function state (reset between functions)
    current_scope: ScopeId,
    locals: Vec<Local>,
    blocks: Vec<BasicBlock>,
    current_block: BlockId,
    next_reg: u32,
    next_block_id: u32,
    local_map: HashMap<Symbol, LocalId>,
    context_stack: Vec<ControlContext>,
    label_blocks: HashMap<Symbol, BlockId>,
    next_temp: u32,

    // Collected output (program-scoped)
    func_table: Vec<FuncDecl>,
    func_id_map: HashMap<Symbol, FuncId>,
    runtime_func_map: HashMap<String, FuncId>,
    functions: Vec<Function>,
    globals: Vec<Global>,
    global_map: HashMap<Symbol, GlobalId>,
    type_defs: Vec<TypeDefInfo>,
    type_def_map: HashMap<Symbol, TypeDefId>,
    struct_defs: Vec<StructDefInfo>,
}

impl<'a> Lowerer<'a> {
    // ── helpers ──────────────────────────────────────────────────────

    fn fresh_reg(&mut self) -> Reg {
        let r = Reg(self.next_reg);
        self.next_reg += 1;
        r
    }

    fn fresh_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        self.blocks.push(BasicBlock {
            id,
            insts: Vec::new(),
            terminator: None,
            terminator_span: Span::new(0, 0, FileId::SYNTHETIC),
        });
        id
    }

    fn block_mut(&mut self, id: BlockId) -> &mut BasicBlock {
        &mut self.blocks[id.0 as usize]
    }

    fn current_block_mut(&mut self) -> &mut BasicBlock {
        self.block_mut(self.current_block)
    }

    fn emit(&mut self, kind: InstKind, span: Span) -> Reg {
        let r = self.fresh_reg();
        self.current_block_mut().insts.push(Inst {
            result: Some(r),
            kind,
            span,
        });
        r
    }

    fn emit_void(&mut self, kind: InstKind, span: Span) {
        self.current_block_mut().insts.push(Inst {
            result: None,
            kind,
            span,
        });
    }

    fn terminate(&mut self, term: Terminator, span: Span) {
        let blk = self.current_block_mut();
        if blk.terminator.is_none() {
            blk.terminator = Some(term);
            blk.terminator_span = span;
        }
    }

    fn switch_to(&mut self, block: BlockId) {
        self.current_block = block;
    }

    fn alloc_local(&mut self, name: Symbol, ty: IrType, is_param: bool) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(Local { name, ty, is_param });
        self.local_map.insert(name, id);
        id
    }

    fn resolve_var(&self, name: Symbol) -> Option<VarRef> {
        if let Some(&g) = self.global_map.get(&name) {
            Some(VarRef::Global(g))
        } else {
            self.lookup_local(name).map(VarRef::Local)
        }
    }

    fn emit_load_var(&mut self, var: VarRef, span: Span) -> Reg {
        match var {
            VarRef::Local(id) => self.emit(InstKind::LoadLocal { local: id }, span),
            VarRef::Global(id) => self.emit(InstKind::LoadGlobal { global: id }, span),
        }
    }

    fn emit_store_var(&mut self, var: VarRef, value: Reg, span: Span) {
        match var {
            VarRef::Local(id) => {
                self.emit_void(InstKind::StoreLocal { local: id, value }, span);
            }
            VarRef::Global(id) => {
                self.emit_void(InstKind::StoreGlobal { global: id, value }, span);
            }
        }
    }

    fn lookup_local(&self, name: Symbol) -> Option<LocalId> {
        self.local_map.get(&name).copied()
    }

    fn alloc_temp(&mut self, prefix: &str, ty: IrType) -> LocalId {
        let n = self.next_temp;
        self.next_temp += 1;
        let name = self.interner.intern(&format!("{prefix}_{n}"));
        self.alloc_local(name, ty, false)
    }

    fn intern_span(&mut self, span: Span) -> Symbol {
        let text = span.slice(self.source);
        self.interner.intern(text)
    }

    fn intern_ident(&mut self, name_span: Span, _sigil: Option<cb_frontend::Sigil>) -> Symbol {
        // `name_span` is the bare-name span (parser strips the sigil byte). The
        // sigil never participates in variable identity, so lowering must key
        // locals by the bare name — matching the checker (cb_syntax.md §1.3–§1.4).
        self.interner.intern(name_span.slice(self.source))
    }

    fn sema_type_to_ir(&self, ty: &Type) -> IrType {
        match ty {
            Type::Byte => IrType::Byte,
            Type::Short => IrType::Short,
            Type::Int => IrType::Int,
            Type::Long => IrType::Long,
            Type::Float => IrType::Float,
            Type::String => IrType::String,
            Type::Array { elem, rank } => IrType::Array {
                elem: Box::new(self.sema_type_to_ir(elem)),
                rank: *rank,
            },
            Type::TypeRef { name } => IrType::TypeRef(*name),
            Type::StructVal { name } => IrType::StructVal(*name),
            Type::RuntimeType { name } => {
                IrType::RuntimeType(self.interner.resolve(*name).to_string())
            }
            Type::FnPtr { params, ret } => {
                let ir_params: Vec<_> = params.iter().map(|p| self.sema_type_to_ir(p)).collect();
                let ir_ret = ret
                    .as_ref()
                    .map(|r| self.sema_type_to_ir(r))
                    .unwrap_or(IrType::Void);
                IrType::FnPtr(Box::new(FnSig {
                    params: ir_params,
                    ret: Box::new(ir_ret),
                }))
            }
            Type::Null => IrType::Null,
            Type::Void => IrType::Void,
            Type::Error => IrType::Void,
        }
    }

    fn reset_function_state(&mut self) {
        self.current_scope = ScopeId(0);
        self.locals.clear();
        self.blocks.clear();
        self.next_reg = 0;
        self.next_block_id = 0;
        self.local_map.clear();
        self.context_stack.clear();
        self.label_blocks.clear();
        self.next_temp = 0;
    }

    fn current_block_is_terminated(&self) -> bool {
        self.blocks[self.current_block.0 as usize]
            .terminator
            .is_some()
    }

    fn start_dead_block(&mut self) {
        let bb = self.fresh_block();
        self.switch_to(bb);
    }

    // ── program-level lowering ──────────────────────────────────────

    fn lower_program(&mut self, program: &[NodeId]) -> Program {
        let top_scope = ScopeId(0);

        // Collect type/struct definitions.
        for (sym, decl) in self.symbols.iter_scope(top_scope) {
            match &decl.kind {
                DeclKind::TypeDef { fields } => {
                    self.type_defs.push(TypeDefInfo {
                        name: sym,
                        fields: fields
                            .iter()
                            .map(|f| (f.name, self.sema_type_to_ir(&f.ty)))
                            .collect(),
                    });
                }
                DeclKind::StructDef { fields } => {
                    self.struct_defs.push(StructDefInfo {
                        name: sym,
                        fields: fields
                            .iter()
                            .map(|f| (f.name, self.sema_type_to_ir(&f.ty)))
                            .collect(),
                    });
                }
                _ => {}
            }
        }

        // Build type_def_map for TypeDefId resolution.
        for (i, td) in self.type_defs.iter().enumerate() {
            self.type_def_map.insert(td.name, TypeDefId(i as u32));
        }

        // Collect global variables.
        for (sym, decl) in self.symbols.iter_scope(top_scope) {
            if matches!(decl.kind, DeclKind::Variable) && decl.is_global {
                let ir_ty = self.sema_type_to_ir(&decl.ty);
                let gid = GlobalId(self.globals.len() as u32);
                self.globals.push(Global {
                    name: sym,
                    ty: ir_ty,
                });
                self.global_map.insert(sym, gid);
            }
        }

        // Build func_table: runtime functions first, then user-defined.
        self.build_func_table(program, top_scope);

        // Lower user-defined functions (in source order matching pre-allocated body_index).
        let func_stmts: Vec<_> = program
            .iter()
            .filter(|&&id| matches!(self.arena[id], Node::Stmt(Stmt::Function { .. })))
            .copied()
            .collect();
        for id in func_stmts {
            self.lower_function_def(id);
        }

        // Lower top-level code into @main.
        self.lower_main(program, top_scope);

        Program {
            func_table: std::mem::take(&mut self.func_table),
            functions: std::mem::take(&mut self.functions),
            globals: std::mem::take(&mut self.globals),
            type_defs: std::mem::take(&mut self.type_defs),
            struct_defs: std::mem::take(&mut self.struct_defs),
        }
    }

    fn build_func_table(&mut self, program: &[NodeId], top_scope: ScopeId) {
        // 1. Register runtime functions.
        for (sym, decl) in self.symbols.iter_scope(top_scope) {
            match &decl.kind {
                DeclKind::RuntimeFn {
                    params,
                    return_ty,
                    c_symbol,
                    fn_ptr,
                } => {
                    let func_id = FuncId(self.func_table.len() as u32);
                    self.func_table.push(FuncDecl {
                        name: sym,
                        sig: FnSig {
                            params: params.iter().map(|p| self.sema_type_to_ir(&p.ty)).collect(),
                            ret: Box::new(self.sema_type_to_ir(return_ty)),
                        },
                        kind: FuncKind::Runtime {
                            symbol: c_symbol.clone(),
                            fn_ptr: *fn_ptr,
                        },
                    });
                    self.runtime_func_map.insert(c_symbol.clone(), func_id);
                }
                DeclKind::OverloadSet { variants } => {
                    for variant in variants {
                        let func_id = FuncId(self.func_table.len() as u32);
                        self.func_table.push(FuncDecl {
                            name: sym,
                            sig: FnSig {
                                params: variant
                                    .params
                                    .iter()
                                    .map(|p| self.sema_type_to_ir(&p.ty))
                                    .collect(),
                                ret: Box::new(self.sema_type_to_ir(&variant.return_ty)),
                            },
                            kind: FuncKind::Runtime {
                                symbol: variant.c_symbol.clone(),
                                fn_ptr: variant.fn_ptr,
                            },
                        });
                        self.runtime_func_map
                            .insert(variant.c_symbol.clone(), func_id);
                    }
                }
                _ => {}
            }
        }

        // 2. Register user-defined functions in source order.
        // Get param/return types from the symbol table (already resolved by sema).
        let func_stmts: Vec<_> = program
            .iter()
            .filter(|&&id| matches!(self.arena[id], Node::Stmt(Stmt::Function { .. })))
            .copied()
            .collect();

        for (body_index, &id) in func_stmts.iter().enumerate() {
            if let Node::Stmt(Stmt::Function { name_span, .. }) = &self.arena[id] {
                let name = self.intern_ident(*name_span, None);
                let (param_types, ret) = if let Some(decl) = self.symbols.lookup(top_scope, name)
                    && let DeclKind::Function {
                        params, return_ty, ..
                    } = &decl.kind
                {
                    let pt: Vec<_> = params.iter().map(|p| self.sema_type_to_ir(&p.ty)).collect();
                    let rt = Box::new(self.sema_type_to_ir(return_ty));
                    (pt, rt)
                } else {
                    (Vec::new(), Box::new(IrType::Void))
                };

                let func_id = FuncId(self.func_table.len() as u32);
                self.func_table.push(FuncDecl {
                    name,
                    sig: FnSig {
                        params: param_types,
                        ret,
                    },
                    kind: FuncKind::UserDefined { body_index },
                });
                self.func_id_map.insert(name, func_id);
            }
        }

        // 3. Register @main.
        let main_name = self.interner.intern("@main");
        let main_body_index = func_stmts.len();
        let func_id = FuncId(self.func_table.len() as u32);
        self.func_table.push(FuncDecl {
            name: main_name,
            sig: FnSig {
                params: Vec::new(),
                ret: Box::new(IrType::Void),
            },
            kind: FuncKind::UserDefined {
                body_index: main_body_index,
            },
        });
        self.func_id_map.insert(main_name, func_id);
    }

    fn lower_main(&mut self, program: &[NodeId], top_scope: ScopeId) {
        self.reset_function_state();
        let entry = self.fresh_block();
        self.switch_to(entry);

        let main_name = self.interner.intern("@main");

        // Allocate locals for top-level variables (sorted by source position
        // for deterministic output).
        let mut top_vars: Vec<_> = self
            .symbols
            .iter_scope(top_scope)
            .filter(|(_, decl)| matches!(decl.kind, DeclKind::Variable))
            .collect();
        top_vars.sort_by_key(|(_, decl)| decl.span.start);
        for (sym, decl) in top_vars {
            if decl.is_global {
                continue;
            }
            let ir_ty = self.sema_type_to_ir(&decl.ty);
            self.alloc_local(sym, ir_ty, false);
        }

        // Lower non-function/type/struct top-level statements.
        for &id in program {
            match &self.arena[id] {
                Node::Stmt(Stmt::Function { .. }) | Node::Stmt(Stmt::TypeDecl { .. }) => continue,
                _ => self.lower_stmt(id),
            }
        }

        // Ensure the last block has a terminator.
        if !self.current_block_is_terminated() {
            self.terminate(
                Terminator::Return { value: None },
                Span::new(0, 0, FileId::SYNTHETIC),
            );
        }

        self.functions.push(Function {
            name: main_name,
            params: Vec::new(),
            return_type: IrType::Void,
            locals: std::mem::take(&mut self.locals),
            blocks: std::mem::take(&mut self.blocks),
        });
    }

    fn lower_function_def(&mut self, id: NodeId) {
        let Node::Stmt(Stmt::Function {
            name_span, body, ..
        }) = self.arena[id].clone()
        else {
            return;
        };

        self.reset_function_state();
        let entry = self.fresh_block();
        self.switch_to(entry);

        let func_name = self.intern_ident(name_span, None);

        // Look up the function declaration to get param/return types and scope.
        let decl = self.symbols.lookup(ScopeId(0), func_name).cloned();
        let (param_types, ret_type, param_infos) = if let Some(ref d) = decl
            && let DeclKind::Function {
                params: ref param_infos,
                ref return_ty,
                ref scope,
            } = d.kind
        {
            if let Some(fn_scope) = scope {
                self.current_scope = *fn_scope;
            }
            let pt: Vec<_> = param_infos
                .iter()
                .map(|p| self.sema_type_to_ir(&p.ty))
                .collect();
            let rt = self.sema_type_to_ir(return_ty);
            (pt, rt, param_infos.clone())
        } else {
            (Vec::new(), IrType::Void, Vec::new())
        };

        // Allocate locals for parameters.
        for (i, pi) in param_infos.iter().enumerate() {
            self.alloc_local(pi.name, param_types[i].clone(), true);
        }

        // Find the function's scope to collect local variables.
        // The function scope is the child of the top-level scope with Function kind.
        // We'll scan Dim statements in the body to allocate locals.
        self.scan_body_for_locals(&body);

        // Scan for labels so forward Gotos can be resolved.
        self.scan_body_for_labels(&body);

        // Lower the function body.
        for &stmt_id in &body {
            self.lower_stmt(stmt_id);
        }

        // Ensure the last block has a terminator.
        if !self.current_block_is_terminated() {
            self.terminate(
                Terminator::Return { value: None },
                Span::new(0, 0, FileId::SYNTHETIC),
            );
        }

        self.functions.push(Function {
            name: func_name,
            params: param_types,
            return_type: ret_type,
            locals: std::mem::take(&mut self.locals),
            blocks: std::mem::take(&mut self.blocks),
        });
    }

    fn scan_body_for_locals(&mut self, body: &[NodeId]) {
        for &id in body {
            match self.arena[id].clone() {
                Node::Stmt(Stmt::VarDecl {
                    is_global: false,
                    names,
                    ..
                }) => {
                    for dn in &names {
                        let name = self.intern_ident(dn.name_span, dn.sigil);
                        if self.lookup_local(name).is_none() {
                            let var_ty = self
                                .symbols
                                .lookup(self.current_scope, name)
                                .map(|decl| self.sema_type_to_ir(&decl.ty))
                                .unwrap_or(IrType::Int);
                            self.alloc_local(name, var_ty, false);
                        }
                    }
                }
                Node::Stmt(Stmt::VarDecl {
                    is_global: true, ..
                }) => {
                    // Globals are collected at program level — no local slot needed.
                }
                // Recurse into nested bodies for variables declared in blocks.
                // CoolBasic has function-level scoping so all Dims in the function
                // are visible throughout.
                Node::Stmt(Stmt::If {
                    then_body,
                    elseifs,
                    else_body,
                    ..
                }) => {
                    self.scan_body_for_locals(&then_body);
                    for ei in &elseifs {
                        self.scan_body_for_locals(&ei.body);
                    }
                    if let Some(eb) = &else_body {
                        self.scan_body_for_locals(eb);
                    }
                }
                Node::Stmt(Stmt::While { body, .. })
                | Node::Stmt(Stmt::RepeatForever { body })
                | Node::Stmt(Stmt::RepeatWhile { body, .. }) => {
                    self.scan_body_for_locals(&body);
                }
                // `For`/`For Each` also bind a loop variable, which may be
                // implicit (no prior `Dim`, §6.3). The checker declares it in the
                // symbol table, but only `Dim`/`Assign` names are scanned here, so
                // allocate its local explicitly — otherwise `resolve_var` misses
                // and the loop silently aliases `LocalId(0)`.
                Node::Stmt(Stmt::For { var, body, .. })
                | Node::Stmt(Stmt::ForEach { var, body, .. }) => {
                    self.alloc_loop_var(var);
                    self.scan_body_for_locals(&body);
                }
                Node::Stmt(Stmt::Select { arms, .. }) => {
                    for &arm_id in &arms {
                        if let Node::CaseArm(ref arm) = self.arena[arm_id] {
                            let body = match arm {
                                CaseArm::Case { body, .. } => body,
                                CaseArm::Default { body } => body,
                            };
                            self.scan_body_for_locals(body);
                        }
                    }
                }
                // Implicit declarations via assignment — the checker already
                // created these in the symbol table.
                Node::Stmt(Stmt::Assign { target, .. }) => {
                    if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[target] {
                        let name = self.intern_ident(*name_span, *sigil);
                        if self.lookup_local(name).is_none() {
                            let var_ty = self
                                .types
                                .get(target)
                                .map(|t| self.sema_type_to_ir(t))
                                .unwrap_or(IrType::Int);
                            self.alloc_local(name, var_ty, false);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Allocate a local for a `For`/`For Each` loop variable if it does not
    /// already have one. The type comes from the checker's recorded type for the
    /// loop-variable node (it declares implicit loop vars in the symbol table and
    /// records their type), defaulting to `Int`.
    fn alloc_loop_var(&mut self, var: NodeId) {
        if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[var] {
            let name = self.intern_ident(*name_span, *sigil);
            if self.lookup_local(name).is_none() {
                let var_ty = self
                    .types
                    .get(var)
                    .map(|t| self.sema_type_to_ir(t))
                    .unwrap_or(IrType::Int);
                self.alloc_local(name, var_ty, false);
            }
        }
    }

    fn scan_body_for_labels(&mut self, body: &[NodeId]) {
        for &id in body {
            match self.arena[id].clone() {
                Node::Stmt(Stmt::Label { name_span }) => {
                    let name = self.intern_span(name_span);
                    if !self.label_blocks.contains_key(&name) {
                        let bb = self.fresh_block();
                        self.label_blocks.insert(name, bb);
                    }
                }
                Node::Stmt(Stmt::If {
                    then_body,
                    elseifs,
                    else_body,
                    ..
                }) => {
                    self.scan_body_for_labels(&then_body);
                    for ei in &elseifs {
                        self.scan_body_for_labels(&ei.body);
                    }
                    if let Some(eb) = &else_body {
                        self.scan_body_for_labels(eb);
                    }
                }
                Node::Stmt(Stmt::While { body, .. })
                | Node::Stmt(Stmt::RepeatForever { body })
                | Node::Stmt(Stmt::RepeatWhile { body, .. })
                | Node::Stmt(Stmt::For { body, .. })
                | Node::Stmt(Stmt::ForEach { body, .. }) => {
                    self.scan_body_for_labels(&body);
                }
                Node::Stmt(Stmt::Select { arms, .. }) => {
                    for &arm_id in &arms {
                        if let Node::CaseArm(ref arm) = self.arena[arm_id] {
                            let body = match arm {
                                CaseArm::Case { body, .. } => body,
                                CaseArm::Default { body } => body,
                            };
                            self.scan_body_for_labels(body);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ── expression lowering ─────────────────────────────────────────

    fn lower_expr(&mut self, id: NodeId) -> Reg {
        let span = self.arena.span_of(id);
        let reg = match self.arena[id].clone() {
            Node::Expr(Expr::IntLit(v)) => {
                let v = v as i64;
                if i32::try_from(v).is_ok() {
                    self.emit(InstKind::ConstInt(v), span)
                } else {
                    self.emit(InstKind::ConstLong(v), span)
                }
            }
            Node::Expr(Expr::FloatLit(v)) => self.emit(InstKind::ConstFloat(v.to_f64()), span),
            Node::Expr(Expr::StrLit { value, .. }) => self.emit(InstKind::ConstString(value), span),
            Node::Expr(Expr::NullLit) => self.emit(InstKind::ConstNull, span),

            Node::Expr(Expr::Ident { name_span, sigil }) => {
                self.lower_ident_expr(name_span, sigil, span)
            }

            Node::Expr(Expr::Binary { op, lhs, rhs }) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    return self.lower_short_circuit(op, lhs, rhs, span);
                }
                self.lower_binary(op, lhs, rhs, span)
            }

            Node::Expr(Expr::Unary { op, operand }) => {
                let val = self.lower_expr(operand);
                let ir_op = match op {
                    UnOp::Neg => IrUnOp::Neg,
                    // CoolBasic unary `+` is absolute value, not identity (FD-028).
                    UnOp::Plus => IrUnOp::Abs,
                    UnOp::Not => IrUnOp::Not,
                    UnOp::BinNot => IrUnOp::BinNot,
                };
                self.emit(
                    InstKind::UnOp {
                        op: ir_op,
                        operand: val,
                    },
                    span,
                )
            }

            Node::Expr(Expr::Call { callee, args }) => self.lower_call(id, callee, &args, span),

            Node::Expr(Expr::Index { array, indices }) => {
                let arr = self.lower_expr(array);
                let idxs: Vec<_> = indices.iter().map(|&i| self.lower_expr(i)).collect();
                self.emit(
                    InstKind::GetElement {
                        array: arr,
                        indices: idxs,
                    },
                    span,
                )
            }

            Node::Expr(Expr::Field { target, name_span }) => {
                let obj = self.lower_expr(target);
                let field_name = self.intern_span(name_span);
                let field_ty = self
                    .types
                    .get(id)
                    .map(|t| self.sema_type_to_ir(t))
                    .unwrap_or(IrType::Void);
                self.emit(
                    InstKind::GetField {
                        object: obj,
                        field: field_name,
                        field_type: field_ty,
                    },
                    span,
                )
            }

            Node::Expr(Expr::Paren { inner }) => return self.lower_expr(inner),

            Node::Expr(Expr::New(kind)) => match kind {
                NewKind::Type(_type_expr_id) => {
                    let type_def = match self.types.get(id) {
                        Some(Type::TypeRef { name }) => self.type_def_map.get(name).copied(),
                        _ => None,
                    };
                    match type_def {
                        Some(type_def) => self.emit(InstKind::NewType { type_def }, span),
                        None => {
                            // Sema resolves `New T` to a known TypeRef; this only
                            // fires on degenerate/already-errored input. Emit a
                            // null rather than indexing a fabricated key.
                            debug_assert!(
                                false,
                                "New Type lowered with unresolved type {:?}",
                                self.types.get(id)
                            );
                            self.emit(InstKind::ConstNull, span)
                        }
                    }
                }
                NewKind::Array { elem: _, dims } => {
                    let elem_ty = self
                        .types
                        .get(id)
                        .and_then(|t| {
                            if let Type::Array { elem, .. } = t {
                                Some(self.sema_type_to_ir(elem))
                            } else {
                                None
                            }
                        })
                        .unwrap_or(IrType::Int);
                    let dim_regs: Vec<_> = dims.iter().map(|&d| self.lower_expr(d)).collect();
                    self.emit(
                        InstKind::NewArray {
                            elem_type: elem_ty,
                            dims: dim_regs,
                        },
                        span,
                    )
                }
            },

            Node::Expr(Expr::Error) => self.emit(InstKind::ConstNull, span),

            _ => self.emit(InstKind::ConstNull, span),
        };

        // Apply implicit conversion if one was recorded.
        self.maybe_convert(id, reg, span)
    }

    fn maybe_convert(&mut self, id: NodeId, reg: Reg, span: Span) -> Reg {
        if let Some((_conv, target_ty)) = self.conversions.get_with_target(id) {
            let from = self
                .types
                .get(id)
                .map(|t| self.sema_type_to_ir(t))
                .unwrap_or(IrType::Void);
            let to = self.sema_type_to_ir(target_ty);
            self.emit(
                InstKind::Convert {
                    value: reg,
                    from,
                    to,
                },
                span,
            )
        } else {
            reg
        }
    }

    fn lower_ident_expr(
        &mut self,
        name_span: Span,
        sigil: Option<cb_frontend::Sigil>,
        span: Span,
    ) -> Reg {
        let name = self.intern_ident(name_span, sigil);

        // Check if this is a constant — inline its value.
        if let Some(decl) = self.symbols.lookup(self.current_scope, name)
            && let DeclKind::Constant { value } = &decl.kind
        {
            return match *value {
                ConstValue::Int(v) => {
                    if i32::try_from(v).is_ok() {
                        self.emit(InstKind::ConstInt(v), span)
                    } else {
                        self.emit(InstKind::ConstLong(v), span)
                    }
                }
                ConstValue::Float(v) => self.emit(InstKind::ConstFloat(v), span),
                ConstValue::String(ref v) => self.emit(InstKind::ConstString(v.clone()), span),
            };
        }

        // Regular variable load.
        if let Some(var) = self.resolve_var(name) {
            return self.emit_load_var(var, span);
        }

        // A bare function name in value position is its address — a fn-pointer
        // (cb_syntax.md §7.4). The call path (see `lower_call`) intercepts a
        // function name used as a callee before it reaches here, and bare 0-arg
        // sub calls are intercepted in `lower_stmt`, so this only fires for
        // genuine value uses. §7.2 forbids overloading, so the name resolves to
        // exactly one `func_id`.
        if let Some(decl) = self.symbols.lookup(self.current_scope, name)
            && matches!(decl.kind, DeclKind::Function { .. })
            && let Some(&func_id) = self.func_id_map.get(&name)
        {
            return self.emit(InstKind::FuncAddr { func: func_id }, span);
        }

        self.emit(InstKind::ConstNull, span)
    }

    fn lower_binary(&mut self, op: BinOp, lhs: NodeId, rhs: NodeId, span: Span) -> Reg {
        let lhs_reg = self.lower_expr(lhs);
        let rhs_reg = self.lower_expr(rhs);

        // Logical `Xor` (cb_syntax.md §5.1): operands are tested as `<> 0` and the
        // result is canonical Integer 1/0 — distinct from the bitwise `BinXor`
        // operator. `check_binary` already coerces both operands to `Int`, so we
        // only need to normalize each to 0/1 before the bitwise xor.
        if matches!(op, BinOp::Xor) {
            let zero = self.emit(InstKind::ConstInt(0), span);
            let lb = self.emit(
                InstKind::BinOp {
                    op: IrBinOp::NotEq,
                    lhs: lhs_reg,
                    rhs: zero,
                },
                span,
            );
            let rb = self.emit(
                InstKind::BinOp {
                    op: IrBinOp::NotEq,
                    lhs: rhs_reg,
                    rhs: zero,
                },
                span,
            );
            return self.emit(
                InstKind::BinOp {
                    op: IrBinOp::BinXor,
                    lhs: lb,
                    rhs: rb,
                },
                span,
            );
        }

        // Check if this is a string operation by looking at operand types.
        let is_string = self
            .types
            .get(lhs)
            .is_some_and(|t| matches!(t, Type::String))
            || self
                .types
                .get(rhs)
                .is_some_and(|t| matches!(t, Type::String));

        let ir_op = if is_string {
            match op {
                BinOp::Add => IrBinOp::StrConcat,
                BinOp::Eq => IrBinOp::StrEq,
                BinOp::NotEq => IrBinOp::StrNotEq,
                BinOp::Lt => IrBinOp::StrLt,
                BinOp::Gt => IrBinOp::StrGt,
                BinOp::LtEq => IrBinOp::StrLtEq,
                BinOp::GtEq => IrBinOp::StrGtEq,
                _ => self.map_binop(op),
            }
        } else {
            self.map_binop(op)
        };

        self.emit(
            InstKind::BinOp {
                op: ir_op,
                lhs: lhs_reg,
                rhs: rhs_reg,
            },
            span,
        )
    }

    fn map_binop(&self, op: BinOp) -> IrBinOp {
        match op {
            BinOp::Add => IrBinOp::Add,
            BinOp::Sub => IrBinOp::Sub,
            BinOp::Mul => IrBinOp::Mul,
            BinOp::Div => IrBinOp::Div,
            BinOp::Pow => IrBinOp::Pow,
            BinOp::Mod => IrBinOp::Mod,
            BinOp::BinAnd => IrBinOp::BinAnd,
            BinOp::BinOr => IrBinOp::BinOr,
            BinOp::BinXor => IrBinOp::BinXor,
            BinOp::Shl => IrBinOp::Shl,
            BinOp::Shr => IrBinOp::Shr,
            BinOp::Sar => IrBinOp::Sar,
            BinOp::Eq => IrBinOp::Eq,
            BinOp::NotEq => IrBinOp::NotEq,
            BinOp::Lt => IrBinOp::Lt,
            BinOp::Gt => IrBinOp::Gt,
            BinOp::LtEq => IrBinOp::LtEq,
            BinOp::GtEq => IrBinOp::GtEq,
            // Logical Xor is normalized to 0/1 in `lower_binary` before this point.
            BinOp::Xor => unreachable!("logical Xor handled in lower_binary"),
            // And/Or should have been handled by lower_short_circuit
            BinOp::And | BinOp::Or => unreachable!("And/Or handled by short-circuit lowering"),
        }
    }

    fn lower_short_circuit(&mut self, op: BinOp, lhs: NodeId, rhs: NodeId, span: Span) -> Reg {
        // Allocate a unique temp local for the result. Logical ops yield Int
        // 1/0 (FD-035), so the temp and both short-circuit constants are Int.
        let tmp = self.alloc_temp("@sc", IrType::Int);

        let lhs_reg = self.lower_expr(lhs);

        let rhs_block = self.fresh_block();
        let short_block = self.fresh_block();
        let merge_block = self.fresh_block();

        match op {
            BinOp::And => {
                // If lhs is true, evaluate rhs; otherwise short-circuit to false.
                self.terminate(
                    Terminator::BranchIf {
                        cond: lhs_reg,
                        then_block: rhs_block,
                        else_block: short_block,
                    },
                    span,
                );

                // Short-circuit block: result = 0 (false)
                self.switch_to(short_block);
                let false_reg = self.emit(InstKind::ConstInt(0), span);
                self.emit_void(
                    InstKind::StoreLocal {
                        local: tmp,
                        value: false_reg,
                    },
                    span,
                );
                self.terminate(Terminator::Goto(merge_block), span);
            }
            BinOp::Or => {
                // If lhs is true, short-circuit to true; otherwise evaluate rhs.
                self.terminate(
                    Terminator::BranchIf {
                        cond: lhs_reg,
                        then_block: short_block,
                        else_block: rhs_block,
                    },
                    span,
                );

                // Short-circuit block: result = 1 (true)
                self.switch_to(short_block);
                let true_reg = self.emit(InstKind::ConstInt(1), span);
                self.emit_void(
                    InstKind::StoreLocal {
                        local: tmp,
                        value: true_reg,
                    },
                    span,
                );
                self.terminate(Terminator::Goto(merge_block), span);
            }
            _ => unreachable!(),
        }

        // RHS block: evaluate rhs and store result.
        self.switch_to(rhs_block);
        let rhs_reg = self.lower_expr(rhs);
        self.emit_void(
            InstKind::StoreLocal {
                local: tmp,
                value: rhs_reg,
            },
            span,
        );
        self.terminate(Terminator::Goto(merge_block), span);

        // Merge block: load result.
        self.switch_to(merge_block);
        self.emit(InstKind::LoadLocal { local: tmp }, span)
    }

    fn lower_call(&mut self, call_id: NodeId, callee: NodeId, args: &[NodeId], span: Span) -> Reg {
        // Check for intrinsic calls.
        if let Node::Expr(Expr::Ident {
            name_span,
            sigil: None,
        }) = &self.arena[callee]
        {
            let name = self.intern_ident(*name_span, None);
            // Match on the case-folded name; `resolve` returns the original
            // casing, so an unfolded compare would miss `LEN`, `Int`, etc.
            let name_str = cb_diagnostics::fold(self.interner.resolve(name));

            match name_str.as_str() {
                "len" => {
                    // Len(s$) lowers to StrLen; Len(arr[, dim]) to Len. Mirror
                    // the operand-type probe used for string binops above.
                    if self
                        .types
                        .get(args[0])
                        .is_some_and(|t| matches!(t, Type::String))
                    {
                        let s = self.lower_expr(args[0]);
                        return self.emit(InstKind::StrLen { s }, span);
                    }
                    let arr = self.lower_expr(args[0]);
                    let dim = if args.len() > 1 {
                        Some(self.lower_expr(args[1]))
                    } else {
                        None
                    };
                    return self.emit(InstKind::Len { array: arr, dim }, span);
                }
                "int" | "integer" => {
                    let val = self.lower_expr(args[0]);
                    return self.emit(
                        InstKind::ConvertExplicit {
                            value: val,
                            target: IrType::Int,
                        },
                        span,
                    );
                }
                "float" => {
                    let val = self.lower_expr(args[0]);
                    return self.emit(
                        InstKind::ConvertExplicit {
                            value: val,
                            target: IrType::Float,
                        },
                        span,
                    );
                }
                "str" => {
                    let val = self.lower_expr(args[0]);
                    return self.emit(
                        InstKind::ConvertExplicit {
                            value: val,
                            target: IrType::String,
                        },
                        span,
                    );
                }
                "first" | "last" => {
                    let ty = self.types.get(args[0]).cloned().unwrap_or(Type::Void);
                    let type_def = match &ty {
                        Type::TypeRef { name } => self.type_def_map.get(name).copied(),
                        _ => None,
                    };
                    let Some(type_def) = type_def else {
                        // Sema resolves the operand to a known TypeRef; degenerate
                        // input falls back to null instead of a panic.
                        debug_assert!(false, "{name_str} lowered with unresolved type {ty:?}");
                        return self.emit(InstKind::ConstNull, span);
                    };
                    return if name_str == "first" {
                        self.emit(InstKind::First { type_def }, span)
                    } else {
                        self.emit(InstKind::Last { type_def }, span)
                    };
                }
                "next" => {
                    let obj = self.lower_expr(args[0]);
                    return self.emit(InstKind::Next { object: obj }, span);
                }
                "previous" => {
                    let obj = self.lower_expr(args[0]);
                    return self.emit(InstKind::Previous { object: obj }, span);
                }
                _ => {}
            }
        }

        // Use resolved_calls to determine the call target.
        if let Some(resolved) = self.resolved_calls.get(&call_id) {
            let arg_regs: Vec<_> = args.iter().map(|&a| self.lower_expr(a)).collect();
            let func_id = match resolved {
                ResolvedCall::UserDefined { name } => self.func_id_map[name],
                ResolvedCall::RuntimeFn { c_symbol } => self.runtime_func_map[c_symbol],
            };
            return self.emit(
                InstKind::Call {
                    callee: func_id,
                    args: arg_regs,
                },
                span,
            );
        }

        // Check if callee is an identifier referring to a known function
        // (fallback for calls not in resolved_calls, e.g. function pointers).
        if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[callee] {
            let name = self.intern_ident(*name_span, *sigil);
            if let Some(decl) = self.symbols.lookup(self.current_scope, name)
                && matches!(
                    decl.kind,
                    DeclKind::Function { .. }
                        | DeclKind::RuntimeFn { .. }
                        | DeclKind::OverloadSet { .. }
                )
            {
                let arg_regs: Vec<_> = args.iter().map(|&a| self.lower_expr(a)).collect();
                let func_id = self.func_id_map[&name];
                return self.emit(
                    InstKind::Call {
                        callee: func_id,
                        args: arg_regs,
                    },
                    span,
                );
            }
        }

        // Indirect call (function pointer or unknown callee).
        let callee_reg = self.lower_expr(callee);
        let arg_regs: Vec<_> = args.iter().map(|&a| self.lower_expr(a)).collect();
        self.emit(
            InstKind::CallIndirect {
                callee: callee_reg,
                args: arg_regs,
            },
            span,
        )
    }

    // ── statement lowering ──────────────────────────────────────────

    fn lower_stmt(&mut self, id: NodeId) {
        if self.current_block_is_terminated() {
            self.start_dead_block();
        }

        match self.arena[id].clone() {
            Node::Stmt(Stmt::Assign { target, value }) => {
                self.lower_assign(target, value, id);
            }
            Node::Stmt(Stmt::ExprStmt { expr }) => {
                // A bare identifier in statement position that resolves to a
                // function is a 0-arg call (CoolBasic subroutine call syntax).
                if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[expr] {
                    let name = self.intern_ident(*name_span, *sigil);
                    if let Some(decl) = self.symbols.lookup(self.current_scope, name) {
                        let func_id = match &decl.kind {
                            DeclKind::Function { .. } => self.func_id_map.get(&name).copied(),
                            DeclKind::RuntimeFn { c_symbol, .. } => {
                                self.runtime_func_map.get(c_symbol).copied()
                            }
                            // An overloaded command called bare (no parens, no
                            // args) — e.g. `DrawScreen`, `Lock` — resolves to its
                            // zero-parameter variant. Without this arm the call is
                            // silently dropped (the window never flips/pumps).
                            DeclKind::OverloadSet { variants } => variants
                                .iter()
                                .find(|v| v.params.is_empty())
                                .and_then(|v| self.runtime_func_map.get(&v.c_symbol).copied()),
                            _ => None,
                        };
                        if let Some(func_id) = func_id {
                            let span = self.arena.span_of(expr);
                            self.emit(
                                InstKind::Call {
                                    callee: func_id,
                                    args: vec![],
                                },
                                span,
                            );
                            return;
                        }
                    }
                }
                self.lower_expr(expr);
                // `MakeError(msg)` terminates the program. The call itself
                // (cb_rt_make_error) only writes the message to stderr; the
                // termination is this Halt(1). Detected via the resolved
                // runtime symbol, so MakeError needs no special sema.
                let is_make_error = matches!(
                    self.resolved_calls.get(&expr),
                    Some(ResolvedCall::RuntimeFn { c_symbol }) if c_symbol == "cb_rt_make_error"
                );
                if is_make_error {
                    self.terminate(Terminator::Halt { code: 1 }, self.arena.span_of(expr));
                }
            }
            Node::Stmt(Stmt::VarDecl {
                is_global: false,
                names,
                init: Some(init_id),
                ..
            }) => {
                let val = self.lower_expr(init_id);
                for dn in &names {
                    let name = self.intern_ident(dn.name_span, dn.sigil);
                    if let Some(local) = self.lookup_local(name) {
                        let span = self.arena.span_of(id);
                        self.emit_void(InstKind::StoreLocal { local, value: val }, span);
                    }
                }
            }
            Node::Stmt(Stmt::VarDecl {
                is_global: true,
                names,
                init: Some(init_id),
                ..
            }) => {
                let val = self.lower_expr(init_id);
                let span = self.arena.span_of(id);
                for dn in &names {
                    let name = self.intern_ident(dn.name_span, dn.sigil);
                    if let Some(var) = self.resolve_var(name) {
                        self.emit_store_var(var, val, span);
                    }
                }
            }
            // `Dim`/`Global` with no initializer: declaration only, no IR.
            Node::Stmt(Stmt::VarDecl { .. }) => {}
            Node::Stmt(Stmt::Const { .. }) => {
                // Constants are inlined at use sites — no IR needed.
            }
            Node::Stmt(Stmt::Return { value }) => {
                let val = value.map(|v| self.lower_expr(v));
                self.terminate(Terminator::Return { value: val }, self.arena.span_of(id));
            }
            Node::Stmt(Stmt::End) => {
                // Terminate the whole program with exit code 0. Like Return,
                // this ends the block; any following statements are dead.
                self.terminate(Terminator::Halt { code: 0 }, self.arena.span_of(id));
            }
            Node::Stmt(Stmt::Delete { operand }) => {
                let span = self.arena.span_of(id);
                match self.delete_classes.get(&id) {
                    Some(DeleteClass::Lvalue) => {
                        if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[operand] {
                            let name = self.intern_ident(*name_span, *sigil);
                            match self.resolve_var(name) {
                                Some(VarRef::Local(local)) => {
                                    self.emit_void(InstKind::DeleteLvalue { local }, span);
                                }
                                Some(VarRef::Global(global)) => {
                                    self.emit_void(InstKind::DeleteLvalueGlobal { global }, span);
                                }
                                None => {}
                            }
                        }
                    }
                    Some(DeleteClass::Rvalue) | None => {
                        let val = self.lower_expr(operand);
                        self.emit_void(InstKind::DeleteRvalue { value: val }, span);
                    }
                }
            }
            Node::Stmt(Stmt::Redim { target, dims, .. }) => {
                let span = self.arena.span_of(id);
                if let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[target] {
                    let name = self.intern_ident(*name_span, *sigil);
                    let dim_regs: Vec<_> = dims.iter().map(|&d| self.lower_expr(d)).collect();
                    match self.resolve_var(name) {
                        Some(VarRef::Local(local)) => {
                            let elem_type = self.locals[local.0 as usize].ty.clone();
                            let elem_ir = if let IrType::Array { elem, .. } = &elem_type {
                                *elem.clone()
                            } else {
                                IrType::Int
                            };
                            self.emit_void(
                                InstKind::Redim {
                                    local,
                                    elem_type: elem_ir,
                                    dims: dim_regs,
                                },
                                span,
                            );
                        }
                        Some(VarRef::Global(global)) => {
                            let elem_type = self.globals[global.0 as usize].ty.clone();
                            let elem_ir = if let IrType::Array { elem, .. } = &elem_type {
                                *elem.clone()
                            } else {
                                IrType::Int
                            };
                            self.emit_void(
                                InstKind::RedimGlobal {
                                    global,
                                    elem_type: elem_ir,
                                    dims: dim_regs,
                                },
                                span,
                            );
                        }
                        None => {}
                    }
                }
            }
            Node::Stmt(Stmt::Label { name_span }) => {
                let name = self.intern_span(name_span);
                let label_bb = self.label_blocks.get(&name).copied().unwrap_or_else(|| {
                    let bb = self.fresh_block();
                    self.label_blocks.insert(name, bb);
                    bb
                });
                if !self.current_block_is_terminated() {
                    self.terminate(Terminator::Goto(label_bb), self.arena.span_of(id));
                }
                self.switch_to(label_bb);
            }
            Node::Stmt(Stmt::Goto { name_span }) => {
                let name = self.intern_span(name_span);
                let target = self.label_blocks.get(&name).copied().unwrap_or_else(|| {
                    let bb = self.fresh_block();
                    self.label_blocks.insert(name, bb);
                    bb
                });
                self.terminate(Terminator::Goto(target), self.arena.span_of(id));
            }
            Node::Stmt(Stmt::Break { count }) => {
                let n = count.map_or(1, |c| c.get()) as usize;
                let mut loops_found = 0usize;
                let mut exit_block = None;
                for ctx in self.context_stack.iter().rev() {
                    if let ControlContext::Loop { exit_block: eb, .. } = ctx {
                        loops_found += 1;
                        if loops_found == n {
                            exit_block = Some(*eb);
                            break;
                        }
                    }
                }
                if let Some(eb) = exit_block {
                    self.terminate(Terminator::Goto(eb), self.arena.span_of(id));
                } else {
                    // Sema (E0332) rejects a `Break` with no enclosing loop, so
                    // this is unreachable for checked input. Stay well-formed for
                    // release/hand-written IR by terminating the block anyway.
                    debug_assert!(
                        false,
                        "Break with no enclosing loop should be rejected by sema (E0332)"
                    );
                    self.terminate(Terminator::Return { value: None }, self.arena.span_of(id));
                }
            }
            Node::Stmt(Stmt::Continue) => {
                match self.context_stack.iter().next_back() {
                    Some(ControlContext::Loop { continue_block, .. }) => {
                        self.terminate(Terminator::Goto(*continue_block), self.arena.span_of(id));
                    }
                    Some(ControlContext::Select {
                        next_arm_body: Some(target),
                    }) => {
                        self.terminate(Terminator::Goto(*target), self.arena.span_of(id));
                    }
                    Some(ControlContext::Select {
                        next_arm_body: None,
                    }) => {
                        // `Continue` in the final `Select` arm falls through past
                        // the `Select`; the arm-lowering merge guard terminates
                        // this block, so nothing is emitted here.
                    }
                    None => {
                        // Sema (E0332) rejects a `Continue` outside any loop or
                        // `Select`; unreachable for checked input.
                        debug_assert!(
                            false,
                            "Continue outside loop/Select should be rejected by sema (E0332)"
                        );
                    }
                }
            }

            // ── control flow ────────────────────────────────────────
            Node::Stmt(Stmt::If {
                cond,
                then_body,
                elseifs,
                else_body,
                ..
            }) => {
                self.lower_if(cond, &then_body, &elseifs, else_body.as_deref(), id);
            }
            Node::Stmt(Stmt::While { cond, body }) => {
                self.lower_while(cond, &body, id);
            }
            Node::Stmt(Stmt::RepeatForever { body }) => {
                self.lower_repeat_forever(&body, id);
            }
            Node::Stmt(Stmt::RepeatWhile { body, cond }) => {
                self.lower_repeat_while(&body, cond, id);
            }
            Node::Stmt(Stmt::For {
                var,
                from,
                to,
                step,
                body,
                ..
            }) => {
                self.lower_for(var, from, to, step, &body, id);
            }
            Node::Stmt(Stmt::ForEach {
                var, source, body, ..
            }) => {
                self.lower_for_each(var, source, &body, id);
            }
            Node::Stmt(Stmt::Select { scrutinee, arms }) => {
                self.lower_select(scrutinee, &arms, id);
            }

            // These are handled at the program level, not inline.
            Node::Stmt(Stmt::Function { .. })
            | Node::Stmt(Stmt::TypeDecl { .. })
            | Node::Stmt(Stmt::FieldDecl { .. })
            | Node::Stmt(Stmt::Include { .. })
            | Node::Stmt(Stmt::Error) => {}

            _ => {}
        }
    }

    fn lower_assign(&mut self, target: NodeId, value: NodeId, stmt_id: NodeId) {
        let span = self.arena.span_of(stmt_id);
        let val = self.lower_expr(value);

        match self.arena[target].clone() {
            Node::Expr(Expr::Ident { name_span, sigil }) => {
                let name = self.intern_ident(name_span, sigil);
                if let Some(var) = self.resolve_var(name) {
                    self.emit_store_var(var, val, span);
                }
            }
            Node::Expr(Expr::Field { .. }) | Node::Expr(Expr::Index { .. }) => {
                // Field/index targets address an owning storage location, not a
                // register value: a value-type struct lives inline, so mutating
                // a `LoadLocal`/`GetField`/`GetElement` register copy would be
                // lost. Resolve the place (root variable + projection path) and
                // emit a single in-place store.
                if let Some((root, path)) = self.lower_place(target) {
                    self.emit_void(
                        InstKind::StorePlace {
                            root,
                            path,
                            value: val,
                        },
                        span,
                    );
                }
            }
            _ => {}
        }
    }

    /// Resolve an assignment lvalue to an owning root variable plus a chain of
    /// field/index projections (left-to-right from the root). Index
    /// expressions are lowered to registers here, after the RHS, preserving
    /// evaluation order. Returns `None` if the target does not bottom out at a
    /// variable (sema rejects such non-lvalues before lowering).
    fn lower_place(&mut self, target: NodeId) -> Option<(PlaceRoot, Vec<Projection>)> {
        match self.arena[target].clone() {
            Node::Expr(Expr::Ident { name_span, sigil }) => {
                let name = self.intern_ident(name_span, sigil);
                let root = match self.resolve_var(name)? {
                    VarRef::Local(id) => PlaceRoot::Local(id),
                    VarRef::Global(id) => PlaceRoot::Global(id),
                };
                Some((root, Vec::new()))
            }
            Node::Expr(Expr::Field {
                target: obj,
                name_span,
            }) => {
                let (root, mut path) = self.lower_place(obj)?;
                path.push(Projection::Field(self.intern_span(name_span)));
                Some((root, path))
            }
            Node::Expr(Expr::Index { array, indices }) => {
                let (root, mut path) = self.lower_place(array)?;
                let idx_regs: Vec<_> = indices.iter().map(|&i| self.lower_expr(i)).collect();
                path.push(Projection::Index(idx_regs));
                Some((root, path))
            }
            Node::Expr(Expr::Paren { inner }) => self.lower_place(inner),
            _ => None,
        }
    }

    // ── control flow lowering ───────────────────────────────────────

    fn lower_if(
        &mut self,
        cond: NodeId,
        then_body: &[NodeId],
        elseifs: &[cb_frontend::ast::ElseIf],
        else_body: Option<&[NodeId]>,
        stmt_id: NodeId,
    ) {
        let span = self.arena.span_of(stmt_id);
        let merge_block = self.fresh_block();
        let then_block = self.fresh_block();

        let first_else = if !elseifs.is_empty() || else_body.is_some() {
            self.fresh_block()
        } else {
            merge_block
        };

        let cond_reg = self.lower_expr(cond);
        self.terminate(
            Terminator::BranchIf {
                cond: cond_reg,
                then_block,
                else_block: first_else,
            },
            span,
        );

        // Then block.
        self.switch_to(then_block);
        for &s in then_body {
            self.lower_stmt(s);
        }
        if !self.current_block_is_terminated() {
            self.terminate(Terminator::Goto(merge_block), span);
        }

        // ElseIf chain.
        let mut current_else = first_else;
        for (i, ei) in elseifs.iter().enumerate() {
            self.switch_to(current_else);
            let ei_then = self.fresh_block();
            let ei_else = if i + 1 < elseifs.len() || else_body.is_some() {
                self.fresh_block()
            } else {
                merge_block
            };

            let ei_cond = self.lower_expr(ei.cond);
            self.terminate(
                Terminator::BranchIf {
                    cond: ei_cond,
                    then_block: ei_then,
                    else_block: ei_else,
                },
                span,
            );

            self.switch_to(ei_then);
            for &s in &ei.body {
                self.lower_stmt(s);
            }
            if !self.current_block_is_terminated() {
                self.terminate(Terminator::Goto(merge_block), span);
            }

            current_else = ei_else;
        }

        // Else block.
        if let Some(eb) = else_body {
            self.switch_to(current_else);
            for &s in eb {
                self.lower_stmt(s);
            }
            if !self.current_block_is_terminated() {
                self.terminate(Terminator::Goto(merge_block), span);
            }
        }

        self.switch_to(merge_block);
    }

    /// Lower a loop body into the *current* block — the caller must already have
    /// `switch_to`'d the body block and emitted any pre-body bindings (e.g.
    /// for-each's element load). Pushes the loop control context, lowers `body`,
    /// pops, and falls through to `continue_block` unless the body already
    /// terminated. The caller emits the step/condition block and the final
    /// `switch_to(exit_block)`, since those vary per loop. (S-M13)
    fn lower_loop_body(
        &mut self,
        body: &[NodeId],
        continue_block: BlockId,
        exit_block: BlockId,
        span: Span,
    ) {
        self.context_stack.push(ControlContext::Loop {
            continue_block,
            exit_block,
        });
        for &s in body {
            self.lower_stmt(s);
        }
        self.context_stack.pop();
        if !self.current_block_is_terminated() {
            self.terminate(Terminator::Goto(continue_block), span);
        }
    }

    fn lower_while(&mut self, cond: NodeId, body: &[NodeId], stmt_id: NodeId) {
        let span = self.arena.span_of(stmt_id);
        let cond_block = self.fresh_block();
        let body_block = self.fresh_block();
        let exit_block = self.fresh_block();

        self.terminate(Terminator::Goto(cond_block), span);

        // Condition block.
        self.switch_to(cond_block);
        let cond_reg = self.lower_expr(cond);
        self.terminate(
            Terminator::BranchIf {
                cond: cond_reg,
                then_block: body_block,
                else_block: exit_block,
            },
            span,
        );

        // Body block.
        self.switch_to(body_block);
        self.lower_loop_body(body, cond_block, exit_block, span);

        self.switch_to(exit_block);
    }

    fn lower_repeat_forever(&mut self, body: &[NodeId], stmt_id: NodeId) {
        let span = self.arena.span_of(stmt_id);
        let body_block = self.fresh_block();
        let exit_block = self.fresh_block();

        self.terminate(Terminator::Goto(body_block), span);

        self.switch_to(body_block);
        self.lower_loop_body(body, body_block, exit_block, span);

        self.switch_to(exit_block);
    }

    fn lower_repeat_while(&mut self, body: &[NodeId], cond: NodeId, stmt_id: NodeId) {
        let span = self.arena.span_of(stmt_id);
        let body_block = self.fresh_block();
        let cond_block = self.fresh_block();
        let exit_block = self.fresh_block();

        self.terminate(Terminator::Goto(body_block), span);

        // Body block.
        self.switch_to(body_block);
        self.lower_loop_body(body, cond_block, exit_block, span);

        // Condition block.
        self.switch_to(cond_block);
        let cond_reg = self.lower_expr(cond);
        self.terminate(
            Terminator::BranchIf {
                cond: cond_reg,
                then_block: body_block,
                else_block: exit_block,
            },
            span,
        );

        self.switch_to(exit_block);
    }

    fn lower_for(
        &mut self,
        var: NodeId,
        from: NodeId,
        to: NodeId,
        step: Option<NodeId>,
        body: &[NodeId],
        stmt_id: NodeId,
    ) {
        let span = self.arena.span_of(stmt_id);

        // Get the loop variable. `scan_body_for_locals`/`lower_main` always
        // allocate a slot for it (including implicit loop vars), so a miss here
        // is an internal lowering bug — fail loudly rather than alias LocalId(0).
        let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[var] else {
            panic!("For loop variable is not an identifier — internal lowering bug");
        };
        let name = self.intern_ident(*name_span, *sigil);
        let var_ref = self
            .resolve_var(name)
            .unwrap_or_else(|| panic!("For loop variable not allocated — internal lowering bug"));

        // Initialize: var = from
        let from_reg = self.lower_expr(from);
        self.emit_store_var(var_ref, from_reg, span);

        // Cache "to" value in a unique temp local (safe for nested For loops).
        let to_reg = self.lower_expr(to);
        let var_ty = match var_ref {
            VarRef::Local(id) => self.locals[id.0 as usize].ty.clone(),
            VarRef::Global(id) => self.globals[id.0 as usize].ty.clone(),
        };
        // Constants synthesised below — the default step `1` (when `Step` is
        // omitted) and the `0` of the direction test — must be emitted in the
        // loop-variable type so all `For` IR operands agree; `check_for` coerces
        // from/to/step to this type, so the loaded regs match.
        let var_is_float = var_ty == IrType::Float;
        let to_local = self.alloc_temp("@for_to", var_ty.clone());
        self.emit_void(
            InstKind::StoreLocal {
                local: to_local,
                value: to_reg,
            },
            span,
        );

        // Cache "step" value (default 1) in a unique temp local.
        let step_reg = if let Some(step_id) = step {
            self.lower_expr(step_id)
        } else if var_is_float {
            self.emit(InstKind::ConstFloat(1.0), span)
        } else {
            self.emit(InstKind::ConstInt(1), span)
        };
        let step_local = self.alloc_temp("@for_step", var_ty);
        self.emit_void(
            InstKind::StoreLocal {
                local: step_local,
                value: step_reg,
            },
            span,
        );

        let cond_up_block = self.fresh_block();
        let cond_down_block = self.fresh_block();
        let cond_check_block = self.fresh_block();
        let body_block = self.fresh_block();
        let step_block = self.fresh_block();
        let exit_block = self.fresh_block();

        self.terminate(Terminator::Goto(cond_check_block), span);

        // Direction check block: if step > 0, use <=; else use >=.
        self.switch_to(cond_check_block);
        let step_val = self.emit(InstKind::LoadLocal { local: step_local }, span);
        let zero = if var_is_float {
            self.emit(InstKind::ConstFloat(0.0), span)
        } else {
            self.emit(InstKind::ConstInt(0), span)
        };
        let step_positive = self.emit(
            InstKind::BinOp {
                op: IrBinOp::Gt,
                lhs: step_val,
                rhs: zero,
            },
            span,
        );
        self.terminate(
            Terminator::BranchIf {
                cond: step_positive,
                then_block: cond_up_block,
                else_block: cond_down_block,
            },
            span,
        );

        // Ascending check: var <= to
        self.switch_to(cond_up_block);
        let var_val = self.emit_load_var(var_ref, span);
        let to_val = self.emit(InstKind::LoadLocal { local: to_local }, span);
        let cmp_up = self.emit(
            InstKind::BinOp {
                op: IrBinOp::LtEq,
                lhs: var_val,
                rhs: to_val,
            },
            span,
        );
        self.terminate(
            Terminator::BranchIf {
                cond: cmp_up,
                then_block: body_block,
                else_block: exit_block,
            },
            span,
        );

        // Descending check: var >= to
        self.switch_to(cond_down_block);
        let var_val2 = self.emit_load_var(var_ref, span);
        let to_val2 = self.emit(InstKind::LoadLocal { local: to_local }, span);
        let cmp_down = self.emit(
            InstKind::BinOp {
                op: IrBinOp::GtEq,
                lhs: var_val2,
                rhs: to_val2,
            },
            span,
        );
        self.terminate(
            Terminator::BranchIf {
                cond: cmp_down,
                then_block: body_block,
                else_block: exit_block,
            },
            span,
        );

        // Body block.
        self.switch_to(body_block);
        self.lower_loop_body(body, step_block, exit_block, span);

        // Step block: var = var + step
        self.switch_to(step_block);
        let cur_var = self.emit_load_var(var_ref, span);
        let cur_step = self.emit(InstKind::LoadLocal { local: step_local }, span);
        let new_var = self.emit(
            InstKind::BinOp {
                op: IrBinOp::Add,
                lhs: cur_var,
                rhs: cur_step,
            },
            span,
        );
        self.emit_store_var(var_ref, new_var, span);
        self.terminate(Terminator::Goto(cond_check_block), span);

        self.switch_to(exit_block);
    }

    fn lower_for_each(&mut self, var: NodeId, source: NodeId, body: &[NodeId], stmt_id: NodeId) {
        let span = self.arena.span_of(stmt_id);
        let source_ty = self.types.get(source).cloned().unwrap_or(Type::Void);

        // As in `lower_for`, the loop variable always has an allocated slot;
        // a miss is an internal lowering bug.
        let Node::Expr(Expr::Ident { name_span, sigil }) = &self.arena[var] else {
            panic!("For Each loop variable is not an identifier — internal lowering bug");
        };
        let name = self.intern_ident(*name_span, *sigil);
        let var_ref = self.resolve_var(name).unwrap_or_else(|| {
            panic!("For Each loop variable not allocated — internal lowering bug")
        });

        match &source_ty {
            Type::TypeRef { name } => {
                self.lower_for_each_type(*name, var_ref, body, span);
            }
            Type::Array { .. } => {
                self.lower_for_each_array(source, var_ref, body, span);
            }
            _ => {
                // Shouldn't happen after sema, but handle gracefully.
                for &s in body {
                    self.lower_stmt(s);
                }
            }
        }
    }

    fn lower_for_each_type(
        &mut self,
        type_name: Symbol,
        var_ref: VarRef,
        body: &[NodeId],
        span: Span,
    ) {
        // Sema resolves the `For Each` source to a known type; bail gracefully
        // (no loop) on degenerate input rather than indexing a missing key.
        let Some(&type_def) = self.type_def_map.get(&type_name) else {
            debug_assert!(false, "For Each lowered with unknown type {type_name:?}");
            return;
        };

        let cond_block = self.fresh_block();
        let body_block = self.fresh_block();
        let step_block = self.fresh_block();
        let exit_block = self.fresh_block();

        // Init: var = First(T)
        let first = self.emit(InstKind::First { type_def }, span);
        self.emit_store_var(var_ref, first, span);
        self.terminate(Terminator::Goto(cond_block), span);

        // Cond: var != null
        self.switch_to(cond_block);
        let cur = self.emit_load_var(var_ref, span);
        let null = self.emit(InstKind::ConstNull, span);
        let not_null = self.emit(
            InstKind::BinOp {
                op: IrBinOp::NotEq,
                lhs: cur,
                rhs: null,
            },
            span,
        );
        self.terminate(
            Terminator::BranchIf {
                cond: not_null,
                then_block: body_block,
                else_block: exit_block,
            },
            span,
        );

        // Body
        self.switch_to(body_block);
        self.lower_loop_body(body, step_block, exit_block, span);

        // Step: var = Next(var)
        self.switch_to(step_block);
        let cur2 = self.emit_load_var(var_ref, span);
        let next = self.emit(InstKind::Next { object: cur2 }, span);
        self.emit_store_var(var_ref, next, span);
        self.terminate(Terminator::Goto(cond_block), span);

        self.switch_to(exit_block);
    }

    fn lower_for_each_array(
        &mut self,
        source: NodeId,
        var_ref: VarRef,
        body: &[NodeId],
        span: Span,
    ) {
        let cond_block = self.fresh_block();
        let body_block = self.fresh_block();
        let step_block = self.fresh_block();
        let exit_block = self.fresh_block();

        // Allocate unique temps (safe for nested ForEach loops).
        let idx_local = self.alloc_temp("@foreach_idx", IrType::Int);

        // Init: idx = 0, compute total element count. For Each walks the whole
        // array in row-major order regardless of rank (cb_syntax.md §6.3), so
        // the bound is the product of all dimensions, not axis-0's length.
        let arr = self.lower_expr(source);
        let len = self.emit(InstKind::ArrayTotalLen { array: arr }, span);
        let len_local = self.alloc_temp("@foreach_len", IrType::Int);
        self.emit_void(
            InstKind::StoreLocal {
                local: len_local,
                value: len,
            },
            span,
        );

        let zero = self.emit(InstKind::ConstInt(0), span);
        self.emit_void(
            InstKind::StoreLocal {
                local: idx_local,
                value: zero,
            },
            span,
        );
        self.terminate(Terminator::Goto(cond_block), span);

        // Cond: idx < len
        self.switch_to(cond_block);
        let cur_idx = self.emit(InstKind::LoadLocal { local: idx_local }, span);
        let cur_len = self.emit(InstKind::LoadLocal { local: len_local }, span);
        let in_bounds = self.emit(
            InstKind::BinOp {
                op: IrBinOp::Lt,
                lhs: cur_idx,
                rhs: cur_len,
            },
            span,
        );
        self.terminate(
            Terminator::BranchIf {
                cond: in_bounds,
                then_block: body_block,
                else_block: exit_block,
            },
            span,
        );

        // Body: var = arr[flat idx]. A single flat index into the row-major
        // backing store visits elements last-index-fastest for any rank.
        self.switch_to(body_block);
        let idx_for_load = self.emit(InstKind::LoadLocal { local: idx_local }, span);
        let arr_reload = self.lower_expr(source);
        let elem = self.emit(
            InstKind::GetElementFlat {
                array: arr_reload,
                index: idx_for_load,
            },
            span,
        );
        self.emit_store_var(var_ref, elem, span);

        self.lower_loop_body(body, step_block, exit_block, span);

        // Step: idx += 1
        self.switch_to(step_block);
        let old_idx = self.emit(InstKind::LoadLocal { local: idx_local }, span);
        let one = self.emit(InstKind::ConstInt(1), span);
        let new_idx = self.emit(
            InstKind::BinOp {
                op: IrBinOp::Add,
                lhs: old_idx,
                rhs: one,
            },
            span,
        );
        self.emit_void(
            InstKind::StoreLocal {
                local: idx_local,
                value: new_idx,
            },
            span,
        );
        self.terminate(Terminator::Goto(cond_block), span);

        self.switch_to(exit_block);
    }

    fn lower_select(&mut self, scrutinee: NodeId, arms: &[NodeId], stmt_id: NodeId) {
        let span = self.arena.span_of(stmt_id);
        let scrut_reg = self.lower_expr(scrutinee);

        let merge_block = self.fresh_block();

        // Pre-create body blocks for each arm (needed for Continue fall-through).
        let mut arm_bodies: Vec<BlockId> = Vec::new();
        for _ in arms {
            arm_bodies.push(self.fresh_block());
        }

        // The `Default` arm is the dispatch chain's final "else" target; its
        // source position is not significant (cb_syntax.md §6.2). Find its body
        // block (if any); when absent, a no-match falls through to `merge_block`.
        let default_body = arms.iter().enumerate().find_map(|(i, arm_id)| {
            matches!(self.arena[*arm_id], Node::CaseArm(CaseArm::Default { .. }))
                .then(|| arm_bodies[i])
        });
        let no_match = default_body.unwrap_or(merge_block);

        // Build the dispatch chain over the `Case` arms only — each `Case`'s
        // "else" is the next `Case`'s check block, and the last `Case`'s "else"
        // is `no_match`. `Default` deliberately takes no part here.
        let case_indices: Vec<usize> = arms
            .iter()
            .enumerate()
            .filter(|(_, arm_id)| {
                matches!(self.arena[**arm_id], Node::CaseArm(CaseArm::Case { .. }))
            })
            .map(|(i, _)| i)
            .collect();

        let entry_check = self.fresh_block();
        self.terminate(Terminator::Goto(entry_check), span);
        self.switch_to(entry_check);

        if case_indices.is_empty() {
            // No comparisons: jump straight to the default body (or the exit).
            self.terminate(Terminator::Goto(no_match), span);
        } else {
            let mut current_check = entry_check;
            for (k, &arm_idx) in case_indices.iter().enumerate() {
                let last = k + 1 == case_indices.len();
                let else_bb = if last { no_match } else { self.fresh_block() };
                let Node::CaseArm(CaseArm::Case { values, .. }) = self.arena[arms[arm_idx]].clone()
                else {
                    unreachable!("case_indices only holds Case arms");
                };
                self.switch_to(current_check);
                self.lower_case_test(scrut_reg, &values, arm_bodies[arm_idx], else_bb, span);
                current_check = else_bb;
            }
        }

        // Lower every arm body in source order (Case and Default alike) so a
        // `Continue` falls through to the next arm's body regardless of kind.
        for (arm_idx, &arm_id) in arms.iter().enumerate() {
            let next_arm_body = arm_bodies.get(arm_idx + 1).copied();
            let body = match self.arena[arm_id].clone() {
                Node::CaseArm(CaseArm::Case { body, .. }) => body,
                Node::CaseArm(CaseArm::Default { body }) => body,
                _ => continue,
            };
            self.switch_to(arm_bodies[arm_idx]);
            self.context_stack
                .push(ControlContext::Select { next_arm_body });
            for &s in &body {
                self.lower_stmt(s);
            }
            self.context_stack.pop();
            if !self.current_block_is_terminated() {
                self.terminate(Terminator::Goto(merge_block), span);
            }
        }

        self.switch_to(merge_block);
    }

    /// Emit the equality test(s) for one `Case` arm into the current block,
    /// branching to `body_bb` on a match and `else_bb` otherwise. A multi-value
    /// `Case` chains `scrut == v_i` comparisons Or-style: each non-final value
    /// falls through to a fresh block holding the next comparison.
    fn lower_case_test(
        &mut self,
        scrut_reg: Reg,
        values: &[NodeId],
        body_bb: BlockId,
        else_bb: BlockId,
        span: Span,
    ) {
        for (vi, &val_id) in values.iter().enumerate() {
            let val_reg = self.lower_expr(val_id);
            let eq = self.emit(
                InstKind::BinOp {
                    op: IrBinOp::Eq,
                    lhs: scrut_reg,
                    rhs: val_reg,
                },
                span,
            );
            let last = vi + 1 == values.len();
            let next = if last { else_bb } else { self.fresh_block() };
            self.terminate(
                Terminator::BranchIf {
                    cond: eq,
                    then_block: body_bb,
                    else_block: next,
                },
                span,
            );
            if !last {
                self.switch_to(next);
            }
        }
    }
}
