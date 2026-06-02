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
