//! Compile-time keyword table. Lookup is ASCII case-insensitive: the table is
//! lowercase and callers must lowercase the candidate first.

use crate::token::Kw;
use phf::phf_map;

static KEYWORDS: phf::Map<&'static str, Kw> = phf_map! {
    "and" => Kw::And,
    "as" => Kw::As,
    "binand" => Kw::BinAnd,
    "binnot" => Kw::BinNot,
    "binor" => Kw::BinOr,
    "binxor" => Kw::BinXor,
    "bool" => Kw::Bool,
    "break" => Kw::Break,
    "byte" => Kw::Byte,
    "case" => Kw::Case,
    "const" => Kw::Const,
    "continue" => Kw::Continue,
    "default" => Kw::Default,
    "delete" => Kw::Delete,
    "dim" => Kw::Dim,
    "each" => Kw::Each,
    "else" => Kw::Else,
    "elseif" => Kw::ElseIf,
    "end" => Kw::End,
    "endfunction" => Kw::EndFunction,
    "endif" => Kw::EndIf,
    "endselect" => Kw::EndSelect,
    "endstruct" => Kw::EndStruct,
    "endtype" => Kw::EndType,
    "false" => Kw::False,
    "field" => Kw::Field,
    "float" => Kw::Float,
    "for" => Kw::For,
    "forever" => Kw::Forever,
    "function" => Kw::Function,
    "global" => Kw::Global,
    "goto" => Kw::Goto,
    "if" => Kw::If,
    "include" => Kw::Include,
    "int" => Kw::Int,
    "integer" => Kw::Integer,
    "long" => Kw::Long,
    "mod" => Kw::Mod,
    "new" => Kw::New,
    "next" => Kw::Next,
    "not" => Kw::Not,
    "null" => Kw::Null,
    "or" => Kw::Or,
    "redim" => Kw::Redim,
    "repeat" => Kw::Repeat,
    "return" => Kw::Return,
    "sar" => Kw::Sar,
    "select" => Kw::Select,
    "shl" => Kw::Shl,
    "short" => Kw::Short,
    "shr" => Kw::Shr,
    "step" => Kw::Step,
    "string" => Kw::String,
    "struct" => Kw::Struct,
    "then" => Kw::Then,
    "to" => Kw::To,
    "true" => Kw::True,
    "type" => Kw::Type,
    "uint" => Kw::UInt,
    "uinteger" => Kw::UInteger,
    "ulong" => Kw::ULong,
    "wend" => Kw::Wend,
    "while" => Kw::While,
    "xor" => Kw::Xor,
};

/// The longest keyword in `KEYWORDS`, used to size on-stack ASCII-lowercase
/// scratch buffers in the lexer. `endfunction` is 11 bytes.
pub(crate) const LONGEST_KEYWORD_LEN: usize = 11;

/// Look up a candidate identifier (any ASCII case) as a keyword. Returns
/// `None` if `candidate` contains any non-ASCII byte (keywords are ASCII).
pub fn lookup(candidate: &str) -> Option<Kw> {
    if !candidate.is_ascii() {
        return None;
    }
    if candidate.len() > LONGEST_KEYWORD_LEN {
        return None;
    }
    let mut buf = [0u8; LONGEST_KEYWORD_LEN];
    for (i, b) in candidate.bytes().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let lowered = std::str::from_utf8(&buf[..candidate.len()]).ok()?;
    KEYWORDS.get(lowered).copied()
}

#[cfg(test)]
mod tests {
    use super::{KEYWORDS, LONGEST_KEYWORD_LEN};

    /// `LONGEST_KEYWORD_LEN` sizes the on-stack lowercase scratch buffer in
    /// [`super::lookup`]; if a longer keyword is ever added without bumping
    /// this constant, lookup silently rejects it. This test pins the
    /// invariant so that regression is impossible.
    #[test]
    fn longest_keyword_len_matches_table() {
        let actual = KEYWORDS
            .keys()
            .map(|k| k.len())
            .max()
            .expect("KEYWORDS is non-empty");
        assert_eq!(
            actual, LONGEST_KEYWORD_LEN,
            "LONGEST_KEYWORD_LEN ({LONGEST_KEYWORD_LEN}) is out of sync with the table (longest entry is {actual} bytes)",
        );
    }
}
