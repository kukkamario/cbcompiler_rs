//! Plain-assertion coverage for the `_global`-suffix mnemonic convention in
//! the IR text printer (II-V5 / II-V6). These are intentionally NOT insta
//! snapshots: they assert the exact suffixed mnemonics that distinguish the
//! global-rooted instructions (`delete_lvalue_global`, `redim_global`) from
//! their local-rooted counterparts (`delete_lvalue`, `redim`).

use cb_diagnostics::source::FileId;
use cb_diagnostics::{Interner, Span};

use cb_ir::print::print_program;
use cb_ir::{
    BasicBlock, BlockId, Function, Global, GlobalId, Inst, InstKind, IrType, Local, LocalId,
    Program, Reg, Terminator,
};

const SPAN: Span = Span::new(0, 0, FileId::SYNTHETIC);

fn single_block_fn(
    interner: &mut Interner,
    locals: Vec<Local>,
    globals: Vec<Global>,
    insts: Vec<Inst>,
) -> Program {
    let fname = interner.intern("f");
    Program {
        func_table: Vec::new(),
        functions: vec![Function {
            name: fname,
            params: Vec::new(),
            return_type: IrType::Void,
            locals,
            blocks: vec![BasicBlock {
                id: BlockId(0),
                insts,
                terminator: Some(Terminator::Return { value: None }),
                terminator_span: SPAN,
            }],
        }],
        globals,
        type_defs: Vec::new(),
        struct_defs: Vec::new(),
    }
}

fn inst(result: Option<u32>, kind: InstKind) -> Inst {
    Inst {
        result: result.map(Reg),
        kind,
        span: SPAN,
    }
}

#[test]
fn delete_lvalue_global_uses_global_suffix() {
    let mut i = Interner::new();
    let x = i.intern("x");
    let g = i.intern("g");
    let enemy = i.intern("Enemy");

    let prog = single_block_fn(
        &mut i,
        vec![Local {
            name: x,
            ty: IrType::TypeRef(enemy),
            is_param: false,
        }],
        vec![Global {
            name: g,
            ty: IrType::TypeRef(enemy),
        }],
        vec![
            inst(None, InstKind::DeleteLvalue { local: LocalId(0) }),
            inst(
                None,
                InstKind::DeleteLvalueGlobal {
                    global: GlobalId(0),
                },
            ),
        ],
    );

    let dump = print_program(&prog, &i);
    // Local form keeps the bare mnemonic; global form gains the `_global`
    // suffix matching load_global/store_global.
    assert!(
        dump.contains("delete_lvalue_global "),
        "missing delete_lvalue_global in:\n{dump}"
    );
    // The local form must still appear exactly once (not as a prefix match of
    // the global form): count whole-token occurrences.
    let bare = dump
        .lines()
        .filter(|l| l.trim_start().starts_with("delete_lvalue "))
        .count();
    assert_eq!(
        bare, 1,
        "expected exactly one bare delete_lvalue in:\n{dump}"
    );
    let global = dump
        .lines()
        .filter(|l| l.trim_start().starts_with("delete_lvalue_global "))
        .count();
    assert_eq!(global, 1, "expected one delete_lvalue_global in:\n{dump}");
}

#[test]
fn redim_global_uses_global_suffix() {
    let mut i = Interner::new();
    let arr = i.intern("arr");
    let garr = i.intern("garr");

    let prog = single_block_fn(
        &mut i,
        vec![Local {
            name: arr,
            ty: IrType::Array {
                elem: Box::new(IrType::Int),
                rank: 1,
            },
            is_param: false,
        }],
        vec![Global {
            name: garr,
            ty: IrType::Array {
                elem: Box::new(IrType::Float),
                rank: 1,
            },
        }],
        vec![
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
                    dims: vec![Reg(0)],
                },
            ),
        ],
    );

    let dump = print_program(&prog, &i);
    assert!(
        dump.contains("redim_global "),
        "missing redim_global in:\n{dump}"
    );
    let bare = dump
        .lines()
        .filter(|l| l.trim_start().starts_with("redim "))
        .count();
    assert_eq!(bare, 1, "expected exactly one bare redim in:\n{dump}");
    let global = dump
        .lines()
        .filter(|l| l.trim_start().starts_with("redim_global "))
        .count();
    assert_eq!(global, 1, "expected one redim_global in:\n{dump}");
}
