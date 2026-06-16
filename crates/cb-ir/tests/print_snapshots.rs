//! Snapshot coverage for the `--dump-ir` text printer (`cb_ir::print`).
//!
//! These build small `Program`s by hand and snapshot `print_program` so that
//! every `InstKind`, `Terminator` (including each `TrapKind` label), `IrType`,
//! `IrBinOp`, and `IrUnOp` arm has direct coverage in `cb-ir` — independent of
//! the incidental coverage from `cb-sema` lowering snapshots.

use cb_diagnostics::source::FileId;
use cb_diagnostics::{Interner, Span, Symbol};

use cb_ir::print::print_program;
use cb_ir::{
    BasicBlock, BlockId, FuncDecl, FuncId, FuncKind, Function, Global, GlobalId, Inst, InstKind,
    IrBinOp, IrUnOp, Local, LocalId, PlaceRoot, Program, Projection, Reg, Terminator, TrapKind,
    TypeDefId, TypeDefInfo,
};
use cb_ir::{FnSig, IrType};

const SPAN: Span = Span::new(0, 0, FileId::SYNTHETIC);

// ── Tiny constructors to keep the test bodies readable ──────────────────

fn inst(result: Option<u32>, kind: InstKind) -> Inst {
    Inst {
        result: result.map(Reg),
        kind,
        span: SPAN,
    }
}

fn block(id: u32, insts: Vec<Inst>, term: Terminator) -> BasicBlock {
    BasicBlock {
        id: BlockId(id),
        insts,
        terminator: Some(term),
        terminator_span: SPAN,
    }
}

fn func(
    name: Symbol,
    params: Vec<IrType>,
    ret: IrType,
    locals: Vec<Local>,
    blocks: Vec<BasicBlock>,
) -> Function {
    Function {
        name,
        params,
        return_type: ret,
        locals,
        blocks,
    }
}

fn local(name: Symbol, ty: IrType, is_param: bool) -> Local {
    Local { name, ty, is_param }
}

fn global(name: Symbol, ty: IrType) -> Global {
    Global { name, ty }
}

fn program(
    func_table: Vec<FuncDecl>,
    functions: Vec<Function>,
    globals: Vec<Global>,
    type_defs: Vec<TypeDefInfo>,
) -> Program {
    Program {
        func_table,
        functions,
        globals,
        type_defs,
        struct_defs: Vec::new(),
    }
}

/// A single-function, single-block program with no globals/types/func_table.
fn single_fn_program(
    name: Symbol,
    locals: Vec<Local>,
    insts: Vec<Inst>,
    term: Terminator,
) -> Program {
    program(
        Vec::new(),
        vec![func(
            name,
            Vec::new(),
            IrType::Void,
            locals,
            vec![block(0, insts, term)],
        )],
        Vec::new(),
        Vec::new(),
    )
}

// ── Tests ───────────────────────────────────────────────────────────────

