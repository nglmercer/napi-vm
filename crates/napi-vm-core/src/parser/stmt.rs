//! Core statement parsing. Classes and `import` / `export` live in
//! `compound.rs`.

use super::{Expr, ForInit, Parser, Pattern, PatternKey, Statement, SwitchCase, VarKind};
use crate::lexer::Token;

impl Parser {
    pub(crate) fn stmt(&mut self) -> Option<Statement> {
        // Nesting guard for nested blocks / control flow; see `Parser::enter`.
        if !self.enter() {
            return None;
        }
        let r = self.stmt_inner();
        self.leave();
        r
    }

    fn stmt_inner(&mut self) -> Option<Statement> {
        match self.cur() {
            Token::Identifier(name)
                if name == "using"
                    && !self.line_break_after_current()
                    && matches!(self.peek(), Token::Identifier(_)) =>
            {
                if self.single_statement {
                    self.record_error("resource declaration requires a statement list".into());
                }
                self.resource_declaration(false)
            }
            Token::KwAwait
                if matches!(self.peek(), Token::Identifier(name) if name == "using")
                    && !self.line_break_after_current()
                    && self
                        .toks
                        .get(self.pos + 2)
                        .is_some_and(|(token, _)| matches!(token, Token::Identifier(_))) =>
            {
                if self.single_statement {
                    self.record_error("resource declaration requires a statement list".into());
                }
                self.adv();
                self.resource_declaration(true)
            }
            Token::KwWith => {
                self.adv();
                self.expect(&Token::LParen);
                let object = self.expr()?;
                self.expect(&Token::RParen);
                let body = self.block_or_stmt();
                Some(Statement::With {
                    object: Box::new(object),
                    body,
                })
            }
            Token::KwVar => self.var_decl(VarKind::Var),
            Token::KwLet
                if self.single_statement
                    || !matches!(
                        self.peek(),
                        Token::Identifier(_)
                            | Token::LBracket
                            | Token::LBrace
                            | Token::KwAwait
                            | Token::KwYield
                            | Token::KwLet
                            | Token::KwAsync
                            | Token::KwAs
                            | Token::KwUndefined
                            | Token::KwConstructor
                            | Token::KwFrom
                            | Token::KwGet
                            | Token::KwOf
                            | Token::KwSet
                    ) =>
            {
                self.adv();
                self.semi();
                Some(Statement::Expr(Expr::Identifier("let".into())))
            }
            Token::KwLet => self.var_decl(VarKind::Let),
            Token::KwConst => {
                if self.single_statement {
                    self.record_error("const declaration requires a statement list".into());
                }
                self.var_decl(VarKind::Const)
            }
            Token::KwFunction => self.fn_decl(false),
            Token::KwAsync => {
                // `async function name(...) { ... }`
                if !self.line_break_after_current() && matches!(self.peek(), Token::KwFunction) {
                    self.adv(); // consume `async`
                    self.fn_decl(true)
                } else {
                    let e = self.expr()?;
                    self.semi();
                    Some(Statement::Expr(e))
                }
            }
            Token::KwClass => {
                if self.single_statement {
                    self.record_error("class declaration requires a statement list".into());
                }
                self.class_decl()
            }
            Token::KwReturn => self.ret(),
            Token::KwIf => self.if_(),
            Token::KwWhile => self.while_(),
            Token::KwDo => self.do_(),
            Token::KwFor => self.for_(),
            Token::KwBreak => {
                self.adv();
                let label = if !self.line_break_before_current()
                    && let Token::Identifier(n) = self.cur()
                {
                    let l = n.clone();
                    self.adv();
                    Some(l)
                } else {
                    None
                };
                self.semi();
                if let Some(l) = label {
                    Some(Statement::LabeledBreak(l))
                } else {
                    Some(Statement::Break)
                }
            }
            Token::KwContinue => {
                self.adv();
                let label = if !self.line_break_before_current()
                    && let Token::Identifier(n) = self.cur()
                {
                    let l = n.clone();
                    self.adv();
                    Some(l)
                } else {
                    None
                };
                self.semi();
                if let Some(l) = label {
                    Some(Statement::LabeledContinue(l))
                } else {
                    Some(Statement::Continue)
                }
            }
            Token::KwThrow => self.throw(),
            Token::KwTry => self.try_(),
            Token::KwSwitch => self.switch(),
            Token::KwExport => self.export(),
            Token::KwImport => {
                let saved_pos = self.pos;
                self.adv();
                if self.eat(&Token::Dot)
                    && let Token::Identifier(m) = self.cur()
                    && m == "meta"
                {
                    self.adv();
                    let mut expr = Expr::ImportMeta;
                    while self.eat(&Token::Dot) {
                        let prop = self.ident()?;
                        expr = Expr::Member {
                            object: Box::new(expr),
                            property: Box::new(Expr::String((prop).into())),
                            computed: false,
                        };
                    }
                    self.semi();
                    return Some(Statement::Expr(expr));
                }
                // `import('m')` at statement position is an expression.
                if matches!(self.cur(), Token::LParen | Token::Dot) {
                    self.pos = saved_pos;
                    let e = self.expr()?;
                    self.semi();
                    return Some(Statement::Expr(e));
                }
                self.pos = saved_pos;
                self.import()
            }
            Token::LBrace => {
                self.adv();
                let b = self.block_body();
                self.expect(&Token::RBrace);
                Some(Statement::Block(b))
            }
            Token::Semicolon => {
                self.adv();
                Some(Statement::Empty)
            }
            _ => {
                // Labeled statement: `label: statement`
                if let Token::Identifier(n) = self.cur()
                    && matches!(self.peek(), Token::Colon)
                {
                    let label = n.clone();
                    self.adv(); // identifier
                    self.adv(); // colon
                    let saved = self.single_statement;
                    self.single_statement = true;
                    let body = self.stmt();
                    self.single_statement = saved;
                    let body = body?;
                    return Some(Statement::Labeled {
                        label,
                        body: Box::new(body),
                    });
                }
                let literal_start =
                    matches!(self.cur(), Token::String(_) | Token::EscapedString(_));
                let mut e = self.expr()?;
                if !literal_start && matches!(e, Expr::String(_) | Expr::EscapedString(_)) {
                    // Parentheses are erased elsewhere in the AST. Preserve
                    // their non-directive status without changing completion.
                    e = Expr::Binary {
                        op: super::BinOp::Comma,
                        left: Box::new(Expr::Undefined),
                        right: Box::new(e),
                    };
                }
                self.semi();
                Some(Statement::Expr(e))
            }
        }
    }

