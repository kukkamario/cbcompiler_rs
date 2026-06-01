use std::cell::{Cell, RefCell};
use std::io::Write;
use std::rc::Rc;

use cb_diagnostics::{Interner, Span, Symbol};
use cb_ir::inst::{InstKind, IrBinOp, IrUnOp, PlaceRoot, Projection, Terminator, TrapKind};
use cb_ir::types::IrType;
use cb_ir::{BlockId, FuncId, FuncKind, Program, Reg, TypeDefInfo};
use cb_runtime_sys::{CbHostApi, CbString, CbStringApi};

use crate::error::{InterpError, InterpErrorKind, StackEntry};
use crate::heap::{ArrayObj, Slab, TypeInstanceObj, TypeList};
use crate::observer::{NoopObserver, Observer};
use crate::string_handle::CbStringHandle;
use crate::value::{Value, default_value};

pub type Slot = (Value, bool);
type FrameBuf = (Vec<Value>, Vec<Slot>);

const MAX_CALL_DEPTH: usize = 10_000;

// ── Runtime trap channel (FD-015) ──────────────────────────────────────
//
// A runtime (C) function asks the host to terminate cleanly or raise an error
// by calling back through `HOST_API`; the callback records the request in this
// thread-local slot and returns (it must never unwind the C frame). The
// interpreter drains the slot in `call_runtime` immediately after each FFI
// dispatch and routes it through the normal `Result`-up-the-stack path.

enum PendingTrap {
    Exit(i32),
    Error(String),
}

thread_local! {
    static PENDING_TRAP: Cell<Option<PendingTrap>> = const { Cell::new(None) };
}

extern "C" fn host_request_exit(code: i32) {
    PENDING_TRAP.with(|slot| slot.set(Some(PendingTrap::Exit(code))));
}

