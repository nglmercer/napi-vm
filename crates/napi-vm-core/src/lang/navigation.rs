//! Navigation features built on the parser's symbol index: go-to-definition,
//! references, highlights, rename, document symbols, signature help and inlay
//! hints.
//!
//! Resolution is scope-accurate — the index records which lexical scope each
//! occurrence belongs to — so two same-named bindings in sibling scopes stay
//! apart. That is what makes rename safe to offer at all.

use crate::lexer::Lexer;
use crate::parser::{DeclKind, Entry, Occurrence, Parser, SymbolIndex};
use crate::span::Span;

/// A source position, one-based, as the lexer reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// A half-open span of one line, for an editor range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub line: usize,
    pub start_column: usize,
    pub end_column: usize,
}

impl Location {
    fn of(entry: &Entry) -> Self {
        Self {
            line: entry.span.line,
            start_column: entry.span.col,
            end_column: entry.span.col + entry.name.chars().count(),
        }
    }
}

/// One entry in the outline.
#[derive(Debug, Clone)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: DeclKind,
    pub detail: Option<String>,
    pub location: Location,
}

/// A callable's signature, for signature help and hovers.
#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub label: String,
    pub parameters: Vec<String>,
    /// Which parameter the cursor is on.
    pub active_parameter: usize,
}

/// An inlay hint: a parameter name shown before an argument.
#[derive(Debug, Clone)]
pub struct InlayHint {
    pub line: usize,
    pub column: usize,
    pub label: String,
}

/// Build the index for a source file.
pub fn index(source: &str) -> SymbolIndex {
    let tokens = Lexer::new(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(tokens);
    let _ = parser.parse();
    parser.index
}

fn entry_at(index: &SymbolIndex, at: Position) -> Option<&Entry> {
    index.entry_at(at.line, at.column)
}

/// The declaration the name under the cursor resolves to.
pub fn definition(index: &SymbolIndex, at: Position) -> Option<Location> {
    let entry = entry_at(index, at)?;
    index.resolve(entry).map(Location::of)
}

/// Every occurrence of the binding under the cursor, declaration included.
pub fn references(index: &SymbolIndex, at: Position) -> Vec<Location> {
    let Some(entry) = entry_at(index, at) else {
        return Vec::new();
    };
    index
        .occurrences_of(entry)
        .into_iter()
        .map(Location::of)
        .collect()
}

/// The edits that rename the binding under the cursor.
///
/// `None` when the cursor is not on a name, or when the new name is not an
/// identifier — renaming to `2 + 2` would produce a program that does not
/// parse, so it is refused rather than applied.
pub fn rename(index: &SymbolIndex, at: Position, new_name: &str) -> Option<Vec<Location>> {
    if !is_identifier(new_name) {
        return None;
    }
    entry_at(index, at)?;
    let edits = references(index, at);
    (!edits.is_empty()).then_some(edits)
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_alphabetic() || first == '_' || first == '$' => {
            chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        }
        _ => false,
    }
}

/// The document outline.
pub fn document_symbols(index: &SymbolIndex) -> Vec<DocumentSymbol> {
    index
        .outline()
        .into_iter()
        .filter_map(|entry| {
            let Occurrence::Declaration(kind) = entry.occurrence else {
                return None;
            };
            Some(DocumentSymbol {
                name: entry.name.clone(),
                kind,
                detail: entry.detail.clone(),
                location: Location::of(entry),
            })
        })
        .collect()
}

