//! Core statement parsing. Classes and `import` / `export` live in
//! `compound.rs`.

use super::{
    Expr, ForBinding, ForInit, Parser, Pattern, PatternKey, Statement, SwitchCase, VarKind,
};
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
        // Labeled statement: `label: statement`
        if matches!(self.peek(), Token::Colon)
            && let Some(label) = self.label_name()
        {
            self.adv(); // identifier
            self.adv(); // colon
            let saved = self.single_statement;
            let saved_annex_b = self.allow_annex_b_function;
            self.single_statement = true;
            self.allow_annex_b_function = true;
            let body = self.stmt();
            self.single_statement = saved;
            self.allow_annex_b_function = saved_annex_b;
            let body = body?;
            return Some(Statement::Labeled {
                label,
                body: Box::new(body),
            });
        }
        match self.cur() {
            Token::Identifier(name)
                if name == "using"
                    && !self.line_break_after_current()
                    && matches!(
                        self.peek(),
                        Token::Identifier(_) | Token::EscapedIdentifier(_)
                    ) =>
            {
                if self.single_statement {
                    self.record_error("resource declaration requires a statement list".into());
                }
                self.resource_declaration(false)
            }
            Token::KwAwait
                if matches!(self.peek(), Token::Identifier(name) if name == "using")
                    && !self.line_break_after_current()
                    && self.toks.get(self.pos + 2).is_some_and(|(token, _)| {
                        matches!(token, Token::Identifier(_) | Token::EscapedIdentifier(_))
                    }) =>
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
                let body = self.block_or_stmt(false);
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
                            | Token::EscapedIdentifier(_)
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
                            | Token::KwStatic
                    ) =>
            {
                // ExpressionStatement excludes a leading `let [` even when
                // the surrounding position cannot contain a declaration.
                if matches!(self.peek(), Token::LBracket) {
                    self.record_error("let [ cannot begin an expression statement".into());
                }
                let expression = self.expr()?;
                self.semi();
                Some(Statement::Expr(expression))
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
                    && let Some(l) = self.label_name()
                {
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
                    && let Some(l) = self.label_name()
                {
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
            Token::KwDebugger => {
                self.adv();
                self.semi();
                Some(Statement::Empty)
            }
            Token::Semicolon => {
                self.adv();
                Some(Statement::Empty)
            }
            _ => {
                let literal_start = matches!(
                    self.cur(),
                    Token::String(_) | Token::EscapedString(_) | Token::LegacyString(_)
                );
                let mut e = self.expr()?;
                if !literal_start && e.is_string_literal() {
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
                        self.expect(&Token::Comma);
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
                            self.record_error(
                                "object binding rest must be last without a trailing comma".into(),
                            );
                            self.expect(&Token::Comma);
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
                            Token::LegacyString(s) => {
                                let expr =
                                    Expr::LegacyLiteral(Box::new(Expr::EscapedString(s.clone())));
                                self.adv();
                                PatternKey::Computed(expr)
                            }
                            Token::LegacyNumber(n) => {
                                let expr = Expr::LegacyLiteral(Box::new(Expr::Number(*n)));
                                self.adv();
                                PatternKey::Computed(expr)
                            }
                            Token::String(s) | Token::EscapedString(s) => {
                                keyword_only = true;
                                let key = PatternKey::Name(s.to_key());
                                self.adv();
                                key
                            }
                            Token::Number(n) => {
                                keyword_only = true;
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
                        self.expect(&Token::Comma);
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
        let annex_b_statement = self.single_statement;
        if annex_b_statement && (!self.allow_annex_b_function || is_async || is_generator) {
            self.record_error(
                "function declaration is not permitted in this statement position".into(),
            );
        }
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
            annex_b_statement,
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
        let c = self.block_or_stmt(true);
        let a = if self.eat(&Token::KwElse) {
            if matches!(self.cur(), Token::KwIf) {
                Some(vec![self.if_()?])
            } else {
                Some(self.block_or_stmt(true))
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

    fn block_or_stmt(&mut self, allow_annex_b_function: bool) -> Vec<Statement> {
        if self.eat(&Token::LBrace) {
            let b = self.block_body();
            self.expect(&Token::RBrace);
            b
        } else {
            let saved = self.single_statement;
            let saved_annex_b = self.allow_annex_b_function;
            self.single_statement = true;
            self.allow_annex_b_function = allow_annex_b_function;
            let statement = self.stmt();
            if statement.is_none() {
                self.record_error("expected a statement body".into());
            }
            if statement.as_ref().is_some_and(Self::is_labelled_function) {
                self.record_error(
                    "labelled function is not permitted in this statement position".into(),
                );
            }
            self.single_statement = saved;
            self.allow_annex_b_function = saved_annex_b;
            statement.into_iter().collect()
        }
    }

    fn while_(&mut self) -> Option<Statement> {
        self.adv();
        self.eat(&Token::LParen);
        let t = Box::new(self.expr()?);
        self.expect(&Token::RParen);
        let b = self.block_or_stmt(false);
        Some(Statement::While { test: t, body: b })
    }

    fn do_(&mut self) -> Option<Statement> {
        self.adv();
        let b = self.block_or_stmt(false);
        if !self.eat(&Token::KwWhile) {
            return None;
        }
        self.eat(&Token::LParen);
        let t = Box::new(self.expr()?);
        self.expect(&Token::RParen);
        self.eat(&Token::Semicolon);
        Some(Statement::DoWhile { test: t, body: b })
    }

    fn iteration_binding(
        &mut self,
        init: &ForInit,
        pattern: Option<Box<Pattern>>,
    ) -> Option<ForBinding> {
        Some(match init {
            ForInit::Var { kind, decls } => {
                let (name, initializer) = decls.first()?;
                ForBinding::Declaration {
                    kind: kind.clone(),
                    pattern: pattern
                        .map(|pattern| *pattern)
                        .unwrap_or_else(|| Pattern::Ident(name.clone())),
                    initializer: initializer.clone().map(Box::new),
                }
            }
            ForInit::Pattern {
                kind,
                pattern,
                init,
                trailing,
            } => {
                if !trailing.is_empty() {
                    self.record_error("multiple iteration declarations".into());
                }
                ForBinding::Declaration {
                    kind: kind.clone(),
                    pattern: pattern.clone(),
                    initializer: Some(Box::new(init.clone())),
                }
            }
            ForInit::Expr(target) => ForBinding::Assignment(Box::new(target.clone())),
        })
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
            && matches!(
                self.peek(),
                Token::Identifier(_) | Token::EscapedIdentifier(_)
            )
        {
            self.adv();
            let name = self.ident()?;
            self.expect(&Token::KwOf);
            let iter = Box::new(self.assign()?);
            self.expect(&Token::RParen);
            let body = self.block_or_stmt(false);
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
        let bare_async_head =
            matches!(self.cur(), Token::KwAsync) && matches!(self.peek(), Token::KwOf);
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
                    let init_expr = self.with_in(false, Self::assign)?;
                    let mut trailing = Vec::new();
                    while self.eat(&Token::Comma) {
                        let n = self.ident()?;
                        let i = if self.eat(&Token::Equal) {
                            Some(self.with_in(false, Self::assign)?)
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
                        Some(self.with_in(false, Self::assign)?)
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
                let b = self.block_or_stmt(false);
                let binding = self.iteration_binding(init, head_pattern)?;
                return Some(Statement::ForIn {
                    binding,
                    obj: o,
                    body: b,
                });
            }
            if self.eat(&Token::KwOf) {
                if bare_async_head && !is_await {
                    self.record_error("bare async is not a for-of assignment head".into());
                }
                if let ForInit::Var { decls, .. } = init.as_ref()
                    && (decls.len() != 1 || decls[0].1.is_some())
                {
                    self.record_error("invalid for-of declaration".into());
                }
                let i = Box::new(self.assign()?);
                self.expect(&Token::RParen);
                let b = self.block_or_stmt(false);
                let binding = self.iteration_binding(init, head_pattern)?;
                return Some(Statement::ForOf {
                    binding,
                    iter: i,
                    body: b,
                    is_await,
                });
            }
        }
        if is_await {
            self.record_error("for await requires of".into());
        }
        if head_pattern.is_some() {
            self.record_error("destructuring declaration requires an initializer".into());
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
        let b = self.block_or_stmt(false);
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
                let binding = if matches!(self.cur(), Token::LBracket | Token::LBrace) {
                    self.pattern()?
                } else {
                    Pattern::Ident(self.ident()?)
                };
                self.expect(&Token::RParen);
                Some(binding)
            } else {
                None
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
        let cs = self.with_statement_list(|parser| {
            let mut cs = Vec::new();
            while parser.until(&Token::RBrace) {
                if parser.eof() {
                    break;
                }
                let t = if parser.eat(&Token::KwCase) {
                    let e = parser.expr()?;
                    parser.eat(&Token::Colon);
                    Some(e)
                } else if parser.eat(&Token::KwDefault) {
                    parser.eat(&Token::Colon);
                    None
                } else {
                    break;
                };
                let mut b = Vec::new();
                while !matches!(parser.cur(), Token::KwCase)
                    && !matches!(parser.cur(), Token::KwDefault)
                    && !matches!(parser.cur(), Token::RBrace)
                {
                    if parser.eof() {
                        break;
                    }
                    b.push(parser.stmt()?);
                }
                cs.push(SwitchCase { test: t, body: b });
            }
            Some(cs)
        })?;
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
                if self.eat(&Token::Equal) {
                    let Some(default) = self.assign() else {
                        self.record_error("expected parameter default expression".into());
                        break;
                    };
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
                    if self.eat(&Token::Equal) {
                        let Some(d) = self.assign() else {
                            self.record_error("expected parameter default expression".into());
                            break;
                        };
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

    fn label_name(&self) -> Option<String> {
        Some(match self.cur() {
            Token::Identifier(name) | Token::EscapedIdentifier(name) => name.clone(),
            Token::KwAs => "as".into(),
            Token::KwAsync => "async".into(),
            Token::KwAwait => "await".into(),
            Token::KwConstructor => "constructor".into(),
            Token::KwFrom => "from".into(),
            Token::KwGet => "get".into(),
            Token::KwLet => "let".into(),
            Token::KwOf => "of".into(),
            Token::KwSet => "set".into(),
            Token::KwStatic => "static".into(),
            Token::KwUndefined => "undefined".into(),
            Token::KwYield => "yield".into(),
            _ => return None,
        })
    }

    pub(crate) fn ident(&mut self) -> Option<String> {
        match self.cur() {
            Token::Identifier(n) | Token::EscapedIdentifier(n) => {
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
            Token::KwStatic => self.consume_contextual_identifier("static"),
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
    pub(crate) fn member_property_name(&mut self) -> Option<String> {
        if let Token::PrivateIdentifier(name) = self.cur() {
            let name = format!("#{name}");
            self.adv();
            Some(name)
        } else {
            self.ident_or_keyword()
        }
    }

    pub(crate) fn ident_or_keyword(&mut self) -> Option<String> {
        let name = self.cur().identifier_name()?.to_owned();
        self.adv();
        Some(name)
    }
}
