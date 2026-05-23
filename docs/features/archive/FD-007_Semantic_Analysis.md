# FD-007: Semantic Analysis

**Status:** Complete
**Completed:** 2026-05-23
**Priority:** High
**Effort:** High (> 4 hours)
**Impact:** Bridges the gap between parsing and code generation — without sema, no backend can run.

## Problem

The compiler can parse CoolBasic source into an untyped AST, but has no understanding of names, types, or scoping. Every backend needs a type-checked, name-resolved program to operate on. Semantic analysis must:

- Resolve identifiers to declarations (variables, functions, types, structs, constants, labels)
- Enforce CoolBasic's type system: 9 primitives, sigils, Type vs Struct, arrays
- Insert or annotate implicit conversions (numeric widening, Bool↔numeric, etc.)
- Diagnose errors: undeclared names, type mismatches, sigil conflicts, duplicate declarations, etc.
- Classify Delete operands as lvalue vs rvalue (deferred from FD-005)
- Honor CoolBasic's scoping rules: function-level scope (no block scope), globals, hoisting

## Design Decisions (from discussion)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Sema location | New `cb-sema` crate | Clean boundary: `cb-frontend` → `cb-sema` → `cb-ir` |
| Typed AST shape | Side-table annotations on existing AST | AST already has `NodeId`; no need to duplicate the tree |
| IR style (FD-008) | Three-address / register-based | Informs sema output: sema annotates types, IR lowering is a later mechanical pass |

## Solution

### New crate: `cb-sema`

**Dependencies:** `cb-frontend`, `cb-diagnostics`

**Public API:**

```rust
pub fn analyze(
    arena: &Arena,
    program: &[NodeId],
    source: &str,
    file_id: FileId,
) -> SemaResult;

pub struct SemaResult {
    pub types: TypeTable,        // NodeId → resolved type
    pub symbols: SymbolTable,    // scopes + declarations
    pub conversions: ConversionTable,  // NodeId → implicit conversion (if any)
    pub diagnostics: Vec<Diagnostic>,
}

/// Maps expression and variable nodes to their resolved types.
pub struct TypeTable {
    entries: HashMap<NodeId, Type>,
}
```

### Core data structures

#### Type representation

```rust
/// Resolved type of a CoolBasic expression or variable.
pub enum Type {
    // Primitives (§3.1)
    Byte,
    Short,
    Int,       // Integer / Int — sigil %
    UInt,      // UInteger / UInt
    Long,
    ULong,
    Float,     // sigil #
    Bool,      // sigil !
    String,    // sigil $

    // Composite
    Array { elem: Box<Type>, rank: u8 },
    TypeRef { name: Symbol },     // reference to a Type…EndType instance
    StructVal { name: Symbol },   // value-type Struct…EndStruct
    FnPtr { params: Vec<Type>, ret: Option<Box<Type>> },

    // Special
    Null,      // type of NullLit — coerces to any reference type
    Void,      // return "type" of a Sub (no return value)
    Error,     // propagated from parse errors; suppresses cascading diagnostics
}
```

#### Symbol table

```rust
pub struct SymbolTable {
    scopes: Vec<Scope>,
}

pub struct Scope {
    parent: Option<ScopeId>,
    kind: ScopeKind,               // TopLevel | Function
    symbols: HashMap<Symbol, Declaration>,
}

pub enum ScopeKind {
    TopLevel,
    Function,
}

pub struct Declaration {
    pub kind: DeclKind,
    pub ty: Type,
    pub span: Span,                // declaration site for diagnostics
    pub is_global: bool,
}

pub enum DeclKind {
    Variable,
    Constant { value: ConstValue },
    Function { params: Vec<ParamInfo>, return_ty: Type },
    TypeDef { fields: Vec<FieldInfo> },
    StructDef { fields: Vec<FieldInfo> },
    Label,
}

/// Compile-time constant value (result of evaluating a Const initializer).
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
}

/// Parameter metadata for a function declaration.
pub struct ParamInfo {
    pub name: Symbol,
    pub ty: Type,
    pub has_default: bool,      // whether a default value was specified
}

/// Field metadata for a Type or Struct definition.
pub struct FieldInfo {
    pub name: Symbol,
    pub ty: Type,
    pub span: Span,             // field declaration site
}
```

#### Conversion table

```rust
pub struct ConversionTable {
    entries: HashMap<NodeId, Conversion>,
}

pub enum Conversion {
    NumericWiden,          // e.g. Byte → Int
    IntToFloat,
    FloatToInt,           // truncation — also emits a warning
    BoolToNumeric,
    NumericToBool,
    NumericToString,      // implicit via `+` operator
    NullToRef,            // Null → any TypeRef/Array
}
```

### Analysis passes

Sema runs in two passes over the AST:

#### Pass 1: Declaration collection (hoisting)

