use std::io::Write;
use std::rc::Rc;

use cb_diagnostics::{Interner, Span};
use cb_ir::inst::{InstKind, IrBinOp, IrUnOp, Terminator, TrapKind};
use cb_ir::types::IrType;
use cb_ir::{BlockId, FuncId, FuncKind, Program, Reg};

use crate::error::{InterpError, InterpErrorKind, StackEntry};
use crate::value::{Value, default_value};

type Slot = (Value, bool);
type FrameBuf = (Vec<Value>, Vec<Slot>);

struct Frame {
    #[allow(dead_code)]
    func_id: FuncId,
    body_index: usize,
    registers: Vec<Value>,
    locals: Vec<Slot>,
    current_block: BlockId,
    pc: usize,
    return_reg: Option<Reg>,
}

pub struct Interpreter<'a> {
    program: &'a Program,
    interner: &'a Interner,
    globals: Vec<Slot>,
    call_stack: Vec<Frame>,
    frame_pool: Vec<FrameBuf>,
    stdout: Box<dyn Write + 'a>,
}

impl<'a> Interpreter<'a> {
    pub fn new(program: &'a Program, interner: &'a Interner) -> Self {
        let globals = program
            .globals
            .iter()
            .map(|g| (default_value(&g.ty), false))
            .collect();

        Self {
            program,
            interner,
            globals,
            call_stack: Vec::new(),
            frame_pool: Vec::new(),
            stdout: Box::new(std::io::stdout()),
        }
    }

    pub fn with_stdout(mut self, stdout: Box<dyn Write + 'a>) -> Self {
        self.stdout = stdout;
        self
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

        self.push_frame(main_id, body_index, &[], None);
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
    ) {
        let func = &self.program.functions[body_index];
        let (mut registers, mut locals) = self.frame_pool.pop().unwrap_or_default();
        registers.clear();
        locals.clear();

        for local in &func.locals {
            let val = if local.is_param {
                Value::Void
            } else {
                default_value(&local.ty)
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
    }

    fn exec_loop(&mut self) -> Result<(), InterpError> {
        loop {
            let frame = self.call_stack.last().unwrap();
            let func = &self.program.functions[frame.body_index];
            let block = &func.blocks[frame.current_block.0 as usize];

            if frame.pc < block.insts.len() {
                let inst = block.insts[frame.pc].clone();
                let prev_depth = self.call_stack.len();
                let result = self.exec_inst(&inst.kind, inst.result, inst.span)?;
                // If a user-defined Call pushed a new frame, don't store
                // the result — the Return handler writes it via return_reg.
                let pushed_frame = self.call_stack.len() > prev_depth;
                if !pushed_frame {
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

                let decl = &self.program.func_table[callee.0 as usize];
                match &decl.kind {
                    FuncKind::UserDefined { body_index } => {
                        let body_index = *body_index;
                        self.push_frame(*callee, body_index, &arg_vals, result_reg);
                        Ok(Value::Void)
                    }
                    FuncKind::Runtime { symbol } => {
                        self.call_runtime(symbol, &arg_vals, span)
                    }
                }
            }

            // ── Stubs for Phase 2 ──────────────────────────────────
            InstKind::NewType { .. }
            | InstKind::NewArray { .. }
            | InstKind::GetField { .. }
            | InstKind::SetField { .. }
            | InstKind::GetElement { .. }
            | InstKind::SetElement { .. }
            | InstKind::First { .. }
            | InstKind::Last { .. }
            | InstKind::Next { .. }
            | InstKind::Previous { .. }
            | InstKind::DeleteLvalue { .. }
            | InstKind::DeleteLvalueGlobal { .. }
            | InstKind::DeleteRvalue { .. }
            | InstKind::Redim { .. }
            | InstKind::RedimGlobal { .. }
            | InstKind::CallIndirect { .. }
            | InstKind::Len { .. } => {
                Err(self.error_at(
                    InterpErrorKind::RuntimeError(format!(
                        "instruction not yet implemented: {kind:?}"
                    )),
                    span,
                ))
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

            (Value::Float(a), Value::Float(b)) => self.float_binop(op, *a, *b),

            (Value::String(a), Value::String(b)) => self.string_binop(op, a, b),

            (Value::Bool(a), Value::Bool(b)) => match op {
                IrBinOp::Eq => Ok(Value::Bool(a == b)),
                IrBinOp::NotEq => Ok(Value::Bool(a != b)),
                _ => Ok(Value::Bool(false)),
            },

            // Null comparisons
            (Value::Null, Value::Null) => match op {
                IrBinOp::Eq => Ok(Value::Bool(true)),
                IrBinOp::NotEq => Ok(Value::Bool(false)),
                _ => Ok(Value::Bool(false)),
            },
            (Value::Null, _) | (_, Value::Null) => match op {
                IrBinOp::Eq => Ok(Value::Bool(false)),
                IrBinOp::NotEq => Ok(Value::Bool(true)),
                _ => Ok(Value::Bool(false)),
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
            IrBinOp::Pow => Ok(wrap(a.wrapping_pow(b.unsigned_abs() as u32))),

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

            _ => Ok(Value::Int(0)),
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

            _ => Ok(Value::UInt(0)),
        }
    }

    fn float_binop(&self, op: IrBinOp, a: f64, b: f64) -> Result<Value, InterpError> {
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

            _ => Ok(Value::Float(0.0)),
        }
    }

    fn string_binop(&self, op: IrBinOp, a: &Rc<str>, b: &Rc<str>) -> Result<Value, InterpError> {
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
            _ => Ok(Value::String(Rc::from(""))),
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
            Value::Null => IrType::Null,
            Value::Void => IrType::Void,
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
            _ => Err(self.error_at(
                InterpErrorKind::RuntimeError(format!("unknown runtime function: {symbol}")),
                span,
            )),
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