/// Signature help for the call the cursor is inside.
///
/// The callee is found by scanning back from the cursor for the identifier
/// that opened the innermost unclosed `(`, and the active parameter by
/// counting the commas at that depth.
pub fn signature_help(source: &str, at: Position, index: &SymbolIndex) -> Option<SignatureInfo> {
    let offset = offset_of(source, at)?;
    let chars: Vec<char> = source.chars().collect();
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut cursor = offset.min(chars.len());
    while cursor > 0 {
        cursor -= 1;
        match chars[cursor] {
            ')' => depth += 1,
            ',' if depth == 0 => commas += 1,
            '(' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
        if cursor == 0 {
            return None;
        }
    }
    // Read the identifier immediately before the open parenthesis.
    let mut end = cursor;
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && (chars[start - 1].is_alphanumeric() || matches!(chars[start - 1], '_' | '$'))
    {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let callee: String = chars[start..end].iter().collect();

    // The callee's parameter list comes from its declaration's detail.
    let declaration = index.entries.iter().find(|entry| {
        entry.name == callee
            && matches!(
                entry.occurrence,
                Occurrence::Declaration(DeclKind::Function)
                    | Occurrence::Declaration(DeclKind::Method)
            )
    })?;
    let detail = declaration
        .detail
        .clone()
        .unwrap_or_else(|| "()".to_string());
    let parameters: Vec<String> = detail
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    Some(SignatureInfo {
        label: format!("{}{}", callee, detail),
        active_parameter: commas.min(parameters.len().saturating_sub(1)),
        parameters,
    })
}

/// Parameter-name hints at call sites of functions declared in this document.
///
/// Only positional arguments that are *not* already the parameter's name are
/// hinted: `add(x, y)` where the parameters are `x, y` needs no annotation.
pub fn inlay_hints(source: &str, index: &SymbolIndex) -> Vec<InlayHint> {
    let mut hints = Vec::new();
    let tokens = Lexer::new(source).tokenize_with_spans();
    for (position, (token, span)) in tokens.iter().enumerate() {
        let crate::lexer::Token::Identifier(callee) = token else {
            continue;
        };
        if !matches!(
            tokens.get(position + 1).map(|(t, _)| t),
            Some(crate::lexer::Token::LParen)
        ) {
            continue;
        }
        let Some(declaration) = index.entries.iter().find(|entry| {
            entry.name == *callee
                && matches!(
                    entry.occurrence,
                    Occurrence::Declaration(DeclKind::Function)
                )
        }) else {
            continue;
        };
        // A recursive call would hint its own parameter list at the definition
        // site, which is noise.
        if declaration.span.line == span.line {
            continue;
        }
        let parameters: Vec<String> = declaration
            .detail
            .clone()
            .unwrap_or_default()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(',')
            .map(|p| p.trim().trim_start_matches("...").to_string())
            .filter(|p| !p.is_empty())
            .collect();
        collect_argument_hints(&tokens, position + 1, &parameters, &mut hints);
    }
    hints
}

/// Walk one argument list, emitting a hint at the start of each argument.
fn collect_argument_hints(
    tokens: &[(crate::lexer::Token, Span)],
    open_paren: usize,
    parameters: &[String],
    hints: &mut Vec<InlayHint>,
) {
    use crate::lexer::Token;
    let mut depth = 0i32;
    let mut argument = 0usize;
    let mut expect_start = true;
    for (token, span) in tokens.iter().skip(open_paren) {
        match token {
            Token::LParen | Token::LBracket | Token::LBrace => {
                if depth > 0 || !matches!(token, Token::LParen) {
                    // Nested grouping: only the outermost list is hinted.
                }
                depth += 1;
            }
            Token::RParen | Token::RBracket | Token::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            Token::Comma if depth == 1 => {
                argument += 1;
                expect_start = true;
                continue;
            }
            _ => {}
        }
        if expect_start && depth == 1 && !matches!(token, Token::LParen) {
            expect_start = false;
            let Some(parameter) = parameters.get(argument) else {
                continue;
            };
            // An argument that already reads as the parameter name says it.
            if matches!(token, Token::Identifier(name) if name == parameter) {
                continue;
            }
            hints.push(InlayHint {
                line: span.line,
                column: span.col,
                label: format!("{}:", parameter),
            });
        }
    }
}

/// Character offset of a one-based line/column position.
fn offset_of(source: &str, at: Position) -> Option<usize> {
    let mut line = 1usize;
    let mut column = 1usize;
    for (offset, c) in source.chars().enumerate() {
        if line == at.line && column == at.column {
            return Some(offset);
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line == at.line && column == at.column).then_some(source.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: usize, column: usize) -> Position {
        Position { line, column }
    }

    #[test]
    fn definition_points_at_the_declaration() {
        let source = "const total = 1;\nconsole.log(total);";
        let index = index(source);
        let location = definition(&index, at(2, 13)).expect("resolves");
        assert_eq!(location.line, 1);
        assert_eq!(location.start_column, 7);
    }

    #[test]
    fn references_include_the_declaration() {
        let source = "let v = 1;\nv = 2;\nv;";
        let index = index(source);
        let found = references(&index, at(1, 5));
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn references_respect_scope() {
        let source = "function a() { let v = 1; return v; }\nfunction b() { let v = 2; return v; }";
        let index = index(source);
        assert_eq!(references(&index, at(1, 20)).len(), 2);
    }

    #[test]
    fn rename_refuses_a_non_identifier() {
        let index = index("let v = 1; v;");
        assert!(rename(&index, at(1, 5), "2 + 2").is_none());
        assert!(rename(&index, at(1, 5), "renamed").is_some());
    }

    #[test]
    fn document_symbols_list_declarations() {
        let index = index("function f(a, b) {}\nclass C {}\nconst x = 1;");
        let symbols = document_symbols(&index);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"f"));
        assert!(names.contains(&"C"));
        assert!(names.contains(&"x"));
        let f = symbols.iter().find(|s| s.name == "f").expect("f");
        assert_eq!(f.detail.as_deref(), Some("(a, b)"));
        assert_eq!(f.location.line, 1);
    }

    #[test]
    fn signature_help_reports_the_active_parameter() {
        let source = "function add(a, b) { return a + b; }\nadd(1, ";
        let index = index(source);
        let help = signature_help(source, at(2, 8), &index).expect("inside the call");
        assert_eq!(help.label, "add(a, b)");
        assert_eq!(help.active_parameter, 1);
    }

    #[test]
    fn signature_help_outside_a_call_is_none() {
        let source = "function add(a, b) {}\nconst x = 1;";
        let index = index(source);
        assert!(signature_help(source, at(2, 12), &index).is_none());
    }

    #[test]
    fn inlay_hints_name_positional_arguments() {
        let source = "function move(x, y) {}\nmove(1, 2);";
        let hints = inlay_hints(source, &index(source));
        let labels: Vec<&str> = hints.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["x:", "y:"]);
    }

    #[test]
    fn an_argument_that_already_names_the_parameter_is_not_hinted() {
        let source = "function move(x, y) {}\nconst x = 1;\nmove(x, 2);";
        let hints = inlay_hints(source, &index(source));
        let labels: Vec<&str> = hints.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["y:"]);
    }
}