Walk the top-level statement list and collect:
- `Function`/`Sub` declarations (name, params, return type)
- `Type … EndType` declarations (name, fields)
- `Struct … EndStruct` declarations (name, fields)
- `Global` declarations
- `Label` declarations
- Top-level `Const` declarations

This enables forward references — functions, types, and structs are visible everywhere regardless of definition order (§7 hoisting rule).

#### Pass 2: Full resolution and type checking

Walk every node in program order. For each node:

1. **Name resolution** — look up identifiers in the current scope chain. CoolBasic rules:
   - Function scope sees: local variables + globals + hoisted functions/types/structs
   - Function scope does NOT see ordinary top-level variables (§4.3)
   - Variables introduced anywhere in a function body are visible to end of function (no block scoping)

2. **Implicit declaration** — if an identifier is not found in scope and appears as an assignment target or `Dim` without `As`, create an implicit declaration. Type inferred from sigil or defaults to Integer (§4.1).

3. **Sigil enforcement** — if a name has a sigil, its type is locked:
   - `%` → Integer, `#` → Float, `$` → String, `!` → Bool
   - Sigil on use must match sigil on declaration (or first use if implicit)
   - `As Type` must agree with sigil if both present

4. **Expression typing** — bottom-up type inference:
   - Literals: `IntLit` → Int, `FloatLit` → Float, `BoolLit` → Bool, `StrLit` → String, `NullLit` → Null
   - Binary ops: result type from operand types + operator rules (see §3.4)
   - Unary ops: same type as operand (with numeric promotion)
   - Call: return type of the resolved function
   - Index: element type of the array
   - Field: type of the field in the Type/Struct definition
   - New: TypeRef or Array depending on NewKind

5. **Implicit conversion insertion** — when operand types don't match but are convertible:
   - Record in `ConversionTable` keyed by the `NodeId` that needs conversion
   - Emit warning diagnostic for narrowing conversions (Float→Int, wide→narrow integer)

6. **Statement checking:**
   - `Assign`: target type compatible with value type (with conversion)
   - `If`/`While`/`Repeat`: condition must be Bool-convertible
   - `For`: loop variable must be numeric; from/to/step must be numeric
   - `Return`: value type must match enclosing function's return type
   - `Delete`: classify operand as lvalue or rvalue; operand must be TypeRef
   - `Goto`: label must exist in the same scope; must not jump into a `For` block from outside (§6.4)

7. **Compiler intrinsic resolution** — the following names are compiler-known and resolved specially (§8):
   - `Len(arr)` / `Len(arr, dim)`: array length query — returns Int
   - `Int(val)`, `Float(val)`, `Str(val)`, `Bool(val)`: explicit type conversion functions — these perform runtime conversion (including string parsing: `Int("123")` → 123, parse failure → 0) and are distinct from implicit conversions

### Diagnostics

New error codes (E03xx series for semantic errors):