#[test]
fn globals_locals_and_signature() {
    // Covers `print_globals`, the param/non-param local flag, and a
    // multi-parameter function signature line.
    let mut i = Interner::new();
    let score = i.intern("score");
    let hiscore = i.intern("hiscore");
    let fname = i.intern("update");
    let dt = i.intern("dt");
    let tmp = i.intern("tmp");

    let prog = program(
        Vec::new(),
        vec![func(
            fname,
            vec![IrType::Float, IrType::Int],
            IrType::Int,
            vec![
                local(dt, IrType::Float, true),
                local(tmp, IrType::Int, false),
            ],
            vec![block(0, vec![], Terminator::Return { value: None })],
        )],
        vec![global(score, IrType::Int), global(hiscore, IrType::Long)],
        Vec::new(),
    );
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn all_binops() {
    use IrBinOp::*;
    let ops = [
        Add, Sub, Mul, Div, Mod, Pow, BinAnd, BinOr, BinXor, Shl, Shr, Sar, Eq, NotEq, Lt, Gt,
        LtEq, GtEq, StrConcat, StrEq, StrNotEq, StrLt, StrGt, StrLtEq, StrGtEq,
    ];
    let mut i = Interner::new();
    let fname = i.intern("binops");
    let insts: Vec<Inst> = ops
        .iter()
        .enumerate()
        .map(|(n, op)| {
            inst(
                Some(n as u32),
                InstKind::BinOp {
                    op: *op,
                    lhs: Reg(100),
                    rhs: Reg(101),
                },
            )
        })
        .collect();
    let prog = single_fn_program(fname, Vec::new(), insts, Terminator::Return { value: None });
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn all_unops() {
    use IrUnOp::*;
    let ops = [Neg, Abs, Not, BinNot];
    let mut i = Interner::new();
    let fname = i.intern("unops");
    let insts: Vec<Inst> = ops
        .iter()
        .enumerate()
        .map(|(n, op)| {
            inst(
                Some(n as u32),
                InstKind::UnOp {
                    op: *op,
                    operand: Reg(100),
                },
            )
        })
        .collect();
    let prog = single_fn_program(fname, Vec::new(), insts, Terminator::Return { value: None });
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn all_types() {
    // Drives `format_type` across every `IrType` variant via local slots.
    let mut i = Interner::new();
    let fname = i.intern("types");
    let v = i.intern("v");
    let enemy = i.intern("Enemy");
    let vec2 = i.intern("Vec2");
    let types = vec![
        IrType::Byte,
        IrType::Short,
        IrType::Int,
        IrType::UInt,
        IrType::Long,
        IrType::ULong,
        IrType::Float,
        IrType::Bool,
        IrType::String,
        IrType::Array {
            elem: Box::new(IrType::Int),
            rank: 2,
        },
        IrType::TypeRef(enemy),
        IrType::StructVal(vec2),
        IrType::FnPtr(Box::new(FnSig {
            params: vec![IrType::Int, IrType::Float],
            ret: Box::new(IrType::Bool),
        })),
        IrType::RuntimeType("Image".to_string()),
        IrType::Null,
        IrType::Void,
    ];
    let locals: Vec<Local> = types.into_iter().map(|t| local(v, t, false)).collect();
    let prog = single_fn_program(fname, locals, vec![], Terminator::Return { value: None });
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn memory_and_variable_insts() {
    let mut i = Interner::new();
    let fname = i.intern("mem");
    let x = i.intern("x");
    let g = i.intern("g");
    let enemy = i.intern("Enemy");
    let hp = i.intern("hp");

    let insts = vec![
        inst(Some(0), InstKind::LoadLocal { local: LocalId(0) }),
        inst(
            None,
            InstKind::StoreLocal {
                local: LocalId(0),
                value: Reg(0),
            },
        ),
        inst(
            Some(1),
            InstKind::LoadGlobal {
                global: GlobalId(0),
            },
        ),
        inst(
            None,
            InstKind::StoreGlobal {
                global: GlobalId(0),
                value: Reg(1),
            },
        ),
        inst(
            Some(2),
            InstKind::NewType {
                type_def: TypeDefId(0),
            },
        ),
        inst(
            Some(3),
            InstKind::NewArray {
                elem_type: IrType::Int,
                dims: vec![Reg(0), Reg(1)],
            },
        ),
        inst(
            Some(4),
            InstKind::GetField {
                object: Reg(2),
                field: hp,
                field_type: IrType::Int,
            },
        ),
        inst(
            Some(5),
            InstKind::GetElement {
                array: Reg(3),
                indices: vec![Reg(0), Reg(1)],
            },
        ),
        // Local root with a field then a (multi-dim) index projection.
        inst(
            None,
            InstKind::StorePlace {
                root: PlaceRoot::Local(LocalId(0)),
                path: vec![
                    Projection::Field(hp),
                    Projection::Index(vec![Reg(0), Reg(1)]),
                ],
                value: Reg(4),
            },
        ),
        // Global root with a single index projection.
        inst(
            None,
            InstKind::StorePlace {
                root: PlaceRoot::Global(GlobalId(0)),
                path: vec![Projection::Index(vec![Reg(0)])],
                value: Reg(1),
            },
        ),
    ];
    let prog = program(
        Vec::new(),
        vec![func(
            fname,
            Vec::new(),
            IrType::Void,
            vec![local(x, IrType::Int, false)],
            vec![block(0, insts, Terminator::Return { value: None })],
        )],
        vec![global(g, IrType::Int)],
        vec![TypeDefInfo {
            name: enemy,
            fields: vec![(hp, IrType::Int)],
        }],
    );
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn type_list_and_delete_insts() {
    let mut i = Interner::new();
    let fname = i.intern("lists");
    let x = i.intern("x");
    let g = i.intern("g");
    let enemy = i.intern("Enemy");

    let insts = vec![
        inst(
            Some(0),
            InstKind::First {
                type_def: TypeDefId(0),
            },
        ),
        inst(
            Some(1),
            InstKind::Last {
                type_def: TypeDefId(0),
            },
        ),
        inst(Some(2), InstKind::Next { object: Reg(0) }),
        inst(Some(3), InstKind::Previous { object: Reg(1) }),
        inst(None, InstKind::DeleteLvalue { local: LocalId(0) }),
        inst(
            None,
            InstKind::DeleteLvalueGlobal {
                global: GlobalId(0),
            },
        ),
        inst(None, InstKind::DeleteRvalue { value: Reg(0) }),
    ];
    let prog = program(
        Vec::new(),
        vec![func(
            fname,
            Vec::new(),
            IrType::Void,
            vec![local(x, IrType::TypeRef(enemy), false)],
            vec![block(0, insts, Terminator::Return { value: None })],
        )],
        vec![global(g, IrType::TypeRef(enemy))],
        vec![TypeDefInfo {
            name: enemy,
            fields: vec![],
        }],
    );
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn intrinsics_and_calls() {
    let mut i = Interner::new();
    let fname = i.intern("calls");
    let spawn = i.intern("Spawn");
    let arr = i.intern("arr");
    let garr = i.intern("garr");

    let insts = vec![
        inst(
            Some(0),
            InstKind::Len {
                array: Reg(50),
                dim: None,
            },
        ),
        inst(
            Some(1),
            InstKind::Len {
                array: Reg(50),
                dim: Some(Reg(51)),
            },
        ),
        inst(Some(2), InstKind::StrLen { s: Reg(52) }),
        inst(
            Some(3),
            InstKind::ConvertExplicit {
                value: Reg(0),
                target: IrType::Float,
            },
        ),
        inst(
            Some(4),
            InstKind::Convert {
                value: Reg(0),
                from: IrType::Int,
                to: IrType::Long,
            },
        ),
        inst(
            Some(5),
            InstKind::Call {
                callee: FuncId(0),
                args: vec![Reg(0), Reg(1)],
            },
        ),
        inst(
            Some(6),
            InstKind::CallIndirect {
                callee: Reg(5),
                args: vec![Reg(0)],
            },
        ),
        inst(Some(7), InstKind::FuncAddr { func: FuncId(0) }),
        inst(
            None,
            InstKind::Redim {
                local: LocalId(0),
                elem_type: IrType::Int,
                dims: vec![Reg(0)],
            },
        ),
        inst(
            None,
            InstKind::RedimGlobal {
                global: GlobalId(0),
                elem_type: IrType::Float,
                dims: vec![Reg(0), Reg(1)],
            },
        ),
    ];
    let prog = program(
        vec![FuncDecl {
            name: spawn,
            sig: FnSig {
                params: vec![IrType::Int, IrType::Int],
                ret: Box::new(IrType::Int),
            },
            kind: FuncKind::UserDefined { body_index: 0 },
        }],
        vec![func(
            fname,
            Vec::new(),
            IrType::Void,
            vec![local(
                arr,
                IrType::Array {
                    elem: Box::new(IrType::Int),
                    rank: 1,
                },
                false,
            )],
            vec![block(0, insts, Terminator::Return { value: None })],
        )],
        vec![global(
            garr,
            IrType::Array {
                elem: Box::new(IrType::Float),
                rank: 2,
            },
        )],
        Vec::new(),
    );
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn all_consts() {
    let mut i = Interner::new();
    let fname = i.intern("consts");
    let insts = vec![
        inst(Some(0), InstKind::ConstInt(42)),
        inst(Some(1), InstKind::ConstLong(9_000_000_000)),
        inst(Some(2), InstKind::ConstFloat(2.5)),
        inst(Some(3), InstKind::ConstBool(true)),
        // Exercises the `{:?}` escaping path (embedded newline + quotes).
        inst(Some(4), InstKind::ConstString("hi\n\"there\"".to_string())),
        inst(Some(5), InstKind::ConstNull),
    ];
    let prog = single_fn_program(fname, Vec::new(), insts, Terminator::Return { value: None });
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn all_terminators() {
    // Goto, BranchIf, Return(Some), Return(None), Halt across five blocks.
    let mut i = Interner::new();
    let fname = i.intern("terms");
    let blocks = vec![
        block(
            0,
            vec![inst(Some(0), InstKind::ConstBool(true))],
            Terminator::Goto(BlockId(1)),
        ),
        block(
            1,
            vec![],
            Terminator::BranchIf {
                cond: Reg(0),
                then_block: BlockId(2),
                else_block: BlockId(3),
            },
        ),
        block(
            2,
            vec![inst(Some(1), InstKind::ConstInt(0))],
            Terminator::Return {
                value: Some(Reg(1)),
            },
        ),
        block(3, vec![], Terminator::Return { value: None }),
        block(4, vec![], Terminator::Halt { code: 1 }),
    ];
    let prog = program(
        Vec::new(),
        vec![func(fname, Vec::new(), IrType::Int, Vec::new(), blocks)],
        Vec::new(),
        Vec::new(),
    );
    insta::assert_snapshot!(print_program(&prog, &i));
}

#[test]
fn all_traps() {
    // One block per `TrapKind` so every trap label is printed.
    let mut i = Interner::new();
    let fname = i.intern("traps");
    let kinds = [
        TrapKind::NullDeref,
        TrapKind::DeletedAccess,
        TrapKind::DivisionByZero,
        TrapKind::IndexOutOfBounds,
        TrapKind::NullFnPtr,
        TrapKind::DoubleDelete,
    ];
    let blocks: Vec<BasicBlock> = kinds
        .iter()
        .enumerate()
        .map(|(n, k)| block(n as u32, vec![], Terminator::Trap(*k)))
        .collect();
    let prog = program(
        Vec::new(),
        vec![func(fname, Vec::new(), IrType::Void, Vec::new(), blocks)],
        Vec::new(),
        Vec::new(),
    );
    insta::assert_snapshot!(print_program(&prog, &i));
}
