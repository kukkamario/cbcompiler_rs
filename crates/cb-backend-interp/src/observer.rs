use cb_diagnostics::Span;
use cb_ir::inst::{InstKind, TrapKind};
use cb_ir::FuncId;

use crate::interp::Frame;
use crate::value::Value;

pub trait Observer {
    fn before_inst(&mut self, _frame: &Frame, _inst: &InstKind, _span: Span) {}
    fn after_inst(&mut self, _frame: &Frame, _inst: &InstKind, _result: &Value, _span: Span) {}
    fn on_call(&mut self, _caller: &Frame, _callee: FuncId, _args: &[Value]) {}
    fn on_return(&mut self, _frame: &Frame, _value: &Value) {}
    fn on_trap(&mut self, _frame: &Frame, _kind: &TrapKind, _span: Span) {}
}

pub struct NoopObserver;

impl Observer for NoopObserver {}