| Code | Condition |
|------|-----------|
| E0300 | Undeclared identifier |
| E0301 | Type mismatch (assignment, return, argument) |
| E0302 | Sigil conflict (sigil doesn't match declared type) |
| E0303 | Duplicate declaration in same scope |
| E0304 | Cannot call non-function |
| E0305 | Wrong number of arguments |
| E0306 | Cannot index non-array |
| E0307 | Wrong number of array indices (rank mismatch) |
| E0308 | Field does not exist on type |
| E0309 | Field access on non-Type/non-Struct |
| E0310 | Delete on non-TypeRef expression |
| E0311 | Cannot use Type/Struct as value (it's a type name, not a variable) |
| E0312 | Undeclared label (Goto target) |
| E0313 | Return outside of function |
| E0314 | Return with value in Sub (no return type) |
| E0315 | Missing return value in Function |
| E0316 | For loop variable must be numeric |
| E0317 | Cannot convert between types (no implicit or explicit path) |
| E0318 | Implicit narrowing conversion (warning, not error) |
| E0319 | Duplicate Type/Struct/Function definition |
| E0320 | Sigil and `As` type disagree |
| E0321 | Goto jumps into a For block from outside (§6.4) |

### String interning

Identifier names need efficient comparison. `Symbol` and `Interner` live in **`cb-diagnostics`** (the shared leaf crate) so that `cb-frontend`, `cb-sema`, and `cb-ir` can all use `Symbol` without circular dependencies. `cb-diagnostics` already hosts other fundamental compiler types (`Span`, `FileId`); `Symbol` is the same kind of infrastructure.

```rust
// In cb-diagnostics::intern

/// Interned string identifier.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Symbol(u32);

pub struct Interner {
    map: HashMap<String, Symbol>,
    strings: Vec<String>,
}
```

CoolBasic identifiers are **case-insensitive** — the interner should normalize to lowercase (or a canonical case) before interning. This is load-bearing: `myVar`, `MyVar`, and `MYVAR` must resolve to the same symbol.

The interner is populated during sema (pass 1 and pass 2) by extracting name text from `Span` ranges in the source. The lexer continues to store names as `Span`s; interning happens when sema first resolves each name.

### Include handling

`Include` (§2.2) is a pre-sema concern: the driver resolves includes, parses each file, and merges the resulting ASTs into a single program before calling `analyze()`. This means:

- `Stmt::Include` nodes are consumed by the driver's include-resolution pass and never reach sema.
- Each included file gets its own `FileId`; the `source` parameter to `analyze()` covers only the main file. Multi-file source access (for interning identifiers from included files) requires either a `SourceMap` that maps `FileId → &str`, or a pre-merge pass that interns all identifiers during include resolution.
- The at-most-once and top-level-only restrictions (§2.2) are enforced during include resolution in the driver, not in sema.

The full include-resolution design is deferred to a future FD. For FD-007, sema assumes a single-file program. The `analyze()` signature accepts a single `source: &str` for now; it will evolve to a `&SourceMap` when multi-file support lands.

### Integration with cb-driver

The driver calls `cb_sema::analyze()` after parsing, before any backend:

```
source → tokenize → parse → analyze → [IR lowering] → [backend]
```

If `analyze` produces errors, the driver reports them and exits (no IR lowering attempted). Warnings are reported but don't block compilation.

## Scope & non-scope

**In scope for FD-007:**
- `cb-sema` crate creation, all structures above
- Pass 1 (declaration collection / hoisting)
- Pass 2 (name resolution, type checking, conversion annotation)
- Diagnostic emission for all E03xx errors
- String interning with case-insensitive comparison (in `cb-diagnostics`)
- Delete lvalue/rvalue classification
- `Const` initializer evaluation (literal folding: `Const x = 1 + 2` → `ConstValue::Int(3)`)
- Compiler intrinsic resolution (`Len`, `Int()`, `Float()`, `Str()`, `Bool()`)
- Goto-into-For restriction (E0321)
- Integration into the driver pipeline

**Deferred to later FDs:**
- General constant folding / optimization (e.g. `1 + 2` in non-Const contexts)
- Exhaustive `Select` arm checking
- Unreachable code detection
- IR lowering (FD-008)
- Runtime error semantics — Delete trap, null deref, etc. (interpreter FD)

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/Cargo.toml` | CREATE | New crate: depends on `cb-frontend`, `cb-diagnostics` |
| `crates/cb-sema/src/lib.rs` | CREATE | Public API: `analyze()`, `SemaResult` |
| `crates/cb-sema/src/types.rs` | CREATE | `Type` enum, type comparison/conversion logic |
| `crates/cb-sema/src/scope.rs` | CREATE | `SymbolTable`, `Scope`, `Declaration`, name resolution |
| `crates/cb-sema/src/check.rs` | CREATE | Main analysis passes (declaration collection + type checking) |
| `crates/cb-diagnostics/src/intern.rs` | CREATE | `Symbol`, `Interner` — case-insensitive string interning (in `cb-diagnostics` so all crates can use `Symbol`) |
| `crates/cb-sema/src/convert.rs` | CREATE | `ConversionTable`, implicit conversion rules |
| `crates/cb-sema/src/diagnostics.rs` | CREATE | E03xx error code definitions and helpers |
| `Cargo.toml` (workspace) | MODIFY | Add `crates/cb-sema` to workspace members |
| `crates/cb-driver/Cargo.toml` | MODIFY | Add `cb-sema` dependency |
| `crates/cb-driver/src/main.rs` | MODIFY | Call `analyze()` after parsing, report sema diagnostics |

## Verification

1. **Unit tests in `cb-sema`:**
   - Name resolution: declared, undeclared, shadowed, global vs local
   - Sigil: matching, conflicting, implicit from sigil
   - Type checking: each expression form, each statement form
   - Implicit conversions: widening (no warning), narrowing (warning), illegal
   - Hoisting: forward reference to function/type/struct
   - Delete: lvalue vs rvalue classification
   - Scope rules: function scope hides top-level vars, globals visible in functions

2. **Snapshot tests (insta):** Parse + analyze a `.cb` snippet, snapshot the diagnostics output.

3. **Driver integration:** `cargo run -p cb-driver -- file.cb` shows sema errors with correct spans and messages.

4. **All existing tests pass:** `cargo test --workspace`

## Related

- [FD-002](archive/FD-002_PARSER.md) — Parser (produces the AST that sema consumes)
- [FD-005](archive/FD-005_DELETE_STATEMENT.md) — Delete statement (deferred lvalue/rvalue to sema)
- [FD-006](archive/FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) — Diagnostics infra (sema emits `Diagnostic`)
- `docs/cb_syntax.md` — Authoritative type/scope/conversion rules (§3, §4, §7)
- Future FD-008: IR design + lowering (consumes `SemaResult`)