#[allow(unsafe_code)]
extern "C" fn host_raise_error(msg: *const CbString) {
    // Copy the message bytes into an owned String at the boundary — the
    // CbString argument is only borrowed for the duration of the call.
    let text = if msg.is_null() {
        String::new()
    } else {
        let api = cb_runtime_sys::string_api();
        let len = unsafe { (api.len)(msg) };
        if len == 0 {
            String::new()
        } else {
            let data = unsafe { (api.data)(msg) };
            let bytes = unsafe { std::slice::from_raw_parts(data, len) };
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    PENDING_TRAP.with(|slot| slot.set(Some(PendingTrap::Error(text))));
}

/// The host API handed to the runtime via `cb_runtime_init` at startup.
/// `'static` so the runtime may hold the pointer for the whole program; the
/// callbacks write to the thread-local `PENDING_TRAP` slot.
static HOST_API: CbHostApi = CbHostApi {
    size: std::mem::size_of::<CbHostApi>() as u32,
    abi_version: cb_runtime_sys::CB_CATALOG_VERSION,
    request_exit: host_request_exit,
    raise_error: host_raise_error,
};

pub struct Frame {
    pub func_id: FuncId,
    pub body_index: usize,
    pub registers: Vec<Value>,
    pub locals: Vec<Slot>,
    pub current_block: BlockId,
    pub pc: usize,
    pub return_reg: Option<Reg>,
}

pub struct Interpreter<'a, O: Observer = NoopObserver> {
    program: &'a Program,
    interner: &'a Interner,
    globals: Vec<Slot>,
    call_stack: Vec<Frame>,
    frame_pool: Vec<FrameBuf>,
    heap: Slab,
    type_lists: Vec<TypeList>,
    stdout: Box<dyn Write + 'a>,
    observer: O,
    /// Runtime string API — used to construct CbString handles for
    /// literals/coercions and to dispatch concat. Lives in .rodata of the
    /// loaded runtime library; never moves, never drops.
    string_api: &'static CbStringApi,
}

impl<'a> Interpreter<'a, NoopObserver> {
    pub fn new(program: &'a Program, interner: &'a Interner) -> Self {
        let string_api = cb_runtime_sys::string_api();
        // FD-015: hand the runtime the host trap-channel API once, before any
        // runtime function runs. The returned hooks (about_to_exit) are
        // reserved/unused for now. Clear any stale pending trap on this thread.
        let _ = cb_runtime_sys::runtime_init(&HOST_API);
        PENDING_TRAP.with(|slot| slot.set(None));
        let struct_defs = &program.struct_defs;
        let globals = program
            .globals
            .iter()
            .map(|g| (default_value(&g.ty, struct_defs, string_api), false))
            .collect();

        let mut heap = Slab::new();
        let type_lists = program
            .type_defs
            .iter()
            .enumerate()
            .map(|(i, _)| TypeList::new(&mut heap, cb_ir::TypeDefId(i as u32)))
            .collect();

        Self {
            program,
            interner,
            globals,
            call_stack: Vec::new(),
            frame_pool: Vec::new(),
            heap,
            type_lists,
            stdout: Box::new(std::io::stdout()),
            observer: NoopObserver,
            string_api,
        }
    }
}

impl<'a, O: Observer> Interpreter<'a, O> {
    pub fn with_stdout(mut self, stdout: Box<dyn Write + 'a>) -> Self {
        self.stdout = stdout;
        self
    }

    pub fn with_observer<O2: Observer>(self, observer: O2) -> Interpreter<'a, O2> {
        Interpreter {
            program: self.program,
            interner: self.interner,
            globals: self.globals,
            call_stack: self.call_stack,
            frame_pool: self.frame_pool,
            heap: self.heap,
            type_lists: self.type_lists,
            stdout: self.stdout,
            observer,
            string_api: self.string_api,
        }
    }

    /// Run the program. `Ok(code)` is the process exit code (0 for normal
    /// completion / `End`, the `MakeError` code for an aborting program);
    /// `Err` is a genuine interpreter trap or internal error.
    pub fn run(&mut self) -> Result<i32, InterpError> {
        let main_id = self.find_main()?;
        let body_index = match &self.program.func_table[main_id.0 as usize].kind {
            FuncKind::UserDefined { body_index } => *body_index,
            FuncKind::Runtime { .. } => {
                return Err(self.error(InterpErrorKind::RuntimeError(
                    "@main is not a user-defined function".into(),
                )));
            }
        };

        self.push_frame(main_id, body_index, &[], None)?;
        // FD-015: a runtime `request_exit(code)` surfaces as `Exit(code)` from
        // `call_runtime`; intercept it here and convert to the clean exit code,
        // mirroring the `Terminator::Halt { code } => Ok(code)` path.
        match self.exec_loop() {
            Err(InterpError {
                kind: InterpErrorKind::Exit(code),
                ..
            }) => Ok(code),
            other => other,
        }
    }

    fn find_main(&self) -> Result<FuncId, InterpError> {
        for (i, decl) in self.program.func_table.iter().enumerate().rev() {
            if self.interner.resolve(decl.name) == "@main" {
                return Ok(FuncId(i as u32));
            }
        }
        Err(self.error(InterpErrorKind::RuntimeError(
            "no @main function found".into(),
        )))
    }

    fn push_frame(
        &mut self,
        func_id: FuncId,
        body_index: usize,
        args: &[Value],
        return_reg: Option<Reg>,
    ) -> Result<(), InterpError> {
        if self.call_stack.len() >= MAX_CALL_DEPTH {
            return Err(self.error(InterpErrorKind::RuntimeError(
                format!("stack overflow: call depth exceeded {MAX_CALL_DEPTH}"),
            )));
        }
        let func = &self.program.functions[body_index];
        let (mut registers, mut locals) = self.frame_pool.pop().unwrap_or_default();
        registers.clear();
        locals.clear();

        let struct_defs = &self.program.struct_defs;
        let string_api = self.string_api;
        for local in &func.locals {
            let val = if local.is_param {
                Value::Void
            } else {
                default_value(&local.ty, struct_defs, string_api)
            };
            locals.push((val, false));
        }

        for (i, arg) in args.iter().enumerate() {
            if i < locals.len() {
                locals[i] = (arg.clone(), false);
            }
        }

        self.call_stack.push(Frame {
            func_id,
            body_index,
            registers,
            locals,
            current_block: BlockId(0),
            pc: 0,
            return_reg,
        });
        Ok(())
    }

    fn exec_loop(&mut self) -> Result<i32, InterpError> {
        loop {
            let frame = self.call_stack.last().unwrap();
            let func = &self.program.functions[frame.body_index];
            let block = &func.blocks[frame.current_block.0 as usize];

            if frame.pc < block.insts.len() {
                let inst = block.insts[frame.pc].clone();
                self.observer.before_inst(
                    self.call_stack.last().unwrap(),
                    &inst.kind,
                    inst.span,
                );
                let prev_depth = self.call_stack.len();
                let result = self.exec_inst(&inst.kind, inst.result, inst.span)?;
                // If a user-defined Call pushed a new frame, don't store
                // the result — the Return handler writes it via return_reg.
                let pushed_frame = self.call_stack.len() > prev_depth;
                if !pushed_frame {
                    self.observer.after_inst(
                        self.call_stack.last().unwrap(),
                        &inst.kind,
                        &result,
                        inst.span,
                    );
                    let frame = self.call_stack.last_mut().unwrap();
                    if let Some(reg) = inst.result {
                        let idx = reg.0 as usize;
                        if idx >= frame.registers.len() {
                            frame.registers.resize(idx + 1, Value::Void);
                        }
                        frame.registers[idx] = result;
                    }
                }
                let frame = self.call_stack.last_mut().unwrap();
                // Advance pc in the correct frame (the caller's frame if
                // a call was made, the current frame otherwise).
                if pushed_frame {
                    // The caller frame is second from top. We need to
                    // advance its pc so when the callee returns, execution
                    // continues at the next instruction.
                    let caller_idx = self.call_stack.len() - 2;
                    self.call_stack[caller_idx].pc += 1;
                } else {
                    frame.pc += 1;
                }
            } else {
                let term = block.terminator.clone();
                let term_span = block.terminator_span;
                match term {
                    Some(Terminator::Goto(target)) => {
                        let frame = self.call_stack.last_mut().unwrap();
                        frame.current_block = target;
                        frame.pc = 0;
                    }
                    Some(Terminator::BranchIf {
                        cond,
                        then_block,
                        else_block,
                    }) => {
                        let frame = self.call_stack.last().unwrap();
                        let cond_val = frame.registers[cond.0 as usize].clone();
                        let target = if cond_val.is_truthy() {
                            then_block
                        } else {
                            else_block
                        };
                        let frame = self.call_stack.last_mut().unwrap();
                        frame.current_block = target;
                        frame.pc = 0;
                    }
                    Some(Terminator::Return { value }) => {
                        let frame = self.call_stack.last().unwrap();
                        let ret_val = value
                            .map(|r| frame.registers[r.0 as usize].clone())
                            .unwrap_or(Value::Void);
                        let return_reg = frame.return_reg;

                        self.observer.on_return(
                            self.call_stack.last().unwrap(),
                            &ret_val,
                        );

                        let old_frame = self.call_stack.pop().unwrap();
                        self.frame_pool
                            .push((old_frame.registers, old_frame.locals));

                        if self.call_stack.is_empty() {
                            return Ok(0);
                        }

                        if let Some(reg) = return_reg {
                            let caller = self.call_stack.last_mut().unwrap();
                            let idx = reg.0 as usize;
                            if idx >= caller.registers.len() {
                                caller.registers.resize(idx + 1, Value::Void);
                            }
                            caller.registers[idx] = ret_val.clone();
                        }

                        // Fire the deferred `after_inst` for the call site. When
                        // the Call pushed this frame, `exec_loop` skipped
                        // `after_inst` because the result wasn't known yet; the
                        // caller's pc was advanced past the call, so the call
                        // instruction is at `pc - 1` of its current block.
                        let caller = self.call_stack.last().unwrap();
                        let caller_block =
                            &self.program.functions[caller.body_index].blocks
                                [caller.current_block.0 as usize];
                        if let Some(call_inst) =
                            caller.pc.checked_sub(1).and_then(|i| caller_block.insts.get(i))
                        {
                            self.observer.after_inst(
                                caller,
                                &call_inst.kind,
                                &ret_val,
                                call_inst.span,
                            );
                        }
                    }
                    Some(Terminator::Halt { code }) => {
                        // `End` / `MakeError`: stop the whole program cleanly.
                        // Any side effect (MakeError's stderr message) has
                        // already run as an ordinary instruction in this block.
                        return Ok(code);
                    }
                    Some(Terminator::Trap(kind)) => {
                        self.observer.on_trap(
                            self.call_stack.last().unwrap(),
                            &kind,
                            term_span,
                        );
                        return Err(self.trap_error(kind, term_span));
                    }
                    None => {
                        return Err(self.error(InterpErrorKind::RuntimeError(
                            "unterminated block".into(),
                        )));
                    }
                }
            }
        }
    }

    fn exec_inst(
        &mut self,
        kind: &InstKind,
        result_reg: Option<Reg>,
        span: Span,
    ) -> Result<Value, InterpError> {
        match kind {
            // ── Constants ──────────────────────────────────────────
            InstKind::ConstInt(v) => Ok(Value::Int(*v as i32)),
            InstKind::ConstLong(v) => Ok(Value::Long(*v)),
            InstKind::ConstFloat(v) => Ok(Value::Float(*v)),
            InstKind::ConstBool(v) => Ok(Value::Bool(*v)),
            InstKind::ConstString(v) => Ok(Value::String(CbStringHandle::from_bytes(
                self.string_api,
                v.as_bytes(),
            ))),
            InstKind::ConstNull => Ok(Value::Null),

            // ── Local/Global load/store ────────────────────────────
            InstKind::LoadLocal { local } => {
                let frame = self.call_stack.last().unwrap();
                let (val, _deleted) = &frame.locals[local.0 as usize];
                Ok(val.clone())
            }
            InstKind::StoreLocal { local, value } => {
                let frame = self.call_stack.last().unwrap();
                let val = frame.registers[value.0 as usize].clone();
                let frame = self.call_stack.last_mut().unwrap();
                frame.locals[local.0 as usize] = (val, false);
                Ok(Value::Void)
            }
            InstKind::LoadGlobal { global } => {
                let (val, _deleted) = &self.globals[global.0 as usize];
                Ok(val.clone())
            }
            InstKind::StoreGlobal { global, value } => {
                let frame = self.call_stack.last().unwrap();
                let val = frame.registers[value.0 as usize].clone();
                self.globals[global.0 as usize] = (val, false);
                Ok(Value::Void)
            }

            // ── Arithmetic ─────────────────────────────────────────
            InstKind::BinOp { op, lhs, rhs } => {
                let frame = self.call_stack.last().unwrap();
                let l = frame.registers[lhs.0 as usize].clone();
                let r = frame.registers[rhs.0 as usize].clone();
                self.eval_binop(*op, &l, &r, span)
            }
            InstKind::UnOp { op, operand } => {
                let frame = self.call_stack.last().unwrap();
                let v = frame.registers[operand.0 as usize].clone();
                self.eval_unop(*op, &v, span)
            }

            // ── Type conversions ───────────────────────────────────
            InstKind::Convert { value, from, to } => {
                let frame = self.call_stack.last().unwrap();
                let v = frame.registers[value.0 as usize].clone();
                self.convert_value(&v, from, to)
            }
            InstKind::ConvertExplicit { value, target } => {
                let frame = self.call_stack.last().unwrap();
                let v = frame.registers[value.0 as usize].clone();
                let from = self.value_ir_type(&v);
                self.convert_value(&v, &from, target)
            }

            // ── Function calls ─────────────────────────────────────
            InstKind::Call { callee, args } => {
                let frame = self.call_stack.last().unwrap();
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|r| frame.registers[r.0 as usize].clone())
                    .collect();

                self.observer.on_call(
                    self.call_stack.last().unwrap(),
                    *callee,
                    &arg_vals,
                );

                let decl = &self.program.func_table[callee.0 as usize];
                match &decl.kind {
                    FuncKind::UserDefined { body_index } => {
                        let body_index = *body_index;
                        self.push_frame(*callee, body_index, &arg_vals, result_reg)?;
                        Ok(Value::Void)
                    }
                    FuncKind::Runtime { symbol, fn_ptr } => {
                        let symbol = symbol.clone();
                        let fn_ptr = *fn_ptr;
                        let sig = decl.sig.clone();
                        self.call_runtime(&symbol, fn_ptr, &sig, &arg_vals, span)
                    }
                }
            }

            // ── Type instance operations ────────────────────────────
            InstKind::NewType { type_def } => {
                let def = &self.program.type_defs[type_def.0 as usize];
                let struct_defs = &self.program.struct_defs;
                let string_api = self.string_api;
                let fields = def
                    .fields
                    .iter()
                    .map(|(_, fty)| default_value(fty, struct_defs, string_api))
                    .collect();
                let id = self.heap.alloc(TypeInstanceObj {
                    type_def: *type_def,
                    fields,
                    prev: None,
                    next: None,
                    is_sentinel: false,
                });
                self.type_lists[type_def.0 as usize].append(&mut self.heap, id);
                Ok(Value::TypeInstance(id))
            }
            InstKind::GetField { object, field, .. } => {
                let frame = self.call_stack.last().unwrap();
                let obj_val = frame.registers[object.0 as usize].clone();
                match obj_val {
                    Value::TypeInstance(id) => {
                        let entry = match self.heap.get(id) {
                            Some(e) => e,
                            None => return Err(self.trap_error(TrapKind::DeletedAccess, span)),
                        };
                        if entry.is_sentinel {
                            return Err(self.trap_error(TrapKind::NullDeref, span));
                        }
                        let def = &self.program.type_defs[entry.type_def.0 as usize];
                        let idx = def.fields.iter().position(|(f, _)| *f == *field);
                        match idx {
                            Some(i) => Ok(entry.fields[i].clone()),
                            None => Err(self.error_at(
                                InterpErrorKind::RuntimeError(format!(
                                    "field not found: {}",
                                    self.interner.resolve(*field)
                                )),
                                span,
                            )),
                        }
                    }
                    Value::Struct(s) => {
                        let idx = s.fields.iter().position(|(f, _)| *f == *field);
                        match idx {
                            Some(i) => Ok(s.fields[i].1.clone()),
                            None => Err(self.error_at(
                                InterpErrorKind::RuntimeError(format!(
                                    "field not found: {}",
                                    self.interner.resolve(*field)
                                )),
                                span,
                            )),
                        }
                    }
                    Value::Null => Err(self.trap_error(TrapKind::NullDeref, span)),
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("get_field on non-object".into()),
                        span,
                    )),
                }
            }
            InstKind::StorePlace { root, path, value } => {
                let frame = self.call_stack.last().unwrap();
                let new_val = frame.registers[value.0 as usize].clone();
                // Resolve index registers up front: the in-place walk below
                // holds a mutable borrow of the root slot and cannot also read
                // registers.
                let mut resolved: Vec<RProj> = Vec::with_capacity(path.len());
                for proj in path {
                    match proj {
                        Projection::Field(f) => resolved.push(RProj::Field(*f)),
                        Projection::Index(idxs) => {
                            let vals = idxs
                                .iter()
                                .map(|r| self.value_to_i64(&frame.registers[r.0 as usize]) as usize)
                                .collect();
                            resolved.push(RProj::Index(vals));
                        }
                    }
                }

                // Address the owning slot directly (locals/globals are value
                // storage), then mutate in place. Borrows of the disjoint
                // `heap`, `call_stack`/`globals`, and `program` fields are kept
                // out of `self`-method calls; errors are deferred until the
                // borrows are released.
                let result = {
                    let heap = &mut self.heap;
                    let type_defs = &self.program.type_defs;
                    let slot: &mut Value = match root {
                        PlaceRoot::Local(id) => {
                            &mut self.call_stack.last_mut().unwrap().locals[id.0 as usize].0
                        }
                        PlaceRoot::Global(id) => &mut self.globals[id.0 as usize].0,
                    };
                    store_walk(slot, &resolved, new_val, heap, type_defs)
                };
                match result {
                    Ok(()) => Ok(Value::Void),
                    Err(e) => Err(self.store_err(e, span)),
                }
            }
            InstKind::First { type_def } => {
                let list = &self.type_lists[type_def.0 as usize];
                match list.first(&self.heap) {
                    Some(id) => Ok(Value::TypeInstance(id)),
                    None => Ok(Value::Null),
                }
            }
            InstKind::Last { type_def } => {
                let list = &self.type_lists[type_def.0 as usize];
                match list.tail {
                    Some(id) => Ok(Value::TypeInstance(id)),
                    None => Ok(Value::Null),
                }
            }
            InstKind::Next { object } => {
                let frame = self.call_stack.last().unwrap();
                let obj_val = frame.registers[object.0 as usize].clone();
                match obj_val {
                    Value::TypeInstance(id) => {
                        let entry = match self.heap.get(id) {
                            Some(e) => e,
                            None => return Err(self.trap_error(TrapKind::DeletedAccess, span)),
                        };
                        match entry.next {
                            Some(next_id) => Ok(Value::TypeInstance(next_id)),
                            None => Ok(Value::Null),
                        }
                    }
                    Value::Null => Ok(Value::Null),
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("next on non-type-instance".into()),
                        span,
                    )),
                }
            }
            InstKind::Previous { object } => {
                let frame = self.call_stack.last().unwrap();
                let obj_val = frame.registers[object.0 as usize].clone();
                match obj_val {
                    Value::TypeInstance(id) => {
                        let entry = match self.heap.get(id) {
                            Some(e) => e,
                            None => return Err(self.trap_error(TrapKind::DeletedAccess, span)),
                        };
                        match entry.prev {
                            Some(prev_id) if self.heap.get(prev_id).is_some_and(|e| !e.is_sentinel) => {
                                Ok(Value::TypeInstance(prev_id))
                            }
                            _ => Ok(Value::Null),
                        }
                    }
                    Value::Null => Ok(Value::Null),
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("previous on non-type-instance".into()),
                        span,
                    )),
                }
            }
            InstKind::DeleteLvalue { local } => {
                self.exec_delete_lvalue_slot(local.0 as usize, true, span)
            }
            InstKind::DeleteLvalueGlobal { global } => {
                self.exec_delete_lvalue_slot(global.0 as usize, false, span)
            }
            InstKind::DeleteRvalue { value } => {
                let frame = self.call_stack.last().unwrap();
                let val = frame.registers[value.0 as usize].clone();
                match val {
                    Value::TypeInstance(id) => {
                        let entry = match self.heap.get(id) {
                            Some(e) => e,
                            None => return Err(self.trap_error(TrapKind::DoubleDelete, span)),
                        };
                        let type_def = entry.type_def;
                        self.type_lists[type_def.0 as usize].unlink(&mut self.heap, id);
                        self.heap.free(id);
                        Ok(Value::Void)
                    }
                    Value::Null => Err(self.trap_error(TrapKind::NullDeref, span)),
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("delete on non-type-instance".into()),
                        span,
                    )),
                }
            }

            // ── Array operations ───────────────────────────────────
            InstKind::NewArray { elem_type, dims } => {
                let dim_sizes = self.resolve_dims(dims, span)?;
                self.make_array(dim_sizes, elem_type.clone(), span)
            }
            InstKind::GetElement { array, indices } => {
                let frame = self.call_stack.last().unwrap();
                let arr_val = frame.registers[array.0 as usize].clone();
                let idx_vals: Vec<usize> = indices
                    .iter()
                    .map(|r| self.value_to_i64(&frame.registers[r.0 as usize]) as usize)
                    .collect();
                match arr_val {
                    Value::Array(rc) => {
                        let arr = rc.borrow();
                        match arr.flat_index(&idx_vals) {
                            Some(fi) => Ok(arr.data[fi].clone()),
                            None => Err(self.trap_error(TrapKind::IndexOutOfBounds, span)),
                        }
                    }
                    Value::Null => Err(self.trap_error(TrapKind::NullDeref, span)),
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("index on non-array".into()),
                        span,
                    )),
                }
            }
            InstKind::Redim { local, elem_type, dims } => {
                let dim_sizes = self.resolve_dims(dims, span)?;
                let new_val = self.make_array(dim_sizes, elem_type.clone(), span)?;
                let frame = self.call_stack.last_mut().unwrap();
                frame.locals[local.0 as usize] = (new_val, false);
                Ok(Value::Void)
            }
            InstKind::RedimGlobal { global, elem_type, dims } => {
                let dim_sizes = self.resolve_dims(dims, span)?;
                let new_val = self.make_array(dim_sizes, elem_type.clone(), span)?;
                self.globals[global.0 as usize] = (new_val, false);
                Ok(Value::Void)
            }
            InstKind::Len { array, dim } => {
                let frame = self.call_stack.last().unwrap();
                let arr_val = frame.registers[array.0 as usize].clone();
                let dim_idx = dim.map(|d| self.value_to_i64(&frame.registers[d.0 as usize]) as usize);
                match arr_val {
                    Value::Array(rc) => {
                        let arr = rc.borrow();
                        let len = match dim_idx {
                            None => arr.dim_len(0).unwrap_or(0),
                            Some(d) => arr.dim_len(d).unwrap_or(0),
                        };
                        Ok(Value::Int(len as i32))
                    }
                    Value::Null => Err(self.trap_error(TrapKind::NullDeref, span)),
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("len on non-array".into()),
                        span,
                    )),
                }
            }
            InstKind::StrLen { s } => {
                // Codepoint count of a UTF-8 string: every byte that is not a
                // UTF-8 continuation byte (0b10xxxxxx) begins a new codepoint.
                // Computed here in Rust as the reference impl; the future LLVM
                // backend will instead emit a runtime char-length call.
                let frame = self.call_stack.last().unwrap();
                let val = frame.registers[s.0 as usize].clone();
                match val {
                    Value::String(handle) => {
                        let count = handle
                            .as_bytes()
                            .iter()
                            .filter(|b| (*b & 0xC0) != 0x80)
                            .count();
                        Ok(Value::Int(count as i32))
                    }
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("strlen on non-string".into()),
                        span,
                    )),
                }
            }

            // ── Indirect calls ─────────────────────────────────────
            InstKind::CallIndirect { callee, args } => {
                let frame = self.call_stack.last().unwrap();
                let callee_val = frame.registers[callee.0 as usize].clone();
                match callee_val {
                    Value::FnPtr(Some(func_id)) => {
                        let arg_vals: Vec<Value> = args
                            .iter()
                            .map(|r| frame.registers[r.0 as usize].clone())
                            .collect();

                        self.observer.on_call(
                            self.call_stack.last().unwrap(),
                            func_id,
                            &arg_vals,
                        );

                        let decl = &self.program.func_table[func_id.0 as usize];
                        match &decl.kind {
                            FuncKind::UserDefined { body_index } => {
                                let body_index = *body_index;
                                self.push_frame(func_id, body_index, &arg_vals, result_reg)?;
                                Ok(Value::Void)
                            }
                            FuncKind::Runtime { symbol, fn_ptr } => {
                                let symbol = symbol.clone();
                                let fn_ptr = *fn_ptr;
                                let sig = decl.sig.clone();
                                self.call_runtime(&symbol, fn_ptr, &sig, &arg_vals, span)
                            }
                        }
                    }
                    Value::FnPtr(None) | Value::Null => {
                        Err(self.trap_error(TrapKind::NullFnPtr, span))
                    }
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("call_indirect on non-function-pointer".into()),
                        span,
                    )),
                }
            }
        }
    }

    /// Resolve dimension registers to concrete sizes, rejecting negative
    /// dimensions with a clean error. Without this a negative size (`New
    /// Int[-1]`) would wrap to a huge `usize` and abort the process on
    /// allocation.
    fn resolve_dims(&self, dims: &[Reg], span: Span) -> Result<Vec<usize>, InterpError> {
        let frame = self.call_stack.last().unwrap();
        let mut sizes = Vec::with_capacity(dims.len());
        for r in dims {
            let n = self.value_to_i64(&frame.registers[r.0 as usize]);
            if n < 0 {
                return Err(self.error_at(
                    InterpErrorKind::RuntimeError(format!("negative array dimension: {n}")),
                    span,
                ));
            }
            sizes.push(n as usize);
        }
        Ok(sizes)
    }

    /// Map a deferred place-walk error to a concrete interpreter error/trap,
    /// matching the diagnostics the old `SetField`/`SetElement` produced.
    fn store_err(&self, e: StoreErr, span: Span) -> InterpError {
        match e {
            StoreErr::NoField(f) => self.error_at(
                InterpErrorKind::RuntimeError(format!(
                    "field not found: {}",
                    self.interner.resolve(f)
                )),
                span,
            ),
            StoreErr::Null => self.trap_error(TrapKind::NullDeref, span),
            StoreErr::Deleted => self.trap_error(TrapKind::DeletedAccess, span),
            StoreErr::OutOfBounds => self.trap_error(TrapKind::IndexOutOfBounds, span),
            StoreErr::NotStruct => self.error_at(
                InterpErrorKind::RuntimeError("set_field on non-object".into()),
                span,
            ),
            StoreErr::NotArray => self.error_at(
                InterpErrorKind::RuntimeError("index on non-array".into()),
                span,
            ),
        }
    }

    /// Allocate an array, turning an over-large request into a clean
    /// `RuntimeError` instead of an allocation abort.
    fn make_array(
        &self,
        dims: Vec<usize>,
        elem_type: IrType,
        span: Span,
    ) -> Result<Value, InterpError> {
        match ArrayObj::new(dims, elem_type, &self.program.struct_defs, self.string_api) {
            Ok(arr) => Ok(Value::Array(Rc::new(RefCell::new(arr)))),
            Err(_) => Err(self.error_at(
                InterpErrorKind::RuntimeError("array too large to allocate".into()),
                span,
            )),
        }
    }

    fn eval_binop(
        &self,
        op: IrBinOp,
        lhs: &Value,
        rhs: &Value,
        span: Span,
    ) -> Result<Value, InterpError> {
        match (lhs, rhs) {
            (Value::Int(a), Value::Int(b)) => self.int_binop(op, *a as i64, *b as i64, span, false),
            (Value::Long(a), Value::Long(b)) => self.int_binop(op, *a, *b, span, true),
            (Value::Byte(a), Value::Byte(b)) => self.int_binop(op, *a as i64, *b as i64, span, false),
            (Value::Short(a), Value::Short(b)) => self.int_binop(op, *a as i64, *b as i64, span, false),
            (Value::UInt(a), Value::UInt(b)) => self.uint_binop(op, *a as u64, *b as u64, span, false),
            (Value::ULong(a), Value::ULong(b)) => self.uint_binop(op, *a, *b, span, true),

            (Value::Float(a), Value::Float(b)) => self.float_binop(op, *a, *b, span),

            (Value::String(a), Value::String(b)) => self.string_binop(op, a, b, span),

            (Value::Bool(a), Value::Bool(b)) => match op {
                IrBinOp::Eq => Ok(Value::Bool(a == b)),
                IrBinOp::NotEq => Ok(Value::Bool(a != b)),
                _ => Err(self.error_at(
                    InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on booleans")),
                    span,
                )),
            },

            // Type instance identity comparison
            (Value::TypeInstance(a), Value::TypeInstance(b)) => match op {
                IrBinOp::Eq => Ok(Value::Bool(a == b)),
                IrBinOp::NotEq => Ok(Value::Bool(a != b)),
                _ => Err(self.error_at(
                    InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on type instances")),
                    span,
                )),
            },

            // Opaque handle identity comparison
            (Value::OpaqueHandle(a), Value::OpaqueHandle(b)) => match op {
                IrBinOp::Eq => Ok(Value::Bool(a == b)),
                IrBinOp::NotEq => Ok(Value::Bool(a != b)),
                _ => Err(self.error_at(
                    InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on opaque handles")),
                    span,
                )),
            },

            // Null comparisons
            (Value::Null, Value::Null) => match op {
                IrBinOp::Eq => Ok(Value::Bool(true)),
                IrBinOp::NotEq => Ok(Value::Bool(false)),
                _ => Err(self.error_at(
                    InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on null values")),
                    span,
                )),
            },
            (Value::Null, _) | (_, Value::Null) => match op {
                IrBinOp::Eq => Ok(Value::Bool(false)),
                IrBinOp::NotEq => Ok(Value::Bool(true)),
                _ => Err(self.error_at(
                    InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on null and non-null values")),
                    span,
                )),
            },

            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!(
                    "type mismatch in binop: {lhs:?} {op:?} {rhs:?}"
                )),
                span,
            )),
        }
    }

    fn int_binop(
        &self,
        op: IrBinOp,
        a: i64,
        b: i64,
        span: Span,
        wide: bool,
    ) -> Result<Value, InterpError> {
        let wrap = |v: i64| -> Value {
            if wide { Value::Long(v) } else { Value::Int(v as i32) }
        };

        match op {
            IrBinOp::Add => Ok(wrap(a.wrapping_add(b))),
            IrBinOp::Sub => Ok(wrap(a.wrapping_sub(b))),
            IrBinOp::Mul => Ok(wrap(a.wrapping_mul(b))),
            IrBinOp::Div | IrBinOp::IntDiv => {
                if b == 0 {
                    return Err(self.trap_error(TrapKind::DivisionByZero, span));
                }
                Ok(wrap(a.wrapping_div(b)))
            }
            IrBinOp::Mod => {
                if b == 0 {
                    return Err(self.trap_error(TrapKind::DivisionByZero, span));
                }
                Ok(wrap(a.wrapping_rem(b)))
            }
            IrBinOp::Pow => {
                if b < 0 {
                    match a {
                        0 => Err(self.trap_error(TrapKind::DivisionByZero, span)),
                        1 => Ok(wrap(1)),
                        -1 => Ok(wrap(if b % 2 == 0 { 1 } else { -1 })),
                        _ => Ok(wrap(0)),
                    }
                } else {
                    Ok(wrap(a.wrapping_pow(b as u32)))
                }
            }

            IrBinOp::BinAnd => Ok(wrap(a & b)),
            IrBinOp::BinOr => Ok(wrap(a | b)),
            IrBinOp::BinXor => Ok(wrap(a ^ b)),
            // Shifts operate at the operand's actual bit width (32 unless
            // `wide`), and the count is reduced modulo that width (x86-style).
            // `Shr` is logical (zero-extend), `Sar` is arithmetic (sign-extend)
            // — see cb_syntax.md §`Shr`.
            IrBinOp::Shl => {
                if wide {
                    Ok(wrap(a.wrapping_shl((b as u32) & 63)))
                } else {
                    Ok(wrap((a as i32).wrapping_shl((b as u32) & 31) as i64))
                }
            }
            IrBinOp::Shr => {
                if wide {
                    Ok(wrap((a as u64).wrapping_shr((b as u32) & 63) as i64))
                } else {
                    Ok(wrap((a as u32).wrapping_shr((b as u32) & 31) as i32 as i64))
                }
            }
            IrBinOp::Sar => {
                if wide {
                    Ok(wrap(a.wrapping_shr((b as u32) & 63)))
                } else {
                    Ok(wrap((a as i32).wrapping_shr((b as u32) & 31) as i64))
                }
            }

            IrBinOp::Eq => Ok(Value::Bool(a == b)),
            IrBinOp::NotEq => Ok(Value::Bool(a != b)),
            IrBinOp::Lt => Ok(Value::Bool(a < b)),
            IrBinOp::Gt => Ok(Value::Bool(a > b)),
            IrBinOp::LtEq => Ok(Value::Bool(a <= b)),
            IrBinOp::GtEq => Ok(Value::Bool(a >= b)),

            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on integers")),
                span,
            )),
        }
    }

    fn uint_binop(
        &self,
        op: IrBinOp,
        a: u64,
        b: u64,
        span: Span,
        wide: bool,
    ) -> Result<Value, InterpError> {
        let wrap = |v: u64| -> Value {
            if wide { Value::ULong(v) } else { Value::UInt(v as u32) }
        };

        match op {
            IrBinOp::Add => Ok(wrap(a.wrapping_add(b))),
            IrBinOp::Sub => Ok(wrap(a.wrapping_sub(b))),
            IrBinOp::Mul => Ok(wrap(a.wrapping_mul(b))),
            IrBinOp::Div | IrBinOp::IntDiv => {
                if b == 0 {
                    return Err(self.trap_error(TrapKind::DivisionByZero, span));
                }
                Ok(wrap(a / b))
            }
            IrBinOp::Mod => {
                if b == 0 {
                    return Err(self.trap_error(TrapKind::DivisionByZero, span));
                }
                Ok(wrap(a % b))
            }
            IrBinOp::Pow => Ok(wrap(a.wrapping_pow(b as u32))),

            IrBinOp::BinAnd => Ok(wrap(a & b)),
            IrBinOp::BinOr => Ok(wrap(a | b)),
            IrBinOp::BinXor => Ok(wrap(a ^ b)),
            // Width-correct, count reduced modulo the operand width. For an
            // unsigned operand `Sar` is identical to the logical `Shr`.
            IrBinOp::Shl => {
                if wide {
                    Ok(wrap(a.wrapping_shl((b as u32) & 63)))
                } else {
                    Ok(wrap((a as u32).wrapping_shl((b as u32) & 31) as u64))
                }
            }
            IrBinOp::Shr | IrBinOp::Sar => {
                if wide {
                    Ok(wrap(a.wrapping_shr((b as u32) & 63)))
                } else {
                    Ok(wrap((a as u32).wrapping_shr((b as u32) & 31) as u64))
                }
            }

            IrBinOp::Eq => Ok(Value::Bool(a == b)),
            IrBinOp::NotEq => Ok(Value::Bool(a != b)),
            IrBinOp::Lt => Ok(Value::Bool(a < b)),
            IrBinOp::Gt => Ok(Value::Bool(a > b)),
            IrBinOp::LtEq => Ok(Value::Bool(a <= b)),
            IrBinOp::GtEq => Ok(Value::Bool(a >= b)),

            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on unsigned integers")),
                span,
            )),
        }
    }

    fn float_binop(&self, op: IrBinOp, a: f64, b: f64, span: Span) -> Result<Value, InterpError> {
        match op {
            IrBinOp::Add => Ok(Value::Float(a + b)),
            IrBinOp::Sub => Ok(Value::Float(a - b)),
            IrBinOp::Mul => Ok(Value::Float(a * b)),
            IrBinOp::Div => Ok(Value::Float(a / b)),
            IrBinOp::IntDiv => Ok(Value::Float((a / b).trunc())),
            IrBinOp::Mod => Ok(Value::Float(a % b)),
            IrBinOp::Pow => Ok(Value::Float(a.powf(b))),

            IrBinOp::Eq => Ok(Value::Bool(a == b)),
            IrBinOp::NotEq => Ok(Value::Bool(a != b)),
            IrBinOp::Lt => Ok(Value::Bool(a < b)),
            IrBinOp::Gt => Ok(Value::Bool(a > b)),
            IrBinOp::LtEq => Ok(Value::Bool(a <= b)),
            IrBinOp::GtEq => Ok(Value::Bool(a >= b)),

            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on floats")),
                span,
            )),
        }
    }

    fn string_binop(
        &self,
        op: IrBinOp,
        a: &CbStringHandle,
        b: &CbStringHandle,
        span: Span,
    ) -> Result<Value, InterpError> {
        match op {
            IrBinOp::StrConcat => Ok(Value::String(a.concat(b))),
            IrBinOp::StrEq => Ok(Value::Bool(a.as_bytes() == b.as_bytes())),
            IrBinOp::StrNotEq => Ok(Value::Bool(a.as_bytes() != b.as_bytes())),
            IrBinOp::StrLt => Ok(Value::Bool(a.as_bytes() < b.as_bytes())),
            IrBinOp::StrGt => Ok(Value::Bool(a.as_bytes() > b.as_bytes())),
            IrBinOp::StrLtEq => Ok(Value::Bool(a.as_bytes() <= b.as_bytes())),
            IrBinOp::StrGtEq => Ok(Value::Bool(a.as_bytes() >= b.as_bytes())),
            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!("invalid binop: {op:?} on strings")),
                span,
            )),
        }
    }

    fn eval_unop(&self, op: IrUnOp, v: &Value, span: Span) -> Result<Value, InterpError> {
        match (op, v) {
            (IrUnOp::Neg, Value::Int(x)) => Ok(Value::Int(x.wrapping_neg())),
            (IrUnOp::Neg, Value::Long(x)) => Ok(Value::Long(x.wrapping_neg())),
            (IrUnOp::Neg, Value::Float(x)) => Ok(Value::Float(-x)),
            (IrUnOp::Neg, Value::Short(x)) => Ok(Value::Short(x.wrapping_neg())),
            (IrUnOp::Neg, Value::Byte(x)) => Ok(Value::Byte(x.wrapping_neg())),
            (IrUnOp::Neg, Value::UInt(x)) => Ok(Value::UInt(x.wrapping_neg())),
            (IrUnOp::Neg, Value::ULong(x)) => Ok(Value::ULong(x.wrapping_neg())),

            (IrUnOp::Plus, _) => Ok(v.clone()),

            // Logical NOT is defined for booleans and every integer width via
            // truthiness, so we don't rely on sema always pre-converting.
            (
                IrUnOp::Not,
                Value::Bool(_)
                | Value::Int(_)
                | Value::Long(_)
                | Value::Byte(_)
                | Value::Short(_)
                | Value::UInt(_)
                | Value::ULong(_),
            ) => Ok(Value::Bool(!v.is_truthy())),

            (IrUnOp::BinNot, Value::Int(x)) => Ok(Value::Int(!x)),
            (IrUnOp::BinNot, Value::Long(x)) => Ok(Value::Long(!x)),
            (IrUnOp::BinNot, Value::Byte(x)) => Ok(Value::Byte(!x)),
            (IrUnOp::BinNot, Value::Short(x)) => Ok(Value::Short(!x)),
            (IrUnOp::BinNot, Value::UInt(x)) => Ok(Value::UInt(!x)),
            (IrUnOp::BinNot, Value::ULong(x)) => Ok(Value::ULong(!x)),

            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!("invalid unop: {op:?} on {v:?}")),
                span,
            )),
        }
    }

    fn convert_value(
        &self,
        v: &Value,
        from: &IrType,
        to: &IrType,
    ) -> Result<Value, InterpError> {
        // Language-level conversion to an integer type uses CoolBasic's
        // `toInt` semantics (cb_runtime.md §Math): a Float is rounded half-up
        // (`(int)(x + 0.5)`), not truncated toward zero, and a String is
        // parsed as a leading integer after trimming. The raw `value_to_i64`
        // helper (truncating) stays for internal, non-language uses.
        let i = self.value_to_int_cb(v);
        let f = self.value_to_f64(v);

        match to {
            IrType::Byte => Ok(Value::Byte(i as u8)),
            IrType::Short => Ok(Value::Short(i as i16)),
            IrType::Int => Ok(Value::Int(i as i32)),
            IrType::UInt => Ok(Value::UInt(i as u32)),
            IrType::Long => Ok(Value::Long(i)),
            IrType::ULong => Ok(Value::ULong(i as u64)),
            IrType::Float => {
                if from.is_integer() {
                    Ok(Value::Float(i as f64))
                } else {
                    Ok(Value::Float(f))
                }
            }
            IrType::Bool => Ok(Value::Bool(v.is_truthy())),
            IrType::String => Ok(Value::String(v.as_cb_string(self.string_api))),
            _ => Ok(v.clone()),
        }
    }

    /// CoolBasic `toInt` conversion: Float rounds half-up (`(int)(x + 0.5)`,
    /// truncating toward zero afterward), String trims then parses a leading
    /// integer (0 on no leading digits), everything else matches `value_to_i64`.
    fn value_to_int_cb(&self, v: &Value) -> i64 {
        match v {
            Value::Float(x) => (*x + 0.5) as i64,
            Value::String(s) => parse_leading_int(s.as_bytes()),
            _ => self.value_to_i64(v),
        }
    }

    fn value_to_i64(&self, v: &Value) -> i64 {
        match v {
            Value::Byte(x) => *x as i64,
            Value::Short(x) => *x as i64,
            Value::Int(x) => *x as i64,
            Value::UInt(x) => *x as i64,
            Value::Long(x) => *x,
            Value::ULong(x) => *x as i64,
            Value::Float(x) => *x as i64,
            Value::Bool(true) => 1,
            Value::Bool(false) => 0,
            // Same leading-integer policy as CoolBasic `toInt`
            // (`value_to_int_cb`): `"3x"` → 3, not 0. Keeps the one
            // string→int rule used everywhere (array indices/dims included).
            Value::String(s) => parse_leading_int(s.as_bytes()),
            _ => 0,
        }
    }

    fn value_to_f64(&self, v: &Value) -> f64 {
        match v {
            Value::Byte(x) => *x as f64,
            Value::Short(x) => *x as f64,
            Value::Int(x) => *x as f64,
            Value::UInt(x) => *x as f64,
            Value::Long(x) => *x as f64,
            Value::ULong(x) => *x as f64,
            Value::Float(x) => *x,
            Value::Bool(true) => 1.0,
            Value::Bool(false) => 0.0,
            // Floats keep the strict full-parse policy (a partial parse of a
            // float prefix has no documented CB semantics); a non-numeric
            // string yields 0.0.
            Value::String(s) => std::str::from_utf8(s.as_bytes())
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            _ => 0.0,
        }
    }

    fn value_ir_type(&self, v: &Value) -> IrType {
        match v {
            Value::Byte(_) => IrType::Byte,
            Value::Short(_) => IrType::Short,
            Value::Int(_) => IrType::Int,
            Value::UInt(_) => IrType::UInt,
            Value::Long(_) => IrType::Long,
            Value::ULong(_) => IrType::ULong,
            Value::Float(_) => IrType::Float,
            Value::Bool(_) => IrType::Bool,
            Value::String(_) => IrType::String,
            Value::Array(_) => IrType::Null,
            Value::TypeInstance(_) => IrType::Null,
            Value::Struct(_) => IrType::Null,
            Value::FnPtr(_) => IrType::Null,
            Value::OpaqueHandle(_) => IrType::Null,
            Value::Null => IrType::Null,
            Value::Void => IrType::Void,
        }
    }

    // ── Delete lvalue helper ────────────────────────────────────────────

    fn exec_delete_lvalue_slot(
        &mut self,
        slot_idx: usize,
        is_local: bool,
        span: Span,
    ) -> Result<Value, InterpError> {
        let slot = if is_local {
            &self.call_stack.last().unwrap().locals[slot_idx]
        } else {
            &self.globals[slot_idx]
        };

        let (val, deleted) = slot;
        if *deleted {
            return Err(self.trap_error(TrapKind::DoubleDelete, span));
        }

        match val {
            Value::TypeInstance(id) => {
                let id = *id;
                let entry = match self.heap.get(id) {
                    Some(e) => e,
                    None => return Err(self.trap_error(TrapKind::DoubleDelete, span)),
                };

                let prev = entry.prev.unwrap_or(
                    self.type_lists[entry.type_def.0 as usize].sentinel,
                );
                let type_def = entry.type_def;

                self.type_lists[type_def.0 as usize].unlink(&mut self.heap, id);
                self.heap.free(id);

                let new_slot = (Value::TypeInstance(prev), true);
                if is_local {
                    self.call_stack.last_mut().unwrap().locals[slot_idx] = new_slot;
                } else {
                    self.globals[slot_idx] = new_slot;
                }
                Ok(Value::Void)
            }
            Value::Null => Err(self.trap_error(TrapKind::NullDeref, span)),
            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError("delete on non-type-instance".into()),
                span,
            )),
        }
    }

    // ── Runtime function dispatch ──────────────────────────────────────

    /// Dispatch a runtime call. Most functions go through libffi using the
    /// catalog-supplied `fn_ptr` and the IR signature — no per-function
    /// interpreter code required. A small intrinsic-override table handles
    /// the few cases where the interpreter wants privileged behavior
    /// (currently just `cb_rt_print`, which writes through `self.stdout`
    /// so integration tests can capture output).
    #[allow(unsafe_code)]
    fn call_runtime(
        &mut self,
        symbol: &str,
        fn_ptr: unsafe extern "C" fn(),
        sig: &cb_ir::FnSig,
        args: &[Value],
        span: Span,
    ) -> Result<Value, InterpError> {
        // Intrinsic overrides — keep this set small. Each entry is a
        // deliberate decision that the interpreter needs to handle this
        // function differently from a plain FFI dispatch.
        if symbol == "cb_rt_print" {
            if let Some(Value::String(h)) = args.first() {
                self.stdout.write_all(h.as_bytes()).ok();
            }
            writeln!(self.stdout).ok();
            return Ok(Value::Void);
        }

        // General path: libffi dispatch using the catalog fn_ptr + IR sig.
        let ret = unsafe { crate::ffi::call(fn_ptr, sig, args, self.string_api) };

        // FD-015: drain any trap the runtime recorded during the call. The
        // callback returned normally (never unwinds), so we route the request
        // through the Result chain here, at the single FFI chokepoint.
        if let Some(pending) = PENDING_TRAP.with(|slot| slot.take()) {
            return match pending {
                PendingTrap::Exit(code) => Err(self.error(InterpErrorKind::Exit(code))),
                PendingTrap::Error(msg) => {
                    if let Some(frame) = self.call_stack.last() {
                        self.observer.on_runtime_error(frame, &msg, span);
                    }
                    Err(self.error_at(InterpErrorKind::RuntimeError(msg), span))
                }
            };
        }
        Ok(ret)
    }

    // ── Error helpers ──────────────────────────────────────────────────

    fn error(&self, kind: InterpErrorKind) -> InterpError {
        InterpError {
            kind,
            stack_trace: self.build_stack_trace(),
        }
    }

    fn error_at(&self, kind: InterpErrorKind, span: Span) -> InterpError {
        let mut trace = self.build_stack_trace();
        if let Some(entry) = trace.first_mut() {
            entry.span = span;
        }
        InterpError {
            kind,
            stack_trace: trace,
        }
    }

    fn trap_error(&self, kind: TrapKind, span: Span) -> InterpError {
        self.error_at(InterpErrorKind::Trap(kind), span)
    }

    fn build_stack_trace(&self) -> Vec<StackEntry> {
        self.call_stack
            .iter()
            .rev()
            .map(|frame| {
                let func = &self.program.functions[frame.body_index];
                let block = &func.blocks[frame.current_block.0 as usize];
                let span = if frame.pc < block.insts.len() {
                    block.insts[frame.pc].span
                } else {
                    block.terminator_span
                };
                StackEntry {
                    func_name: func.name,
                    span,
                }
            })
            .collect()
    }
}

