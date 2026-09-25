mod ast;
mod cache;
mod compound;
mod expr;
mod index;
mod primary;
mod stmt;

pub use ast::*;
pub(crate) use cache::parse_cached;
pub use index::{DeclKind, Entry, Occurrence, ScopeNode, SymbolIndex};

use crate::lexer::Token;
use crate::span::Span;

/// Maximum statement/expression nesting the parser accepts. The parser is
/// recursive descent, so each nesting level costs native stack frames;
/// 100k-deep parentheses would overflow the stack and SIGSEGV the host.
/// Each recursive expression level passes through several parser functions,
/// so even 256 levels can exhaust the smaller native stacks used by embedded
/// runtimes before this guard unwinds. Legitimate code rarely nests beyond a
/// few dozen levels.
const MAX_PARSE_DEPTH: u32 = 64;

#[cfg(test)]
thread_local! {
    /// Test-only count of whole-program parses on this thread. Thread-local
    /// so parallel tests never observe each other's parsing; steady-state
    /// plugin calls must leave it unchanged, proving the hot path parses
    /// nothing.
    pub static PARSE_PROGRAM_COUNT: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

/// Render a token the way a syntax error should name it.
///
/// Literals and identifiers are shown with their text, so the message points
/// at something the reader can find in the source; everything else falls back
/// to the token's debug name.
fn describe(token: &Token) -> String {
    match token {
        Token::EOF => "end of input".to_string(),
        Token::Number(n) => format!("number `{n}`"),
        Token::String(s) => format!("string `{s}`"),
        Token::Identifier(name) => format!("`{name}`"),
        Token::Unknown(c) => format!("`{c}`"),
        other => format!("`{other:?}`"),
    }
}

pub struct Parser {
    toks: Vec<(Token, Span)>,
    pos: usize,
    /// Sentinel EOF token used as a fallback when pos is out of bounds.
    eof_tok: (Token, Span),
    /// Current statement/expression nesting depth.
    depth: u32,
    /// Set once when nesting exceeds `MAX_PARSE_DEPTH`; `parse()` stops and
    /// callers (the NAPI layer) surface it as an error.
    pub depth_exceeded: bool,
    /// The first syntax error encountered, if any.
    ///
    /// Parsing continues after recording it so tooling can still collect an
    /// approximate tree and further diagnostics, but execution entry points
    /// refuse to run a program whose parse failed. Only the first error is
    /// kept: everything after it is likely a cascade from the same mistake.
    error: Option<ParseError>,
    /// Name occurrences and their scopes, recorded as parsing proceeds. The
    /// AST has no spans, so this is what the language server points at.
    pub index: index::SymbolIndex,
    /// The scope occurrences are currently recorded into.
    current_scope: usize,
}

/// A syntax error, with the source position of the token that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.span.is_unknown() {
            write!(f, "SyntaxError: {}", self.message)
        } else {
            write!(
                f,
                "SyntaxError: {} at {}:{}",
                self.message, self.span.line, self.span.col
            )
        }
    }
}

impl Parser {
    pub fn new(t: Vec<Token>) -> Self {
        Self {
            toks: t.into_iter().map(|t| (t, Span::unknown())).collect(),
            pos: 0,
            eof_tok: (Token::EOF, Span::unknown()),
            depth: 0,
            depth_exceeded: false,
            error: None,
            index: index::SymbolIndex::default(),
            current_scope: 0,
        }
    }

    pub fn new_with_spans(t: Vec<(Token, Span)>) -> Self {
        Self {
            toks: t,
            pos: 0,
            eof_tok: (Token::EOF, Span::unknown()),
            depth: 0,
            depth_exceeded: false,
            error: None,
            index: index::SymbolIndex::default(),
            current_scope: 0,
        }
    }

    /// Parse a whole program, recovering after errors so tooling still gets a
    /// tree. Check [`Parser::error`] before executing the result --
    /// [`Parser::parse_program`] does that for you.
    pub fn parse(&mut self) -> Vec<Statement> {
        let mut s = Vec::new();
        while !self.eof() && !self.depth_exceeded {
            if let Some(st) = self.stmt() {
                s.push(st);
            } else {
                // Record the position of the token that could not start a
                // statement, then skip it and keep going. Without the record,
                // a malformed program used to parse "successfully" into a
                // partial tree and then run.
                self.record_error(format!("unexpected token {}", describe(self.cur())));
                self.adv();
            }
        }
        s
    }

