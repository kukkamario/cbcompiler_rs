//! AST node arena and node variants for the CoolBasic frontend.
//!
//! Design notes (see FD-002 §A):
//! - Single homogeneous arena of `Node` with a parallel `spans` side table.
//! - `NodeId` is a `u32` index; child relationships are stored as `NodeId`
//!   (or `Vec<NodeId>` for variable-arity children).
//! - `Expr::FloatLit` carries a [`FloatBits`] wrapper (raw IEEE-754 bits)
//!   rather than a bare `f64`, so the literal types are bit-comparable and
//!   could derive `Eq` if a use case ever wants it. `Eq` is not currently
//!   derived because no caller needs it and `PartialEq` is sufficient.

use crate::span::Span;
use crate::token::{FloatBits, Kw, Sigil, StrLitKind};

/// Index into [`Arena`].
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct NodeId(pub u32);

/// Arena of AST nodes with a parallel span side table.
///
/// Invariant: `nodes.len() == spans.len()`. Maintained by [`Arena::alloc`]
/// being the only mutation API.
#[derive(Clone, Debug, Default)]
pub struct Arena {
    pub(crate) nodes: Vec<Node>,
    pub(crate) spans: Vec<Span>,
}

impl Arena {
    /// An empty arena.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            spans: Vec::new(),
        }
    }

    /// Allocate a node and its span; returns the new id.
    pub fn alloc(&mut self, node: Node, span: Span) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(node);
        self.spans.push(span);
        id
    }

    /// Span of the node with this id.
    pub fn span_of(&self, id: NodeId) -> Span {
        self.spans[id.0 as usize]
    }

    /// Number of allocated nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the arena holds any nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl std::ops::Index<NodeId> for Arena {
    type Output = Node;

    fn index(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }
}

/// Top-level node variants stored in the arena.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Expr(Expr),
    Stmt(Stmt),
    TypeExpr(TypeExpr),
    Param(Param),
    CaseArm(CaseArm),
}

/// Expression nodes.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    IntLit(u64),
    FloatLit(FloatBits),
    NullLit,
    StrLit {
        value: String,
        kind: StrLitKind,
    },
    Ident {
        name_span: Span,
        sigil: Option<Sigil>,
    },
    Unary {
        op: UnOp,
        operand: NodeId,
    },
    Binary {
        op: BinOp,
        lhs: NodeId,
        rhs: NodeId,
    },
    Call {
        callee: NodeId,
        args: Vec<NodeId>,
    },
    Index {
        array: NodeId,
        indices: Vec<NodeId>,
    },
    Field {
        target: NodeId,
        /// Bare-name span of the field (sigil byte excluded, if any). FD-004
        /// #12: previously a full `Expr::Ident` node was allocated for this
        /// position to share machinery with regular identifier exprs. The
        /// span-only form matches every other declaration site (e.g.
        /// `DimName::name_span`, `Stmt::FieldDecl::name_span`) and avoids an
        /// allocation per `.field` access in deep chains.
        name_span: Span,
    },
    Paren {
        inner: NodeId,
    },
    New(NewKind),
    Error,
}

/// Shape of a `New` expression.
#[derive(Clone, Debug, PartialEq)]
pub enum NewKind {
    Type(NodeId),
    Array { elem: NodeId, dims: Vec<NodeId> },
}