    fn resource_declaration(&mut self, is_await: bool) -> Option<Statement> {
        self.adv();
        let mut declarations = Vec::new();
        loop {
            let name = self.ident()?;
            self.expect(&Token::Equal);
            let init = self.assign()?;
            declarations.push(Statement::VarDecl {
                kind: VarKind::Const,
                name,
                init: Some(Box::new(init)),
                destructuring: None,
            });
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.semi();
        Some(Statement::ResourceDeclaration {
            declarations,
            is_await,
        })
    }

    pub(crate) fn var_decl(&mut self, k: VarKind) -> Option<Statement> {
        self.adv();
        let mut decls = Vec::new();
        loop {
            let mut name = String::new();
            let mut destructuring = None;
            let mut init = None;

            if matches!(self.cur(), Token::LBracket) || matches!(self.cur(), Token::LBrace) {
                // Destructuring declaration: `const [a, b] = ...` / `const {a} = ...`
                destructuring = Some(Box::new(self.pattern()?));
                if self.eat(&Token::Equal) {
                    init = Some(Box::new(self.assign()?));
                }
            } else {
                let span = self.cur_span();
                name = self.ident()?;
                self.record(
                    &name,
                    span,
                    crate::parser::Occurrence::Declaration(crate::parser::DeclKind::Variable),
                    Some(
                        match k {
                            VarKind::Var => "var",
                            VarKind::Let => "let",
                            VarKind::Const => "const",
                        }
                        .to_string(),
                    ),
                );
                if self.eat(&Token::Equal) {
                    init = Some(Box::new(self.assign()?));
                }
            }

            decls.push(Statement::VarDecl {
                kind: k.clone(),
                name,
                init,
                destructuring,
            });
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.semi();
        if decls.len() == 1 {
            Some(decls.pop().unwrap())
        } else {
            // Not a `Block`: these declarators belong to the enclosing scope.
            Some(Statement::Declarations(decls))
        }
    }

    pub(crate) fn pattern(&mut self) -> Option<Pattern> {
        match self.cur() {
            Token::LBracket => {
                self.adv();
                let mut elements = Vec::new();
                while self.until(&Token::RBracket) {
                    if self.eat(&Token::Comma) {
                        elements.push(Pattern::Elision);
                        continue;
                    }
                    if self.eat(&Token::DotDotDot) {
                        let p = self.pattern()?;
                        elements.push(Pattern::Rest(Box::new(p)));
                        if !matches!(self.cur(), Token::RBracket) {
                            self.record_error(
                                "rest element must be last without a trailing comma".into(),
                            );
                        }
                    } else {
                        elements.push(self.pattern()?);
                    }
                    if !matches!(self.cur(), Token::RBracket) {
                        self.eat(&Token::Comma);
                    }
                }
                self.expect(&Token::RBracket);
                Some(Pattern::Array(elements))
            }
            Token::LBrace => {
                self.adv();
                let mut props = Vec::new();
                while self.until(&Token::RBrace) {
                    // `{ ...rest }` collects the remaining properties.
                    if self.eat(&Token::DotDotDot) {
                        let rest = self.pattern()?;
                        props.push((
                            PatternKey::Name("...".to_string()),
                            Some(Pattern::Rest(Box::new(rest))),
                        ));
                        if !matches!(self.cur(), Token::RBrace) {
                            self.eat(&Token::Comma);
                        }
                        continue;
                    }
                    // `{ [expr]: target }` evaluates the key; string and
                    // numeric keys behave like their object-literal forms.
                    let mut keyword_only = false;
                    let key = if self.eat(&Token::LBracket) {
                        let expr = self.assign()?;
                        self.expect(&Token::RBracket);
                        PatternKey::Computed(expr)
                    } else {
                        match self.cur() {
                            Token::String(s) | Token::EscapedString(s) => {
                                let key = PatternKey::Name(s.to_key());
                                self.adv();
                                key
                            }
                            Token::Number(n) => {
                                let key = PatternKey::Name(crate::format::number_string(*n));
                                self.adv();
                                key
                            }
                            _ => {
                                if let Some(name) = self.ident() {
                                    PatternKey::Name(name)
                                } else {
                                    let name = self.ident_or_keyword()?;
                                    // Reserved words work as keys only with
                                    // `: target`; they can never bind shorthand.
                                    keyword_only = true;
                                    PatternKey::Name(name)
                                }
                            }
                        }
                    };
                    let mut pat = None;
                    let mut saw_colon = false;
                    if self.eat(&Token::Colon) {
                        saw_colon = true;
                        pat = Some(self.pattern()?);
                    }
                    // `{ a = 1 }` and `{ a: b = 1 }` supply a default for a
                    // property that is absent or `undefined`. A computed key
                    // without a colon (`{ [k] }`, `{ [k] = 1 }`) is invalid.
                    if self.eat(&Token::Equal) {
                        let default = self.assign()?;
                        let target = match (pat, &key) {
                            (Some(target), _) => target,
                            (None, PatternKey::Name(name)) => Pattern::Ident(name.clone()),
                            (None, PatternKey::Computed(_)) => return None,
                        };
                        pat = Some(Pattern::Default(Box::new(target), Box::new(default)));
                    }
                    if (matches!(key, PatternKey::Computed(_)) || keyword_only) && !saw_colon {
                        return None;
                    }
                    props.push((key, pat));
                    if !matches!(self.cur(), Token::RBrace) {
                        self.eat(&Token::Comma);
                    }
                }
                self.expect(&Token::RBrace);
                Some(Pattern::Object(props))
            }
            _ => {
                let name = self.ident()?;
                if self.eat(&Token::Equal) {
                    Some(Pattern::Default(
                        Box::new(Pattern::Ident(name)),
                        Box::new(self.assign()?),
                    ))
                } else {
                    Some(Pattern::Ident(name))
                }
            }
        }
    }

    pub(crate) fn fn_decl(&mut self, is_async: bool) -> Option<Statement> {
        self.fn_decl_named(is_async, None)
    }

    /// Parse a function declaration whose name may be omitted (`export default
    /// function () { … }`), in which case `fallback` supplies the binding name.
    pub(crate) fn fn_decl_named(
        &mut self,
        is_async: bool,
        fallback: Option<&str>,
    ) -> Option<Statement> {
        self.adv(); // consume `function`
        // Generator declaration: `function*`.
        let is_generator = self.eat(&Token::Star);
        let name_span = self.cur_span();
        let n = if let Some(name) = self.ident() {
            name
        } else {
            fallback?.to_string()
        };
        // The function's own name belongs to the enclosing scope; its
        // parameters and body belong to a new one.
        let outer = self.push_scope(true);
        self.eat(&Token::LParen);
        let (p, defaults, b) = self.callable_parts(is_async, is_generator);
        self.pop_scope(outer);
        self.record(
            &n,
            name_span,
            crate::parser::Occurrence::Declaration(crate::parser::DeclKind::Function),
            Some(format!("({})", p.join(", "))),
        );
        self.check_parameters(&p, &defaults, &b, is_async || is_generator);
        let body = Self::function_body(&p, defaults, b);
        Some(Statement::FnDecl {
            name: n,
            params: p,
            body,
            is_async,
            is_generator,
        })
    }

    fn ret(&mut self) -> Option<Statement> {
        self.adv();
        let e = if self.line_break_before_current()
            || matches!(self.cur(), Token::Semicolon | Token::RBrace | Token::EOF)
        {
            None
        } else {
            Some(Box::new(self.expr()?))
        };
        self.semi();
        Some(Statement::Return(e))
    }

    fn if_(&mut self) -> Option<Statement> {
        self.adv();
        self.eat(&Token::LParen);
        let t = Box::new(self.expr()?);
        self.expect(&Token::RParen);
        let c = self.block_or_stmt();
        let a = if self.eat(&Token::KwElse) {
            if matches!(self.cur(), Token::KwIf) {
                Some(vec![self.if_()?])
            } else {
                Some(self.block_or_stmt())
            }
        } else {
            None
        };
        Some(Statement::If {
            test: t,
            then: c,
            else_: a,
        })
    }

    /// Parses either a `{ ... }` block or a single statement, returning the
    /// body as a statement list. Enables braceless `if`/`for`/`while` bodies.
    fn is_labelled_function(statement: &Statement) -> bool {
        match statement {
            Statement::Labeled { body, .. } => {
                matches!(body.as_ref(), Statement::FnDecl { .. })
                    || Self::is_labelled_function(body)
            }
            _ => false,
        }
    }

    fn block_or_stmt(&mut self) -> Vec<Statement> {
        if self.eat(&Token::LBrace) {
            let b = self.block_body();
            self.expect(&Token::RBrace);
            b
        } else {
            let saved = self.single_statement;
            self.single_statement = true;
            let statement = self.stmt();
            if statement.as_ref().is_some_and(Self::is_labelled_function) {
                self.record_error(
                    "labelled function is not permitted in this statement position".into(),
                );
            }
            self.single_statement = saved;
            statement.into_iter().collect()
        }
    }

    fn while_(&mut self) -> Option<Statement> {
        self.adv();
        self.eat(&Token::LParen);
        let t = Box::new(self.expr()?);
        self.expect(&Token::RParen);
        let b = self.block_or_stmt();
        Some(Statement::While { test: t, body: b })
    }

    fn do_(&mut self) -> Option<Statement> {
        self.adv();
        let b = self.block_or_stmt();
        if !self.eat(&Token::KwWhile) {
            return None;
        }
        self.eat(&Token::LParen);
        let t = Box::new(self.expr()?);
        self.expect(&Token::RParen);
        self.eat(&Token::Semicolon);
        Some(Statement::DoWhile { test: t, body: b })
    }

    fn for_(&mut self) -> Option<Statement> {
        self.adv();
        // `for await (… of …)`.
        let is_await = self.eat(&Token::KwAwait);
        self.eat(&Token::LParen);
        let await_using = matches!(self.cur(), Token::KwAwait)
            && matches!(self.peek(), Token::Identifier(name) if name == "using");
        if await_using {
            self.adv();
        }
        if matches!(self.cur(), Token::Identifier(name) if name == "using")
            && matches!(self.peek(), Token::Identifier(_))
        {
            self.adv();
            let name = self.ident()?;
            self.expect(&Token::KwOf);
            let iter = Box::new(self.expr()?);
            self.expect(&Token::RParen);
            let body = self.block_or_stmt();
            return Some(Statement::ResourceForOf {
                name,
                iter,
                body,
                is_await,
                await_disposal: await_using,
            });
        }
        // `for (const [k, v] of pairs)` / `for (const { id } of rows)`: the
        // head binds a pattern, which the loop destructures per iteration.
        let mut head_pattern: Option<Box<Pattern>> = None;
        let init = if matches!(self.cur(), Token::KwVar | Token::KwLet | Token::KwConst) {
            let kind = match self.cur() {
                Token::KwVar => VarKind::Var,
                Token::KwLet => VarKind::Let,
                _ => VarKind::Const,
            };
            self.adv();
            if matches!(self.cur(), Token::LBracket | Token::LBrace) {
                let pattern = self.pattern()?;
                if kind != VarKind::Var
                    && super::pattern_names(&pattern)
                        .iter()
                        .any(|name| name == "let")
                {
                    self.record_error("let is not a lexical binding name".into());
                }
                // `for (… in …)` / `for (… of …)` heads carry no initializer;
                // a `=` here means a C-style head with optional trailing
                // `, name = init` declarators.
                if self.eat(&Token::Equal) {
                    let init_expr = self.assign()?;
                    let mut trailing = Vec::new();
                    while self.eat(&Token::Comma) {
                        let n = self.ident()?;
                        let i = if self.eat(&Token::Equal) {
                            Some(self.assign()?)
                        } else {
                            None
                        };
                        trailing.push((n, i));
                    }
                    Some(Box::new(ForInit::Pattern {
                        kind,
                        pattern,
                        init: init_expr,
                        trailing,
                    }))
                } else {
                    head_pattern = Some(Box::new(pattern));
                    let decls = vec![("*pattern*".to_string(), None)];
                    Some(Box::new(ForInit::Var { kind, decls }))
                }
            } else {
                let mut decls = Vec::new();
                loop {
                    let n = self.ident()?;
                    if kind != VarKind::Var && n == "let" {
                        self.record_error("let is not a lexical binding name".into());
                    }
                    let i = if self.eat(&Token::Equal) {
                        Some(self.assign()?)
                    } else {
                        None
                    };
                    decls.push((n, i));
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
                Some(Box::new(ForInit::Var { kind, decls }))
            }
        } else if matches!(self.cur(), Token::Semicolon) {
            None
        } else {
            Some(Box::new(ForInit::Expr(self.with_in(false, Self::expr)?)))
        };
        if let Some(init) = init.as_ref()
            && !matches!(self.cur(), Token::Semicolon)
        {
            if self.eat(&Token::KwIn) {
                if is_await {
                    self.record_error("for await requires of".into());
                }
                if let ForInit::Var { decls, .. } = init.as_ref()
                    && decls.len() != 1
                {
                    self.record_error("multiple declarations in for-in".into());
                }
                let o = Box::new(self.expr()?);
                self.expect(&Token::RParen);
                let b = self.block_or_stmt();
                let (n, target) = match init.as_ref() {
                    ForInit::Var { decls, .. } => (decls.first()?.0.clone(), None),
                    ForInit::Expr(target) => (String::new(), Some(Box::new(target.clone()))),
                    _ => {
                        self.record_error("invalid for-in declaration head".into());
                        return None;
                    }
                };
                return Some(Statement::ForIn {
                    name: n,
                    target,
                    obj: o,
                    body: b,
                });
            }
            if self.eat(&Token::KwOf) {
                if let ForInit::Var { decls, .. } = init.as_ref()
                    && (decls.len() != 1 || decls[0].1.is_some())
                {
                    self.record_error("invalid for-of declaration".into());
                }
                let i = Box::new(self.expr()?);
                self.expect(&Token::RParen);
                let b = self.block_or_stmt();
                let n = match init.as_ref() {
                    ForInit::Var { decls, .. } => decls.first()?.0.clone(),
                    _ => return None,
                };
                return Some(Statement::ForOf {
                    name: n,
                    pattern: head_pattern,
                    iter: i,
                    body: b,
                    is_await,
                });
            }
        }
        if is_await {
            self.record_error("for await requires of".into());
        }
        self.expect(&Token::Semicolon);
        let t = if !matches!(self.cur(), Token::Semicolon) {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect(&Token::Semicolon);
        let u = if !matches!(self.cur(), Token::RParen) {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect(&Token::RParen);
        let b = self.block_or_stmt();
        Some(Statement::For {
            init,
            test: t,
            update: u,
            body: b,
        })
    }

    fn throw(&mut self) -> Option<Statement> {
        self.adv();
        if self.line_break_before_current() {
            self.record_error("line terminator after throw".into());
        }
        let e = self.expr()?;
        self.semi();
        Some(Statement::Throw(Box::new(e)))
    }

    fn try_(&mut self) -> Option<Statement> {
        self.adv();
        self.eat(&Token::LBrace);
        let b = self.block_body();
        self.expect(&Token::RBrace);
        let c = if self.eat(&Token::KwCatch) {
            let p = if self.eat(&Token::LParen) {
                let x = self.ident()?;
                self.expect(&Token::RParen);
                x
            } else {
                String::new()
            };
            self.eat(&Token::LBrace);
            let cb = self.block_body();
            self.expect(&Token::RBrace);
            Some((p, cb))
        } else {
            None
        };
        let f = if self.eat(&Token::KwFinally) {
            self.eat(&Token::LBrace);
            let fb = self.block_body();
            self.expect(&Token::RBrace);
            Some(fb)
        } else {
            None
        };
        Some(Statement::Try {
            body: b,
            catch: c,
            finally: f,
        })
    }

    fn switch(&mut self) -> Option<Statement> {
        self.adv();
        self.eat(&Token::LParen);
        let d = Box::new(self.expr()?);
        self.expect(&Token::RParen);
        self.eat(&Token::LBrace);
        let mut cs = Vec::new();
        while self.until(&Token::RBrace) {
            if self.eof() {
                break;
            }
            let t = if self.eat(&Token::KwCase) {
                let e = self.expr()?;
                self.eat(&Token::Colon);
                Some(e)
            } else if self.eat(&Token::KwDefault) {
                self.eat(&Token::Colon);
                None
            } else {
                break;
            };
            let mut b = Vec::new();
            while !matches!(self.cur(), Token::KwCase)
                && !matches!(self.cur(), Token::KwDefault)
                && !matches!(self.cur(), Token::RBrace)
            {
                if self.eof() {
                    break;
                }
                b.push(self.stmt()?);
            }
            cs.push(SwitchCase { test: t, body: b });
        }
        self.expect(&Token::RBrace);
        Some(Statement::Switch { disc: d, cases: cs })
    }

    /// Parse a parameter list. Returns the parameter names (rest params keep
    /// their `...` prefix) plus guard statements that implement default values
    /// (`if (name === undefined) name = <default>;`), to be prepended to a body.
    /// Parse a parameter list, recording each name as a declaration in the
    /// scope the caller has already opened for the body.
    pub(crate) fn params(&mut self) -> (Vec<String>, Vec<Statement>) {
        let mut names = Vec::new();
        let mut defaults = Vec::new();
        while self.until(&Token::RParen) {
            if self.eat(&Token::DotDotDot) {
                let span = self.cur_span();
                if matches!(self.cur(), Token::LBracket | Token::LBrace) {
                    let Some(pattern) = self.pattern() else {
                        break;
                    };
                    let slot = format!("*pattern{}*", names.len());
                    defaults.push(Statement::VarDecl {
                        kind: VarKind::Let,
                        name: String::new(),
                        init: Some(Box::new(Expr::Identifier(slot.clone()))),
                        destructuring: Some(Box::new(pattern)),
                    });
                    names.push(format!("...{slot}"));
                    if !matches!(self.cur(), Token::RParen) {
                        self.record_error(
                            "rest parameter must be last without a trailing comma".into(),
                        );
                    }
                } else if let Some(name) = self.ident() {
                    self.record(
                        &name,
                        span,
                        crate::parser::Occurrence::Declaration(crate::parser::DeclKind::Parameter),
                        None,
                    );
                    names.push(format!("...{}", name));
                    if !matches!(self.cur(), Token::RParen) {
                        self.record_error(
                            "rest parameter must be last without a trailing comma".into(),
                        );
                    }
                } else {
                    self.record_error("expected rest parameter binding".into());
                }
            } else if matches!(self.cur(), Token::LBracket | Token::LBrace) {
                // A destructured parameter — `function f({ a, b })`. The
                // slot takes a synthetic name and the body opens with a
                // declaration that unpacks it, which reuses the declaration
                // path rather than duplicating the binding logic.
                let Some(pattern) = self.pattern() else {
                    break;
                };
                let slot = format!("*pattern{}*", names.len());
                let mut init: Expr = Expr::Identifier(slot.clone());
                if self.eat(&Token::Equal)
                    && let Some(default) = self.assign()
                {
                    // `function f({ a } = {})`: the default applies to the
                    // whole parameter before it is unpacked.
                    defaults.push(Self::default_guard(&slot, default));
                    init = Expr::Identifier(slot.clone());
                }
                defaults.push(Statement::VarDecl {
                    kind: VarKind::Let,
                    name: String::new(),
                    init: Some(Box::new(init)),
                    destructuring: Some(Box::new(pattern)),
                });
                names.push(slot);
            } else {
                let span = self.cur_span();
                if let Some(name) = self.ident() {
                    self.record(
                        &name,
                        span,
                        crate::parser::Occurrence::Declaration(crate::parser::DeclKind::Parameter),
                        None,
                    );
                    if self.eat(&Token::Equal)
                        && let Some(d) = self.assign()
                    {
                        defaults.push(Self::default_guard(&name, d));
                    }
                    names.push(name);
                } else {
                    self.record_error("expected parameter binding".into());
                    self.adv();
                }
            }
            if !matches!(self.cur(), Token::RParen) {
                self.expect(&Token::Comma);
            }
        }
        (names, defaults)
    }

    pub(crate) fn ident(&mut self) -> Option<String> {
        match self.cur() {
            Token::Identifier(n) => {
                let v = n.clone();
                self.adv();
                Some(v)
            }
            // These words are contextual in ECMAScript, not reserved binding
            // names. The lexer keeps dedicated tokens for their grammar roles
            // (async functions, accessors and module syntax), while binding
            // positions may still use them as identifiers.
            Token::KwAs => self.consume_contextual_identifier("as"),
            Token::KwLet => self.consume_contextual_identifier("let"),
            Token::KwUndefined => self.consume_contextual_identifier("undefined"),
            Token::KwAwait if !self.await_expression => self.consume_contextual_identifier("await"),
            Token::KwYield if !self.yield_expression => self.consume_contextual_identifier("yield"),
            Token::KwAsync => self.consume_contextual_identifier("async"),
            Token::KwConstructor => self.consume_contextual_identifier("constructor"),
            Token::KwFrom => self.consume_contextual_identifier("from"),
            Token::KwGet => self.consume_contextual_identifier("get"),
            Token::KwOf => self.consume_contextual_identifier("of"),
            Token::KwSet => self.consume_contextual_identifier("set"),
            _ => None,
        }
    }

    fn consume_contextual_identifier(&mut self, name: &str) -> Option<String> {
        self.adv();
        Some(name.to_string())
    }

    /// Like `ident()`, but also accepts keywords as property names (valid after
    /// `.` in member expressions: `obj.for`, `obj.of`, `obj.get`, etc.).
    pub(crate) fn ident_or_keyword(&mut self) -> Option<String> {
        match self.cur() {
            Token::Identifier(n) => {
                let v = n.clone();
                self.adv();
                Some(v)
            }
            // Keywords that can appear as property names after `.`.
            Token::KwFor => {
                self.adv();
                Some("for".to_string())
            }
            Token::KwOf => {
                self.adv();
                Some("of".to_string())
            }
            Token::KwIn => {
                self.adv();
                Some("in".to_string())
            }
            Token::KwIf => {
                self.adv();
                Some("if".to_string())
            }
            Token::KwDo => {
                self.adv();
                Some("do".to_string())
            }
            Token::KwAs => {
                self.adv();
                Some("as".to_string())
            }
            Token::KwLet => {
                self.adv();
                Some("let".to_string())
            }
            Token::KwNew => {
                self.adv();
                Some("new".to_string())
            }
            Token::KwVar => {
                self.adv();
                Some("var".to_string())
            }
            Token::KwGet => {
                self.adv();
                Some("get".to_string())
            }
            Token::KwSet => {
                self.adv();
                Some("set".to_string())
            }
            Token::KwTry => {
                self.adv();
                Some("try".to_string())
            }
            Token::KwCase => {
                self.adv();
                Some("case".to_string())
            }
            Token::KwElse => {
                self.adv();
                Some("else".to_string())
            }
            Token::KwWith => {
                self.adv();
                Some("with".to_string())
            }
            Token::KwFrom => {
                self.adv();
                Some("from".to_string())
            }
            Token::KwVoid => {
                self.adv();
                Some("void".to_string())
            }
            Token::KwThis => {
                self.adv();
                Some("this".to_string())
            }
            Token::KwTrue => {
                self.adv();
                Some("true".to_string())
            }
            Token::KwNull => {
                self.adv();
                Some("null".to_string())
            }
            Token::KwAsync => {
                self.adv();
                Some("async".to_string())
            }
            Token::KwAwait => {
                self.adv();
                Some("await".to_string())
            }
            Token::KwBreak => {
                self.adv();
                Some("break".to_string())
            }
            Token::KwCatch => {
                self.adv();
                Some("catch".to_string())
            }
            Token::KwClass => {
                self.adv();
                Some("class".to_string())
            }
            Token::KwConst => {
                self.adv();
                Some("const".to_string())
            }
            Token::KwSuper => {
                self.adv();
                Some("super".to_string())
            }
            Token::KwThrow => {
                self.adv();
                Some("throw".to_string())
            }
            Token::KwWhile => {
                self.adv();
                Some("while".to_string())
            }
            Token::KwYield => {
                self.adv();
                Some("yield".to_string())
            }
            Token::KwFalse => {
                self.adv();
                Some("false".to_string())
            }
            Token::KwDelete => {
                self.adv();
                Some("delete".to_string())
            }
            Token::KwExport => {
                self.adv();
                Some("export".to_string())
            }
            Token::KwImport => {
                self.adv();
                Some("import".to_string())
            }
            Token::KwReturn => {
                self.adv();
                Some("return".to_string())
            }
            Token::KwStatic => {
                self.adv();
                Some("static".to_string())
            }
            Token::KwSwitch => {
                self.adv();
                Some("switch".to_string())
            }
            Token::KwTypeof => {
                self.adv();
                Some("typeof".to_string())
            }
            Token::KwDefault => {
                self.adv();
                Some("default".to_string())
            }
            Token::KwExtends => {
                self.adv();
                Some("extends".to_string())
            }
            Token::KwFinally => {
                self.adv();
                Some("finally".to_string())
            }
            Token::KwContinue => {
                self.adv();
                Some("continue".to_string())
            }
            Token::KwFunction => {
                self.adv();
                Some("function".to_string())
            }
            Token::KwInstanceof => {
                self.adv();
                Some("instanceof".to_string())
            }
            Token::KwUndefined => {
                self.adv();
                Some("undefined".to_string())
            }
            Token::KwConstructor => {
                self.adv();
                Some("constructor".to_string())
            }
            _ => None,
        }
    }
}
