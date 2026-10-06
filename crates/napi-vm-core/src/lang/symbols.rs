//! Document symbols: the top-level declarations of a program, for an outline
//! view. Kept to top level (functions, classes, variables) which is what a
//! playground outline needs; nested symbols can be added once the AST is
//! span-annotated.

use crate::parser::{Statement, VarKind};

use super::{CompletionKind, Symbol};

pub fn symbols(stmts: &[Statement]) -> Vec<Symbol> {
    let mut out = Vec::new();
    for s in stmts {
        match s {
            Statement::FnDecl { name, params, .. } => out.push(Symbol {
                name: name.clone(),
                kind: CompletionKind::Function,
                detail: Some(format!("({})", params.join(", "))),
            }),
            Statement::ClassDecl { name, .. } => out.push(Symbol {
                name: name.clone(),
                kind: CompletionKind::Class,
                detail: None,
            }),
            Statement::VarDecl { name, kind, .. } => out.push(Symbol {
                name: name.clone(),
                kind: CompletionKind::Variable,
                detail: Some(
                    match kind {
                        VarKind::Var => "var",
                        VarKind::Let => "let",
                        VarKind::Const => "const",
                    }
                    .to_string(),
                ),
            }),
            _ => {}
        }
    }
    out
}