/// Statement nodes.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Assign {
        target: NodeId,
        value: NodeId,
    },
    ExprStmt {
        expr: NodeId,
    },
    Dim {
        names: Vec<DimName>,
        ty: Option<NodeId>,
        init: Option<NodeId>,
    },
    Global {
        names: Vec<DimName>,
        ty: Option<NodeId>,
        init: Option<NodeId>,
    },
    Const {
        name_span: Span,
        sigil: Option<Sigil>,
        ty: Option<NodeId>,
        value: NodeId,
        is_global: bool,
    },
    Redim {
        target: NodeId,
        elem_ty: NodeId,
        dims: Vec<NodeId>,
    },
    If {
        cond: NodeId,
        then_body: Vec<NodeId>,
        elseifs: Vec<ElseIf>,
        else_body: Option<Vec<NodeId>>,
        form: IfForm,
    },
    While {
        cond: NodeId,
        body: Vec<NodeId>,
    },
    RepeatForever {
        body: Vec<NodeId>,
    },
    RepeatWhile {
        body: Vec<NodeId>,
        cond: NodeId,
    },
    For {
        var: NodeId,
        from: NodeId,
        to: NodeId,
        step: Option<NodeId>,
        body: Vec<NodeId>,
        next_name: Option<Span>,
    },
    ForEach {
        var: NodeId,
        source: NodeId,
        body: Vec<NodeId>,
        next_name: Option<Span>,
    },
    Select {
        scrutinee: NodeId,
        arms: Vec<NodeId>,
    },
    Function {
        name_span: Span,
        return_sigil: Option<Sigil>,
        params: Vec<NodeId>,
        return_ty: Option<NodeId>,
        body: Vec<NodeId>,
    },
    Type {
        name_span: Span,
        fields: Vec<NodeId>,
    },
    Struct {
        name_span: Span,
        fields: Vec<NodeId>,
    },
    FieldDecl {
        name_span: Span,
        sigil: Option<Sigil>,
        ty: Option<NodeId>,
    },
    Return {
        value: Option<NodeId>,
    },
    Goto {
        label_span: Span,
    },
    Label {
        name_span: Span,
    },
    Break {
        count: Option<u32>,
    },
    Continue,
    /// `End` — terminate the whole program (`cb_runtime.md` §System). Lowered
    /// to an IR `Halt(0)` terminator. Distinct from the block closers
    /// (`End If`/`EndFunction`/…), which the block parsers consume directly.
    End,
    Include {
        path: NodeId,
    },
    /// `Delete <expr>` — removes a `Type` node from its linked list and frees
    /// it (`cb_syntax.md` §3.3). The parser is permissive on operand shape;
    /// sema classifies lvalue (rewinds the variable + marks deleted) vs
    /// rvalue (frees only) and emits runtime-trap diagnostics where it can
    /// prove them statically.
    Delete {
        operand: NodeId,
    },
    Error,
}

/// One name in a `Dim` / `Global` declaration.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct DimName {
    pub name_span: Span,
    pub sigil: Option<Sigil>,
}

/// One `ElseIf` arm of an `If` statement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElseIf {
    pub cond: NodeId,
    pub body: Vec<NodeId>,
}

/// Surface form of an `If` statement: block or single-line.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IfForm {
    Block,
    SingleLine,
}

/// One arm of a `Select` statement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaseArm {
    Case {
        values: Vec<NodeId>,
        body: Vec<NodeId>,
    },
    Default {
        body: Vec<NodeId>,
    },
}

/// A function parameter (or fn-ptr type parameter; `name_span` is `None`
/// inside fn-ptr type position).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Param {
    pub name_span: Option<Span>,
    pub sigil: Option<Sigil>,
    pub ty: Option<NodeId>,
    pub default: Option<NodeId>,
}

/// Type-expression nodes.
#[derive(Clone, Debug, PartialEq)]
pub enum TypeExpr {
    Primitive {
        kw: Kw,
    },
    Named {
        name_span: Span,
    },
    Array {
        elem: NodeId,
        rank: u8,
    },
    FnPtr {
        params: Vec<NodeId>,
        ret: Option<NodeId>,
    },
    Paren {
        inner: NodeId,
    },
    Error,
}

/// Binary operators surfaced in [`Expr::Binary`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,
    BinAnd,
    BinOr,
    BinXor,
    Shl,
    Shr,
    Sar,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    Xor,
}

/// Unary operators surfaced in [`Expr::Unary`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UnOp {
    Plus,
    Neg,
    Not,
    BinNot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;

    #[test]
    fn arena_alloc_roundtrip() {
        let mut arena = Arena::new();
        let span = Span::new(0, 5, FileId(0));
        let id = arena.alloc(Node::Expr(Expr::IntLit(42)), span);
        assert_eq!(arena.span_of(id), span);
        match &arena[id] {
            Node::Expr(Expr::IntLit(v)) => assert_eq!(*v, 42),
            other => panic!("unexpected node: {other:?}"),
        }
        assert_eq!(arena.len(), 1);
    }
}