    /// Parse a whole program, refusing to return a tree that did not parse.
    ///
    /// This is what execution should use: running the salvaged half of a
    /// malformed program is worse than reporting where it broke.
    pub fn parse_program(&mut self) -> Result<Vec<Statement>, ParseError> {
        #[cfg(test)]
        PARSE_PROGRAM_COUNT.with(|count| count.set(count.get() + 1));
        let stmts = self.parse();
        if self.depth_exceeded {
            return Err(ParseError {
                message: "maximum parse depth exceeded".to_string(),
                span: self.cur_span(),
            });
        }
        match self.error.take() {
            Some(error) => Err(error),
            None => Ok(stmts),
        }
    }

    /// The first syntax error recorded, if any.
    pub fn error(&self) -> Option<&ParseError> {
        self.error.as_ref()
    }

    /// Record a syntax error at the current token. The first error wins:
    /// later ones are usually cascades from the same mistake.
    pub(crate) fn record_error(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(ParseError {
                message,
                span: self.cur_span(),
            });
        }
    }

    /// Loop condition for a delimited list: true while the cursor is neither
    /// at `close` nor at end of input.
    ///
    /// Every such loop must use this. Testing only for the closing token spins
    /// forever on truncated input, because `cur()` keeps returning `EOF` once
    /// the tokens run out and `adv()` cannot move past the end. That turned a
    /// malformed program like `function f( { }` into an unkillable loop in the
    /// interpreter thread -- parsing happens before the loop budget exists, so
    /// nothing interrupted it.
    pub(crate) fn until(&mut self, close: &Token) -> bool {
        if self.eof() {
            self.record_error(format!(
                "unexpected end of input, expected {}",
                describe(close)
            ));
            return false;
        }
        self.cur() != close
    }

    /// Consume `tok`, or record a syntax error naming what was expected.
    ///
    /// Use this wherever the grammar *requires* a token -- the `)` of an `if`
    /// head, the `{` of a block. `eat` returns a bool that is easy to ignore,
    /// and ignoring it is how `if (true { }` came to parse as valid.
    pub(crate) fn expect(&mut self, tok: &Token) -> bool {
        if self.eat(tok) {
            return true;
        }
        self.record_error(format!(
            "expected {}, found {}",
            describe(tok),
            describe(self.cur())
        ));
        false
    }

    /// Open a lexical scope and make it current. `is_function` marks a
    /// function or class body, which is where `var` hoisting stops.
    pub(crate) fn push_scope(&mut self, is_function: bool) -> usize {
        let parent = self.current_scope;
        self.index.scopes.push(index::ScopeNode {
            parent: Some(parent),
            is_function,
        });
        self.current_scope = self.index.scopes.len() - 1;
        parent
    }

    /// Restore the scope `push_scope` displaced.
    pub(crate) fn pop_scope(&mut self, parent: usize) {
        self.current_scope = parent;
    }

    /// Record a name occurrence at `span`.
    pub(crate) fn record(
        &mut self,
        name: &str,
        span: Span,
        occurrence: index::Occurrence,
        detail: Option<String>,
    ) {
        // A synthesized name (a desugaring's placeholder) has no source
        // position and nothing to point at.
        if span.line == 0 || name.is_empty() {
            return;
        }
        self.index.entries.push(index::Entry {
            name: name.to_string(),
            span,
            occurrence,
            scope: self.current_scope,
            detail,
        });
    }

    /// Span of the token under the cursor.
    pub(crate) fn cur_span(&self) -> Span {
        self.toks.get(self.pos).unwrap_or(&self.eof_tok).1
    }

    /// Enter one level of statement/expression nesting. Returns `false` (and
    /// latches `depth_exceeded`) once the limit is passed; callers return
    /// `None` so parsing unwinds instead of recursing further.
    pub(crate) fn enter(&mut self) -> bool {
        if self.depth_exceeded {
            return false;
        }
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            self.depth_exceeded = true;
            false
        } else {
            true
        }
    }

    /// Leave one nesting level (paired with a successful `enter`).
    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub(crate) fn cur(&self) -> &Token {
        &self.toks.get(self.pos).unwrap_or(&self.eof_tok).0
    }

    pub(crate) fn peek(&self) -> &Token {
        &self.toks.get(self.pos + 1).unwrap_or(&self.eof_tok).0
    }

    pub(crate) fn adv(&mut self) -> &Token {
        if self.pos < self.toks.len() {
            self.pos += 1;
        }
        &self.toks.get(self.pos - 1).unwrap_or(&self.eof_tok).0
    }

    /// Is the current token `keyword` acting as the `get`/`set` prefix of an
    /// accessor, rather than as an ordinary property name?
    ///
    /// It is an accessor only when a property name follows. `{ get: 1 }`,
    /// `{ get() {} }`, `{ get }` and `{ get, x }` all name a property `get`.
    pub(crate) fn starts_accessor(&self, keyword: &Token) -> bool {
        self.cur() == keyword
            && matches!(
                self.peek(),
                Token::Identifier(_)
                    | Token::String(_)
                    | Token::Number(_)
                    | Token::LBracket
                    | Token::KwGet
                    | Token::KwSet
            )
    }

    pub(crate) fn eat(&mut self, t: &Token) -> bool {
        if self.cur() == t {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn eof(&self) -> bool {
        matches!(self.cur(), Token::EOF)
    }

    pub(crate) fn semi(&mut self) {
        if matches!(self.cur(), Token::Semicolon) {
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Vec<Statement> {
        let mut lex = Lexer::new(src);
        let toks = lex.tokenize_with_spans();
        let mut parser = Parser::new_with_spans(toks);
        parser.parse()
    }

    #[test]
    fn test_var_decl() {
        let stmts = parse("const x = 42;");
        assert_eq!(stmts.len(), 1);
        assert!(
            matches!(&stmts[0], Statement::VarDecl { kind: VarKind::Const, name, .. } if name == "x")
        );
    }

    #[test]
    fn test_fn_decl() {
        let stmts = parse("function add(a, b) { return a + b; }");
        assert_eq!(stmts.len(), 1);
        assert!(
            matches!(&stmts[0], Statement::FnDecl { name, params, .. } if name == "add" && params.len() == 2)
        );
    }

    #[test]
    fn test_if_else() {
        let stmts = parse("if (true) { 1; } else { 2; }");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::If { else_: Some(_), .. }));
    }

    #[test]
    fn test_for_loop() {
        let stmts = parse("for (let i = 0; i < 10; i++) { i; }");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::For { .. }));
    }

    #[test]
    fn test_while_loop() {
        let stmts = parse("while (true) { break; }");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::While { .. }));
    }

    #[test]
    fn test_arrow_fn() {
        let stmts = parse("const f = (x) => x * 2;");
        assert_eq!(stmts.len(), 1);
        if let Statement::VarDecl {
            init: Some(init), ..
        } = &stmts[0]
        {
            assert!(matches!(init.as_ref(), Expr::ArrowFn { .. }));
        } else {
            panic!("expected var decl with init");
        }
    }

    #[test]
    fn test_class_decl() {
        let stmts = parse("class Foo { constructor() {} bar() {} }");
        assert_eq!(stmts.len(), 1);
        assert!(
            matches!(&stmts[0], Statement::ClassDecl { name, body, .. } if name == "Foo" && body.len() == 2)
        );
    }

    #[test]
    fn test_try_catch() {
        let stmts = parse("try { throw 'x'; } catch(e) { e; }");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::Try { catch: Some(_), .. }));
    }

    #[test]
    fn test_switch() {
        let stmts = parse("switch (x) { case 1: break; default: break; }");
        assert_eq!(stmts.len(), 1);
        if let Statement::Switch { cases, .. } = &stmts[0] {
            assert_eq!(cases.len(), 2);
        } else {
            panic!("expected switch");
        }
    }

    #[test]
    fn test_import() {
        let stmts = parse("import { foo } from 'bar';");
        assert_eq!(stmts.len(), 1);
        assert!(
            matches!(&stmts[0], Statement::Import { module, named, .. } if module == "bar" && named.len() == 1)
        );
    }

    #[test]
    fn test_export_default() {
        let stmts = parse("export default 42;");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::ExportDefault(_)));
    }

    /// `export default class`/`function` are declarations: the parser binds
    /// the name (synthesizing one when anonymous) and exports that binding.
    #[test]
    fn test_export_default_class_declaration() {
        for source in [
            "export default class P { hi() { return 1; } }",
            "export default class { hi() { return 1; } }",
        ] {
            let stmts = parse(source);
            assert_eq!(stmts.len(), 1);
            // `export default <declaration>` desugars to a scope-transparent
            // declarator group: the binding must land in the module scope, not
            // in a block of its own.
            let Statement::Declarations(inner) = &stmts[0] else {
                panic!("expected a desugared declaration group for {source}");
            };
            assert!(matches!(&inner[0], Statement::ClassDecl { .. }));
            assert!(matches!(&inner[1], Statement::ExportDefault(_)));
        }
    }

    #[test]
    fn test_export_default_function_declaration() {
        for source in [
            "export default function f() { return 1; }",
            "export default function () { return 1; }",
            "export default async function () { return 1; }",
        ] {
            let stmts = parse(source);
            assert_eq!(stmts.len(), 1);
            // `export default <declaration>` desugars to a scope-transparent
            // declarator group: the binding must land in the module scope, not
            // in a block of its own.
            let Statement::Declarations(inner) = &stmts[0] else {
                panic!("expected a desugared declaration group for {source}");
            };
            assert!(matches!(&inner[0], Statement::FnDecl { .. }));
            assert!(matches!(&inner[1], Statement::ExportDefault(_)));
        }
    }

    /// `get` / `set` / `static` are modifiers only when a member name follows.
    #[test]
    fn test_class_members_named_like_modifiers() {
        let stmts = parse("class A { get() { return 1; } set(v) { return v; } static() {} }");
        let Statement::ClassDecl { body, .. } = &stmts[0] else {
            panic!("expected a class declaration");
        };
        assert_eq!(body.len(), 3);
        for member in body {
            assert!(matches!(member, ClassMember::Method { .. }));
        }
    }

    #[test]
    fn test_class_accessors_still_parse() {
        let stmts = parse("class A { get value() { return 1; } set value(v) {} }");
        let Statement::ClassDecl { body, .. } = &stmts[0] else {
            panic!("expected a class declaration");
        };
        assert!(matches!(
            &body[0],
            ClassMember::Getter {
                name: MemberName::Static(name),
                ..
            } if name == "value"
        ));
        assert!(matches!(
            &body[1],
            ClassMember::Setter {
                name: MemberName::Static(name),
                ..
            } if name == "value"
        ));
    }

    #[test]
    fn test_binary_precedence() {
        let stmts = parse("1 + 2 * 3;");
        assert_eq!(stmts.len(), 1);
        if let Statement::Expr(Expr::Binary { op, right, .. }) = &stmts[0] {
            assert_eq!(*op, BinOp::Add);
            assert!(matches!(right.as_ref(), Expr::Binary { op, .. } if *op == BinOp::Mul));
        } else {
            panic!("expected binary expr");
        }
    }

    #[test]
    fn test_ternary() {
        let stmts = parse("true ? 1 : 2;");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(
            &stmts[0],
            Statement::Expr(Expr::Conditional { .. })
        ));
    }

    #[test]
    fn test_member_access() {
        let stmts = parse("obj.prop;");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(
            &stmts[0],
            Statement::Expr(Expr::Member {
                computed: false,
                ..
            })
        ));
    }

    #[test]
    fn test_computed_member() {
        let stmts = parse("arr[0];");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(
            &stmts[0],
            Statement::Expr(Expr::Member { computed: true, .. })
        ));
    }

    #[test]
    fn test_new_expr() {
        let stmts = parse("new Foo(1, 2);");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::Expr(Expr::New { .. })));
    }

    #[test]
    fn test_for_of() {
        let stmts = parse("for (const x of arr) { x; }");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::ForOf { .. }));
    }

    #[test]
    fn test_for_in() {
        let stmts = parse("for (const k in obj) { k; }");
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Statement::ForIn { .. }));
    }
}
