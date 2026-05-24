use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use cb_diagnostics::{Interner, Span};
use cb_ir::inst::{InstKind, IrBinOp, IrUnOp, Terminator, TrapKind};
use cb_ir::types::IrType;
use cb_ir::{BlockId, FuncId, FuncKind, Program, Reg};

use crate::error::{InterpError, InterpErrorKind, StackEntry};
use crate::heap::{ArrayObj, Slab, TypeInstanceObj, TypeList};
use crate::observer::{NoopObserver, Observer};
use crate::value::{Value, default_value};

pub type Slot = (Value, bool);
type FrameBuf = (Vec<Value>, Vec<Slot>);

const MAX_CALL_DEPTH: usize = 10_000;

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
}

impl<'a> Interpreter<'a, NoopObserver> {
    pub fn new(program: &'a Program, interner: &'a Interner) -> Self {
        let struct_defs = &program.struct_defs;
        let globals = program
            .globals
            .iter()
            .map(|g| (default_value(&g.ty, struct_defs), false))
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
        }
    }

    pub fn run(&mut self) -> Result<(), InterpError> {
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
        self.exec_loop()
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
        for local in &func.locals {
            let val = if local.is_param {
                Value::Void
            } else {
                default_value(&local.ty, struct_defs)
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

    fn exec_loop(&mut self) -> Result<(), InterpError> {
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
                            return Ok(());
                        }

                        if let Some(reg) = return_reg {
                            let caller = self.call_stack.last_mut().unwrap();
                            let idx = reg.0 as usize;
                            if idx >= caller.registers.len() {
                                caller.registers.resize(idx + 1, Value::Void);
                            }
                            caller.registers[idx] = ret_val;
                        }
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
            InstKind::ConstString(v) => Ok(Value::String(Rc::from(v.as_str()))),
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
                    FuncKind::Runtime { symbol } => {
                        self.call_runtime(symbol, &arg_vals, span)
                    }
                }
            }

            // ── Type instance operations ────────────────────────────
            InstKind::NewType { type_def } => {
                let def = &self.program.type_defs[type_def.0 as usize];
                let struct_defs = &self.program.struct_defs;
                let fields = def
                    .fields
                    .iter()
                    .map(|(_, fty)| default_value(fty, struct_defs))
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
            InstKind::SetField { object, field, value } => {
                let frame = self.call_stack.last().unwrap();
                let obj_val = frame.registers[object.0 as usize].clone();
                let new_val = frame.registers[value.0 as usize].clone();
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
                            Some(i) => {
                                self.heap.get_mut(id).expect("set_field: entry must exist").fields[i] = new_val;
                                Ok(Value::Void)
                            }
                            None => Err(self.error_at(
                                InterpErrorKind::RuntimeError(format!(
                                    "field not found: {}",
                                    self.interner.resolve(*field)
                                )),
                                span,
                            )),
                        }
                    }
                    Value::Struct(mut s) => {
                        let idx = s.fields.iter().position(|(f, _)| *f == *field);
                        match idx {
                            Some(i) => {
                                s.fields[i].1 = new_val;
                                let frame = self.call_stack.last_mut().unwrap();
                                frame.registers[object.0 as usize] = Value::Struct(s);
                                Ok(Value::Void)
                            }
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
                        InterpErrorKind::RuntimeError("set_field on non-object".into()),
                        span,
                    )),
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
                let frame = self.call_stack.last().unwrap();
                let dim_sizes: Vec<usize> = dims
                    .iter()
                    .map(|r| self.value_to_i64(&frame.registers[r.0 as usize]) as usize)
                    .collect();
                let arr = ArrayObj::new(dim_sizes, elem_type.clone());
                Ok(Value::Array(Rc::new(RefCell::new(arr))))
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
            InstKind::SetElement { array, indices, value } => {
                let frame = self.call_stack.last().unwrap();
                let arr_val = frame.registers[array.0 as usize].clone();
                let new_val = frame.registers[value.0 as usize].clone();
                let idx_vals: Vec<usize> = indices
                    .iter()
                    .map(|r| self.value_to_i64(&frame.registers[r.0 as usize]) as usize)
                    .collect();
                match arr_val {
                    Value::Array(rc) => {
                        let mut arr = rc.borrow_mut();
                        match arr.flat_index(&idx_vals) {
                            Some(fi) => {
                                arr.data[fi] = new_val;
                                Ok(Value::Void)
                            }
                            None => Err(self.trap_error(TrapKind::IndexOutOfBounds, span)),
                        }
                    }
                    Value::Null => Err(self.trap_error(TrapKind::NullDeref, span)),
                    _ => Err(self.error_at(
                        InterpErrorKind::RuntimeError("set_element on non-array".into()),
                        span,
                    )),
                }
            }
            InstKind::Redim { local, elem_type, dims } => {
                let frame = self.call_stack.last().unwrap();
                let dim_sizes: Vec<usize> = dims
                    .iter()
                    .map(|r| self.value_to_i64(&frame.registers[r.0 as usize]) as usize)
                    .collect();
                let new_arr = ArrayObj::new(dim_sizes, elem_type.clone());
                let new_val = Value::Array(Rc::new(RefCell::new(new_arr)));
                let frame = self.call_stack.last_mut().unwrap();
                frame.locals[local.0 as usize] = (new_val, false);
                Ok(Value::Void)
            }
            InstKind::RedimGlobal { global, elem_type, dims } => {
                let frame = self.call_stack.last().unwrap();
                let dim_sizes: Vec<usize> = dims
                    .iter()
                    .map(|r| self.value_to_i64(&frame.registers[r.0 as usize]) as usize)
                    .collect();
                let new_arr = ArrayObj::new(dim_sizes, elem_type.clone());
                let new_val = Value::Array(Rc::new(RefCell::new(new_arr)));
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
                            FuncKind::Runtime { symbol } => {
                                self.call_runtime(symbol, &arg_vals, span)
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
            IrBinOp::Shl => Ok(wrap(a.wrapping_shl(b as u32))),
            IrBinOp::Shr => Ok(wrap((a as u64).wrapping_shr(b as u32) as i64)),
            IrBinOp::Sar => Ok(wrap(a.wrapping_shr(b as u32))),

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
            IrBinOp::Shl => Ok(wrap(a.wrapping_shl(b as u32))),
            IrBinOp::Shr | IrBinOp::Sar => Ok(wrap(a.wrapping_shr(b as u32))),

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

    fn string_binop(&self, op: IrBinOp, a: &Rc<str>, b: &Rc<str>, span: Span) -> Result<Value, InterpError> {
        match op {
            IrBinOp::StrConcat => {
                let mut s = String::with_capacity(a.len() + b.len());
                s.push_str(a);
                s.push_str(b);
                Ok(Value::String(Rc::from(s.as_str())))
            }
            IrBinOp::StrEq => Ok(Value::Bool(**a == **b)),
            IrBinOp::StrNotEq => Ok(Value::Bool(**a != **b)),
            IrBinOp::StrLt => Ok(Value::Bool(**a < **b)),
            IrBinOp::StrGt => Ok(Value::Bool(**a > **b)),
            IrBinOp::StrLtEq => Ok(Value::Bool(**a <= **b)),
            IrBinOp::StrGtEq => Ok(Value::Bool(**a >= **b)),
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

            (IrUnOp::Plus, _) => Ok(v.clone()),

            (IrUnOp::Not, Value::Bool(x)) => Ok(Value::Bool(!x)),
            (IrUnOp::Not, Value::Int(x)) => Ok(Value::Bool(*x == 0)),

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
        let i = self.value_to_i64(v);
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
            IrType::String => Ok(Value::String(v.as_string())),
            _ => Ok(v.clone()),
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
            Value::String(s) => s.parse().unwrap_or(0),
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
            Value::String(s) => s.parse().unwrap_or(0.0),
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

    fn call_runtime(
        &mut self,
        symbol: &str,
        args: &[Value],
        span: Span,
    ) -> Result<Value, InterpError> {
        match symbol {
            "cb_rt_print" => {
                let text = args.first().map(|v| v.as_string()).unwrap_or_else(|| Rc::from(""));
                write!(self.stdout, "{text}").ok();
                writeln!(self.stdout).ok();
                Ok(Value::Void)
            }
            "cb_rt_abs_int" => {
                let v = args.first().map(|v| self.value_to_i64(v)).unwrap_or(0) as i32;
                Ok(Value::Int(v.wrapping_abs()))
            }
            "cb_rt_abs_float" => {
                let v = args.first().map(|v| self.value_to_f64(v)).unwrap_or(0.0);
                Ok(Value::Float(v.abs()))
            }
            "cb_rt_screen" | "cb_rt_drawscreen" | "cb_rt_color" | "cb_rt_line"
            | "cb_rt_screen_width" | "cb_rt_screen_height" | "cb_rt_mouse_x"
            | "cb_rt_mouse_y" => {
                return self.call_runtime_ffi(symbol, args);
            }
            "cb_rt_create_test_handle" => Ok(Value::OpaqueHandle(42)),
            "cb_rt_use_test_handle" => {
                let h = match args.first() {
                    Some(Value::OpaqueHandle(h)) => *h,
                    Some(Value::Null) => 0,
                    _ => 0,
                };
                Ok(Value::Int(h as i32))
            }
            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!("unknown runtime function: {symbol}")),
                span,
            )),
        }
    }

    #[allow(unsafe_code)]
    fn call_runtime_ffi(&mut self, symbol: &str, args: &[Value]) -> Result<Value, InterpError> {
        match symbol {
            "cb_rt_screen" => {
                let w = args.first().map(|v| self.value_to_i64(v)).unwrap_or(640) as i32;
                let h = args.get(1).map(|v| self.value_to_i64(v)).unwrap_or(480) as i32;
                unsafe { cb_runtime_sys::cb_rt_screen(w, h) };
                Ok(Value::Void)
            }
            "cb_rt_drawscreen" => {
                unsafe { cb_runtime_sys::cb_rt_drawscreen() };
                Ok(Value::Void)
            }
            "cb_rt_color" => {
                let r = args.first().map(|v| self.value_to_i64(v)).unwrap_or(255) as i32;
                let g = args.get(1).map(|v| self.value_to_i64(v)).unwrap_or(255) as i32;
                let b = args.get(2).map(|v| self.value_to_i64(v)).unwrap_or(255) as i32;
                unsafe { cb_runtime_sys::cb_rt_color(r, g, b) };
                Ok(Value::Void)
            }
            "cb_rt_line" => {
                let x1 = args.first().map(|v| self.value_to_f64(v)).unwrap_or(0.0) as f32;
                let y1 = args.get(1).map(|v| self.value_to_f64(v)).unwrap_or(0.0) as f32;
                let x2 = args.get(2).map(|v| self.value_to_f64(v)).unwrap_or(0.0) as f32;
                let y2 = args.get(3).map(|v| self.value_to_f64(v)).unwrap_or(0.0) as f32;
                unsafe { cb_runtime_sys::cb_rt_line(x1, y1, x2, y2) };
                Ok(Value::Void)
            }
            "cb_rt_screen_width" => {
                let w = unsafe { cb_runtime_sys::cb_rt_screen_width() };
                Ok(Value::Int(w))
            }
            "cb_rt_screen_height" => {
                let h = unsafe { cb_runtime_sys::cb_rt_screen_height() };
                Ok(Value::Int(h))
            }
            "cb_rt_mouse_x" => {
                let x = unsafe { cb_runtime_sys::cb_rt_mouse_x() };
                Ok(Value::Int(x))
            }
            "cb_rt_mouse_y" => {
                let y = unsafe { cb_runtime_sys::cb_rt_mouse_y() };
                Ok(Value::Int(y))
            }
            _ => unreachable!(),
        }
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
