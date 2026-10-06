//! A symbol index built while parsing.
//!
//! The AST carries no spans, so the language server has nothing to point at.
//! Rather than annotate every node, the parser records each *name occurrence*
//! as it consumes it: where the name is, whether it declares or reads, and
//! which scope it belongs to.
//!
//! Scopes come from the parser itself — it already knows where a block, a
//! function body or a class body begins and ends — so resolution here is as
//! accurate as the parse. That is what makes rename safe: two `x`es in
//! different function bodies resolve to different declarations, and renaming
//! one leaves the other alone.

use crate::span::Span;

/// What a name occurrence is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occurrence {
    /// A binding site: `let x`, a parameter, `function f`, `class C`.
    Declaration(DeclKind),
    /// A read or write of a name declared elsewhere.
    Reference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Variable,
    Function,
    Class,
    Parameter,
    Import,
    Method,
    Property,
}

impl DeclKind {
    /// The LSP `SymbolKind` number for this declaration.
    pub fn lsp_kind(self) -> u32 {
        match self {
            DeclKind::Variable => 13,
            DeclKind::Function => 12,
            DeclKind::Class => 5,
            DeclKind::Parameter => 13,
            DeclKind::Import => 13,
            DeclKind::Method => 6,
            DeclKind::Property => 7,
        }
    }
}

/// One name, where it appeared, and what it was doing there.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub span: Span,
    pub occurrence: Occurrence,
    /// Index into [`SymbolIndex::scopes`].
    pub scope: usize,
    /// Extra text for an outline entry: a parameter list, or a declaration
    /// keyword.
    pub detail: Option<String>,
}

/// A lexical scope: its parent, so a lookup can walk outwards.
#[derive(Debug, Clone)]
pub struct ScopeNode {
    pub parent: Option<usize>,
    /// `true` for a function or class body, which is where `var` and function
    /// declarations stop hoisting outwards.
    pub is_function: bool,
}

#[derive(Debug, Clone)]
pub struct SymbolIndex {
    pub entries: Vec<Entry>,
    pub scopes: Vec<ScopeNode>,
}

impl Default for SymbolIndex {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            scopes: vec![ScopeNode {
                parent: None,
                is_function: true,
            }],
        }
    }
}

impl SymbolIndex {
    /// The declaration a name occurrence resolves to: the nearest enclosing
    /// scope that declares it.
    pub fn resolve(&self, entry: &Entry) -> Option<&Entry> {
        let mut scope = Some(entry.scope);
        while let Some(current) = scope {
            if let Some(found) = self.entries.iter().find(|candidate| {
                candidate.scope == current
                    && candidate.name == entry.name
                    && matches!(candidate.occurrence, Occurrence::Declaration(_))
            }) {
                return Some(found);
            }
            scope = self.scopes.get(current).and_then(|node| node.parent);
        }
        None
    }

    /// Is `scope` inside `ancestor` (or the same scope)?
    fn descends_from(&self, mut scope: usize, ancestor: usize) -> bool {
        loop {
            if scope == ancestor {
                return true;
            }
            match self.scopes.get(scope).and_then(|node| node.parent) {
                Some(parent) => scope = parent,
                None => return false,
            }
        }
    }

    /// Every occurrence — the declaration and all its references — that
    /// resolves to the same binding as `entry`.
    ///
    /// A same-named binding in a sibling scope is excluded, which is what
    /// makes rename safe.
    pub fn occurrences_of(&self, entry: &Entry) -> Vec<&Entry> {
        let Some(declaration) = self.resolve(entry) else {
            // An unresolved name (a global, or a typo) matches by name alone;
            // there is no binding to scope it to.
            return self
                .entries
                .iter()
                .filter(|candidate| candidate.name == entry.name)
                .collect();
        };
        self.entries
            .iter()
            .filter(|candidate| {
                candidate.name == declaration.name
                    && self.descends_from(candidate.scope, declaration.scope)
                    // A candidate in a nested scope that redeclares the name
                    // belongs to that other binding, not this one.
                    && self
                        .resolve(candidate)
                        .is_some_and(|target| std::ptr::eq(target, declaration))
            })
            .collect()
    }

    /// The entry whose name covers `(line, column)`, if any.
    ///
    /// Positions are one-based lines and columns, matching the lexer's spans.
    pub fn entry_at(&self, line: usize, column: usize) -> Option<&Entry> {
        self.entries.iter().find(|entry| {
            entry.span.line == line
                && column >= entry.span.col
                && column < entry.span.col + entry.name.chars().count()
        })
    }

    /// Top-level declarations, for an outline.
    pub fn outline(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|entry| {
                matches!(entry.occurrence, Occurrence::Declaration(_))
                    && !matches!(
                        entry.occurrence,
                        Occurrence::Declaration(DeclKind::Parameter)
                    )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::lexer::Lexer;
    use crate::parser::{Occurrence, Parser};

    fn index_of(source: &str) -> crate::parser::SymbolIndex {
        let tokens = Lexer::new(source).tokenize_with_spans();
        let mut parser = Parser::new_with_spans(tokens);
        let _ = parser.parse();
        parser.index
    }

    #[test]
    fn a_declaration_is_recorded_with_its_position() {
        let index = index_of("const answer = 42;");
        let entry = index
            .entries
            .iter()
            .find(|e| e.name == "answer")
            .expect("declaration recorded");
        assert_eq!(entry.span.line, 1);
        assert_eq!(entry.span.col, 7);
        assert!(matches!(entry.occurrence, Occurrence::Declaration(_)));
    }

    #[test]
    fn a_reference_resolves_to_its_declaration() {
        let index = index_of("const x = 1;\nconsole.log(x);");
        let reference = index.entry_at(2, 13).expect("reference at the cursor");
        assert!(matches!(reference.occurrence, Occurrence::Reference));
        let declaration = index.resolve(reference).expect("resolves");
        assert_eq!(declaration.span.line, 1);
    }

    #[test]
    fn sibling_scopes_do_not_share_a_binding() {
        let index = index_of(
            "function a() { let v = 1; return v; }\nfunction b() { let v = 2; return v; }",
        );
        let first = index.entry_at(1, 20).expect("first declaration");
        let occurrences = index.occurrences_of(first);
        // The declaration and its one reference, and nothing from `b`.
        assert_eq!(occurrences.len(), 2);
        assert!(occurrences.iter().all(|e| e.span.line == 1));
    }

    #[test]
    fn an_inner_declaration_shadows_an_outer_one() {
        let index = index_of("let v = 1;\nfunction f() { let v = 2; return v; }\nv;");
        let outer = index.entry_at(1, 5).expect("outer declaration");
        let outer_occurrences = index.occurrences_of(outer);
        // The outer `v` and its use on line 3 — not the shadowed inner one.
        assert_eq!(outer_occurrences.len(), 2);
        assert!(outer_occurrences.iter().all(|e| e.span.line != 2));
    }

    #[test]
    fn a_parameter_is_a_declaration_in_the_body_scope() {
        let index = index_of("function f(value) { return value; }");
        let parameter = index.entry_at(1, 12).expect("parameter");
        assert_eq!(index.occurrences_of(parameter).len(), 2);
    }

    #[test]
    fn the_outline_lists_declarations() {
        let index = index_of("function f() {}\nclass C {}\nconst x = 1;");
        let names: Vec<&str> = index
            .outline()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert!(names.contains(&"f"));
        assert!(names.contains(&"C"));
        assert!(names.contains(&"x"));
    }
}