/// Parse a leading integer from UTF-8 bytes, mirroring `stoi(trim(s))`: skip
/// leading ASCII whitespace, accept an optional `+`/`-`, then consume digits
/// up to the first non-digit. Returns 0 when no digits lead (matching
/// cbEnchanted's `try { stoi } catch { 0 }`). Saturates rather than wrapping.
fn parse_leading_int(bytes: &[u8]) -> i64 {
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut neg = false;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        neg = bytes[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut val: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        val = val
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as i64);
        i += 1;
    }
    if i == start {
        return 0; // no leading digits
    }
    if neg { -val } else { val }
}

// ── StorePlace path walking ─────────────────────────────────────────────

/// A [`Projection`] with its index registers already resolved to concrete
/// sizes, so the in-place store walk needs no register access.
enum RProj {
    Field(Symbol),
    Index(Vec<usize>),
}

/// Failure encountered while walking a [`InstKind::StorePlace`] path. Mapped
/// to a concrete interpreter error by [`Interpreter::store_err`] once the
/// borrows held during the walk are released.
enum StoreErr {
    NoField(Symbol),
    Null,
    Deleted,
    OutOfBounds,
    NotStruct,
    NotArray,
}

/// Walk `projs` from `slot` toward the target location and write `value`
/// there, mutating in place. Value-type structs are mutated through the slot;
/// arrays and type-instances are reference types mutated through their shared
/// handles (so the change persists regardless of the owning slot).
fn store_walk(
    slot: &mut Value,
    projs: &[RProj],
    value: Value,
    heap: &mut Slab,
    type_defs: &[TypeDefInfo],
) -> Result<(), StoreErr> {
    let (proj, rest) = match projs.split_first() {
        Some(split) => split,
        None => {
            *slot = value;
            return Ok(());
        }
    };

    match proj {
        RProj::Field(f) => match slot {
            Value::Struct(s) => {
                let i = s
                    .fields
                    .iter()
                    .position(|(fld, _)| fld == f)
                    .ok_or(StoreErr::NoField(*f))?;
                store_walk(&mut s.fields[i].1, rest, value, heap, type_defs)
            }
            Value::TypeInstance(id) => {
                let id = *id;
                let entry = heap.get(id).ok_or(StoreErr::Deleted)?;
                if entry.is_sentinel {
                    return Err(StoreErr::Null);
                }
                let def = &type_defs[entry.type_def.0 as usize];
                let i = def
                    .fields
                    .iter()
                    .position(|(fld, _)| fld == f)
                    .ok_or(StoreErr::NoField(*f))?;
                if rest.is_empty() {
                    heap.get_mut(id).ok_or(StoreErr::Deleted)?.fields[i] = value;
                    Ok(())
                } else {
                    // Take the field out so the recursion can borrow `heap`
                    // freely for deeper reference steps, then put it back.
                    let mut taken = std::mem::replace(
                        &mut heap.get_mut(id).ok_or(StoreErr::Deleted)?.fields[i],
                        Value::Void,
                    );
                    let r = store_walk(&mut taken, rest, value, heap, type_defs);
                    heap.get_mut(id).ok_or(StoreErr::Deleted)?.fields[i] = taken;
                    r
                }
            }
            Value::Null => Err(StoreErr::Null),
            _ => Err(StoreErr::NotStruct),
        },
        RProj::Index(idxs) => match slot {
            Value::Array(rc) => {
                // Arrays are reference types: clone the handle (refcount bump)
                // to release the slot borrow, then mutate the shared backing.
                let rc = rc.clone();
                let mut arr = rc.borrow_mut();
                let fi = arr.flat_index(idxs).ok_or(StoreErr::OutOfBounds)?;
                store_walk(&mut arr.data[fi], rest, value, heap, type_defs)
            }
            Value::Null => Err(StoreErr::Null),
            _ => Err(StoreErr::NotArray),
        },
    }
}
