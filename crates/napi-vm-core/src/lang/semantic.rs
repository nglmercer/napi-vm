//! Semantic tokens: a classification of every token in a document, for
//! syntax highlighting that follows the grammar rather than a regular
//! expression.
//!
//! Derived from the token stream, so it is exactly as accurate as the lexer —
//! which is what makes it worth having over an editor's own heuristics: a
//! regular expression inside a template literal, or a `/` that is division
//! rather than a pattern, are already resolved here.

use crate::lexer::{Lexer, Token};

/// The token types this server reports, in the order the LSP legend lists
/// them. The index into this array is what the protocol transmits.
pub const LEGEND: &[&str] = &[
    "keyword", "string", "number", "operator", "variable", "function", "class", "property",
    "regexp", "comment",
];

fn type_index(name: &str) -> u32 {
    LEGEND
        .iter()
        .position(|candidate| *candidate == name)
        .unwrap_or(4) as u32
}

/// One classified token: an absolute position, a length, and a type index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub token_type: u32,
}

/// Classify every token in `source`.
pub fn semantic_tokens(source: &str) -> Vec<SemanticToken> {
    let tokens = Lexer::new(source).tokenize_with_spans();
    let mut out = Vec::new();
    for (position, (token, span)) in tokens.iter().enumerate() {
        let (kind, length) = match token {
            Token::EOF => continue,
            Token::Identifier(name) => {
                // An identifier followed by `(` is being called; one after a
                // `.` is a property. Everything else is a plain variable.
                let kind = if matches!(
                    tokens.get(position + 1).map(|(t, _)| t),
                    Some(Token::LParen)
                ) {
                    "function"
                } else if matches!(
                    tokens.get(position.wrapping_sub(1)).map(|(t, _)| t),
                    Some(Token::Dot)
                ) && position > 0
                {
                    "property"
                } else if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    "class"
                } else {
                    "variable"
                };
                (kind, name.chars().count())
            }
            Token::String(text) => ("string", text.chars().count() + 2),
            Token::TemplateQuasi(chunk) => ("string", chunk.raw.chars().count()),
            Token::Number(value) => ("number", crate::format::number_string(*value).len()),
            Token::BigInt(digits) => ("number", digits.chars().count() + 1),
            Token::Regex(pattern, flags) => (
                "regexp",
                pattern.chars().count() + flags.chars().count() + 2,
            ),
            other => {
                let text = describe_token(other);
                if text.is_empty() {
                    continue;
                }
                let kind = if text.chars().all(|c| c.is_alphabetic()) {
                    "keyword"
                } else {
                    "operator"
                };
                (kind, text.chars().count())
            }
        };
        out.push(SemanticToken {
            line: span.line,
            column: span.col,
            length,
            token_type: type_index(kind),
        });
    }
    out
}

/// Encode tokens the way the LSP wants them: five integers each, with the
/// line and column *deltas* from the previous token.
pub fn encode(tokens: &[SemanticToken]) -> Vec<u32> {
    let mut out = Vec::with_capacity(tokens.len() * 5);
    let mut previous_line = 1usize;
    let mut previous_column = 1usize;
    for token in tokens {
        let delta_line = token.line.saturating_sub(previous_line);
        let delta_column = if delta_line == 0 {
            token.column.saturating_sub(previous_column)
        } else {
            token.column.saturating_sub(1)
        };
        out.push(delta_line as u32);
        out.push(delta_column as u32);
        out.push(token.length as u32);
        out.push(token.token_type);
        // No modifiers are reported.
        out.push(0);
        previous_line = token.line;
        previous_column = token.column;
    }
    out
}

/// The source text of a fixed token, for length and keyword classification.
/// Tokens whose text varies are handled by the caller.
fn describe_token(token: &Token) -> &'static str {
    use Token::*;
    match token {
        KwVar => "var",
        KwLet => "let",
        KwConst => "const",
        KwFunction => "function",
        KwReturn => "return",
        KwIf => "if",
        KwElse => "else",
        KwWhile => "while",
        KwDo => "do",
        KwFor => "for",
        KwBreak => "break",
        KwContinue => "continue",
        KwClass => "class",
        KwExtends => "extends",
        KwNew => "new",
        KwThis => "this",
        KwSuper => "super",
        KwNull => "null",
        KwTrue => "true",
        KwFalse => "false",
        KwTypeof => "typeof",
        KwInstanceof => "instanceof",
        KwIn => "in",
        KwOf => "of",
        KwDelete => "delete",
        KwVoid => "void",
        KwThrow => "throw",
        KwTry => "try",
        KwCatch => "catch",
        KwFinally => "finally",
        KwSwitch => "switch",
        KwCase => "case",
        KwDefault => "default",
        KwAsync => "async",
        KwAwait => "await",
        KwYield => "yield",
        KwImport => "import",
        KwExport => "export",
        KwFrom => "from",
        KwAs => "as",
        KwStatic => "static",
        KwGet => "get",
        KwSet => "set",
        KwConstructor => "constructor",
        Plus => "+",
        Minus => "-",
        Star => "*",
        Slash => "/",
        Percent => "%",
        Equal => "=",
        EqualEqual => "==",
        EqualEqualEqual => "===",
        NotEqual => "!=",
        NotEqualEqual => "!==",
        Less => "<",
        Greater => ">",
        LessEqual => "<=",
        GreaterEqual => ">=",
        And => "&&",
        Or => "||",
        Not => "!",
        Question => "?",
        Colon => ":",
        Semicolon => ";",
        Comma => ",",
        Dot => ".",
        LParen => "(",
        RParen => ")",
        LBrace => "{",
        RBrace => "}",
        LBracket => "[",
        RBracket => "]",
        Arrow => "=>",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<&'static str> {
        semantic_tokens(source)
            .iter()
            .map(|token| LEGEND[token.token_type as usize])
            .collect()
    }

    #[test]
    fn keywords_strings_and_numbers_are_classified() {
        assert_eq!(
            kinds("const x = 'a';"),
            vec!["keyword", "variable", "operator", "string", "operator"]
        );
        assert_eq!(kinds("1;"), vec!["number", "operator"]);
    }

    #[test]
    fn a_call_target_is_a_function() {
        assert_eq!(kinds("f()"), vec!["function", "operator", "operator"]);
    }

    #[test]
    fn a_member_name_is_a_property() {
        assert_eq!(kinds("o.p"), vec!["variable", "operator", "property"]);
    }

    #[test]
    fn a_capitalized_name_is_a_class() {
        assert_eq!(kinds("Foo"), vec!["class"]);
    }

    #[test]
    fn a_regex_literal_is_not_division() {
        assert_eq!(kinds("/ab/g"), vec!["regexp"]);
        assert_eq!(kinds("a / b"), vec!["variable", "operator", "variable"]);
    }

    #[test]
    fn encoding_uses_deltas() {
        let tokens = semantic_tokens("a\nb");
        let encoded = encode(&tokens);
        // Two tokens, five integers each.
        assert_eq!(encoded.len(), 10);
        // The first is on line 1 column 1: no delta from the origin.
        assert_eq!(&encoded[0..2], &[0, 0]);
        // The second is one line down, at column 1.
        assert_eq!(&encoded[5..7], &[1, 0]);
    }
}
