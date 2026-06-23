//! Semantic-analysis error codes (E03xx series).
#![allow(dead_code)]

use cb_diagnostics::DiagnosticCode;

pub const E_UNDECLARED_IDENT: DiagnosticCode = DiagnosticCode::new("E0300");
pub const E_TYPE_MISMATCH: DiagnosticCode = DiagnosticCode::new("E0301");
pub const E_SIGIL_CONFLICT: DiagnosticCode = DiagnosticCode::new("E0302");
pub const E_DUPLICATE_DECL: DiagnosticCode = DiagnosticCode::new("E0303");
pub const E_CALL_NON_FUNCTION: DiagnosticCode = DiagnosticCode::new("E0304");
pub const E_WRONG_ARG_COUNT: DiagnosticCode = DiagnosticCode::new("E0305");
pub const E_INDEX_NON_ARRAY: DiagnosticCode = DiagnosticCode::new("E0306");
pub const E_RANK_MISMATCH: DiagnosticCode = DiagnosticCode::new("E0307");
pub const E_NO_SUCH_FIELD: DiagnosticCode = DiagnosticCode::new("E0308");
pub const E_FIELD_ON_NON_TYPE: DiagnosticCode = DiagnosticCode::new("E0309");
pub const E_DELETE_NON_TYPEREF: DiagnosticCode = DiagnosticCode::new("E0310");
pub const E_TYPE_AS_VALUE: DiagnosticCode = DiagnosticCode::new("E0311");
pub const E_UNDECLARED_LABEL: DiagnosticCode = DiagnosticCode::new("E0312");
pub const E_RETURN_OUTSIDE_FN: DiagnosticCode = DiagnosticCode::new("E0313");
pub const E_RETURN_VALUE_IN_SUB: DiagnosticCode = DiagnosticCode::new("E0314");
pub const E_MISSING_RETURN_VALUE: DiagnosticCode = DiagnosticCode::new("E0315");
pub const E_FOR_VAR_NOT_NUMERIC: DiagnosticCode = DiagnosticCode::new("E0316");
pub const E_CANNOT_CONVERT: DiagnosticCode = DiagnosticCode::new("E0317");
pub const E_NARROWING_CONVERSION: DiagnosticCode = DiagnosticCode::new("E0318");
pub const E_DUPLICATE_DEFINITION: DiagnosticCode = DiagnosticCode::new("E0319");
pub const E_SIGIL_AS_DISAGREE: DiagnosticCode = DiagnosticCode::new("E0320");
pub const E_GOTO_INTO_FOR: DiagnosticCode = DiagnosticCode::new("E0321");
pub const E_CONST_EVAL_ERROR: DiagnosticCode = DiagnosticCode::new("E0322");
pub const E_AMBIGUOUS_OVERLOAD: DiagnosticCode = DiagnosticCode::new("E0323");
pub const E_NO_MATCHING_OVERLOAD: DiagnosticCode = DiagnosticCode::new("E0324");
pub const E_INVALID_ASSIGN_TARGET: DiagnosticCode = DiagnosticCode::new("E0325");
pub const E_LITERAL_OVERFLOW: DiagnosticCode = DiagnosticCode::new("E0326");
pub const E_CONST_FLOAT_DIV_ZERO: DiagnosticCode = DiagnosticCode::new("E0327");
/// A built-in runtime command name was used as a variable via an implicit
/// assignment. Implicit declarations may not shadow commands; an explicit
/// `Dim` is required (FD-027).
pub const E_RUNTIME_COMMAND_AS_VAR: DiagnosticCode = DiagnosticCode::new("E0328");
/// A bare overloaded or built-in command name was used in value position to
/// take its address. Only non-overloaded user-defined functions/subs have a
/// single well-defined address (cb_syntax.md §7.2/§7.4).
pub const E_ADDRESS_OF_UNSUPPORTED: DiagnosticCode = DiagnosticCode::new("E0329");
/// A reserved-but-unsupported type name (`Bool`, `Boolean`, `UInt`,
/// `UInteger`, `ULong`) was used in a type position. These names stay reserved
/// (cb_syntax.md §1.5/§3.1) but denote no type since FD-035 narrowed the
/// scalar set to Byte/Short/Int/Long/Float/String.
pub const E_RESERVED_TYPE: DiagnosticCode = DiagnosticCode::new("E0330");
/// An implicit declaration (a first assignment with no sigil and no `As`) could
/// not infer a type from its value — the value is `Null` (no concrete reference
/// type) or has no value at all (a Sub call). The fix is an explicit `As`
/// annotation or `Dim` (cb_syntax.md §4.1).
pub const E_CANNOT_INFER_TYPE: DiagnosticCode = DiagnosticCode::new("E0331");
