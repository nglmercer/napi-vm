//! Code actions: quick fixes for the diagnostics the server reports.
//!
//! Only fixes that are *unambiguous* are offered. A guess about what the
//! author meant, applied silently by an editor, costs more than no fix at all —
//! so an unclosed `(` gets "insert the missing `)`", and a mismatched pair,
//! where the intent could be either side, gets nothing.

use super::{Diagnostic, format_source::FormatOptions};

/// One edit: replace `[start, end)` on `line` with `text`.
///
/// Positions are one-based, matching the diagnostics they come from. An empty
/// range is an insertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    /// `"quickfix"` or `"source.fixAll"`, as the protocol names them.
    pub kind: &'static str,
    pub edits: Vec<TextEdit>,
}

/// Quick fixes for the diagnostics overlapping the requested line range,
/// plus the whole-document actions that are always available.
pub fn code_actions(
    source: &str,
    diagnostics: &[Diagnostic],
    start_line: usize,
    end_line: usize,
) -> Vec<CodeAction> {
    let mut actions = Vec::new();
    for diagnostic in diagnostics {
        if diagnostic.line < start_line || diagnostic.line > end_line {
            continue;
        }
        if let Some(action) = fix_for(source, diagnostic) {
            actions.push(action);
        }
    }
    if let Some(action) = format_action(source) {
        actions.push(action);
    }
    actions
}

/// The closing delimiter an "Unclosed 'x'" diagnostic is missing.
fn closer_for(message: &str) -> Option<&'static str> {
    let opener = message.strip_prefix("Unclosed '")?.strip_suffix('\'')?;
    Some(match opener {
        "(" => ")",
        "[" => "]",
        "{" | "${" => "}",
        _ => return None,
    })
}

fn fix_for(source: &str, diagnostic: &Diagnostic) -> Option<CodeAction> {
    // An unclosed delimiter: append its closer at the end of the document,
    // which is where it can go without changing what any existing line means.
    if let Some(closer) = closer_for(&diagnostic.message) {
        let lines: Vec<&str> = source.split('\n').collect();
        let last = lines.len().max(1);
        let column = lines.last().map(|l| l.chars().count()).unwrap_or(0) + 1;
        return Some(CodeAction {
            title: format!("Insert the missing '{}'", closer),
            kind: "quickfix",
            edits: vec![TextEdit {
                line: last,
                start_column: column,
                end_column: column,
                text: closer.to_string(),
            }],
        });
    }

    // A stray closer: removing it is the only edit that cannot be wrong, since
    // there is no opener it could have belonged to.
    if diagnostic.message.starts_with("Unmatched closing '") {
        let character = diagnostic.message.chars().nth(19)?;
        return Some(CodeAction {
            title: format!("Remove the unmatched '{}'", character),
            kind: "quickfix",
            edits: vec![TextEdit {
                line: diagnostic.line,
                start_column: diagnostic.col,
                end_column: diagnostic.col + 1,
                text: String::new(),
            }],
        });
    }

    None
}

/// "Fix indentation", offered when the document is not already formatted.
fn format_action(source: &str) -> Option<CodeAction> {
    let formatted = super::format_source::format_source(source, &FormatOptions::default());
    if formatted == source {
        return None;
    }
    Some(CodeAction {
        title: "Fix indentation".to_string(),
        kind: "source.fixAll",
        edits: vec![TextEdit {
            line: 1,
            start_column: 1,
            // A whole-document replacement: the end is past the last line.
            end_column: usize::MAX,
            text: formatted,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::diagnose;

    fn actions_for(source: &str) -> Vec<CodeAction> {
        code_actions(source, &diagnose(source), 1, usize::MAX)
    }

    #[test]
    fn an_unclosed_paren_offers_its_closer() {
        let actions = actions_for("const a = (1;");
        let fix = actions
            .iter()
            .find(|a| a.kind == "quickfix")
            .expect("a quick fix");
        assert_eq!(fix.title, "Insert the missing ')'");
        assert_eq!(fix.edits[0].text, ")");
    }

    #[test]
    fn an_unclosed_brace_offers_its_closer() {
        let actions = actions_for("function f() {");
        assert!(actions.iter().any(|a| a.title == "Insert the missing '}'"));
    }

    #[test]
    fn a_stray_closer_offers_removal() {
        let actions = actions_for("const a = 1);");
        let fix = actions
            .iter()
            .find(|a| a.kind == "quickfix")
            .expect("a quick fix");
        assert_eq!(fix.title, "Remove the unmatched ')'");
        assert_eq!(fix.edits[0].text, "");
    }

    #[test]
    fn a_mismatched_pair_offers_no_guess() {
        // Either side could be the mistake, so neither is proposed.
        let actions = actions_for("const a = (1];");
        assert!(actions.iter().all(|a| a.kind != "quickfix"));
    }

    #[test]
    fn badly_indented_source_offers_a_format() {
        let actions = actions_for("function f() {\nreturn 1;\n}");
        let format = actions
            .iter()
            .find(|a| a.kind == "source.fixAll")
            .expect("a format action");
        assert_eq!(format.title, "Fix indentation");
        assert!(format.edits[0].text.contains("  return 1;"));
    }

    #[test]
    fn well_formatted_source_offers_nothing() {
        assert!(actions_for("const a = 1;\n").is_empty());
    }

    #[test]
    fn a_diagnostic_outside_the_range_is_skipped() {
        let source = "const a = 1;\nconst b = (2;";
        assert!(
            code_actions(source, &diagnose(source), 1, 1)
                .iter()
                .all(|a| a.kind != "quickfix")
        );
    }
}
