//! Lightweight, always-safe diagnostics.
//!
//! The interpreter's parser is error-tolerant (it skips tokens it cannot
//! understand rather than reporting them), so it does not produce structured
//! errors today. These diagnostics cover the two classes of problem that matter
//! most while editing and that can be detected without modifying the core:
//!
//!   * unbalanced delimiters — `( [ {` and their closers, including the `${`
//!     of template literals, and
//!   * runaway nesting — the parser's own depth limit.
//!
//! Precise, span-accurate parse-error reporting is a follow-up that belongs
//! with the LSP work (it requires the parser to collect errors).

use crate::lexer::{Lexer, Token};
use crate::parser::Parser;
use crate::span::Span;

use super::{Diagnostic, DiagnosticSeverity};

pub fn diagnose(source: &str) -> Vec<Diagnostic> {
    let toks = Lexer::new(source).tokenize_with_spans();
    let mut diags = Vec::new();

    check_delimiters(&toks, &mut diags);
    check_depth(&toks, &mut diags);

    diags
}

#[derive(Clone, Copy, PartialEq)]
enum Opener {
    Paren,
    Brace,
    Bracket,
    TemplateExpr, // the `${` inside a template literal
}

impl Opener {
    fn char(self) -> &'static str {
        match self {
            Opener::Paren => "(",
            Opener::Brace => "{",
            Opener::Bracket => "[",
            Opener::TemplateExpr => "${",
        }
    }
}

fn opener(tok: &Token) -> Option<Opener> {
    Some(match tok {
        Token::LParen => Opener::Paren,
        Token::LBrace => Opener::Brace,
        Token::LBracket => Opener::Bracket,
        Token::DollarLBrace => Opener::TemplateExpr,
        _ => return None,
    })
}

/// The opener a closing token matches, plus its display char.
fn closer(tok: &Token) -> Option<(Opener, &'static str)> {
    Some(match tok {
        Token::RParen => (Opener::Paren, ")"),
        Token::RBrace => (Opener::Brace, "}"),
        Token::RBracket => (Opener::Bracket, "]"),
        _ => return None,
    })
}

fn check_delimiters(toks: &[(Token, Span)], diags: &mut Vec<Diagnostic>) {
    let mut stack: Vec<(Opener, Span)> = Vec::new();

    for (tok, span) in toks {
        if let Some(op) = opener(tok) {
            stack.push((op, *span));
            continue;
        }
        if let Some((expected, ch)) = closer(tok) {
            match stack.pop() {
                Some((op, _)) if op == expected => {}
                Some((op, ospan)) => {
                    // A `${` is closed by a plain `}`.
                    let ok = matches!((op, expected), (Opener::TemplateExpr, Opener::Brace));
                    if ok {
                        continue;
                    }
                    diags.push(Diagnostic {
                        line: span.line,
                        col: span.col,
                        message: format!(
                            "Mismatched '{}': does not close '{}' opened at {}",
                            ch,
                            op.char(),
                            ospan
                        ),
                        severity: DiagnosticSeverity::Error,
                    });
                    // State is now ambiguous; stop cascading further errors.
                    stack.clear();
                }
                None => {
                    diags.push(Diagnostic {
                        line: span.line,
                        col: span.col,
                        message: format!("Unmatched closing '{}'", ch),
                        severity: DiagnosticSeverity::Error,
                    });
                }
            }
        }
    }

    for (op, ospan) in stack {
        diags.push(Diagnostic {
            line: ospan.line,
            col: ospan.col,
            message: format!("Unclosed '{}'", op.char()),
            severity: DiagnosticSeverity::Error,
        });
    }
}

fn check_depth(toks: &[(Token, Span)], diags: &mut Vec<Diagnostic>) {
    let mut parser = Parser::new_with_spans(toks.to_vec());
    let _ = parser.parse();
    if parser.depth_exceeded {
        diags.push(Diagnostic {
            line: 1,
            col: 1,
            message: "Maximum parse depth exceeded (expression nested too deeply)".to_string(),
            severity: DiagnosticSeverity::Error,
        });
    }
}
