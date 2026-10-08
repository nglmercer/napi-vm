//! Primary expressions: literals, identifiers, arrays, objects, function
//! expressions, `new`, template literals, and arrow-function parsing.

use super::{Expr, ExprOrBlock, ObjectProp, Parser, Statement};
use crate::lexer::Token;

/// Placeholder name for an anonymous `class` expression. Any name a class
/// expression does have binds only inside its own body, so the placeholder is
/// never observable.
const ANONYMOUS_CLASS: &str = "*anonymous class*";

impl Parser {
    pub(super) fn primary(&mut self) -> Option<Expr> {
        match self.cur() {
            Token::LegacyNumber(n) => {
                let value = *n;
                self.adv();
                Some(Expr::LegacyLiteral(Box::new(Expr::Number(value))))
            }
            Token::LegacyString(text) => {
                let value = text.clone();
                self.adv();
                Some(Expr::LegacyLiteral(Box::new(Expr::EscapedString(value))))
            }
            Token::Number(n) => {
                let v = *n;
                self.adv();
                Some(Expr::Number(v))
            }
            Token::String(s) | Token::EscapedString(s) => {
                let escaped = matches!(self.cur(), Token::EscapedString(_));
                let value = s.clone();
                self.adv();
                Some(if escaped {
                    Expr::EscapedString(value)
                } else {
                    Expr::String(value)
                })
            }
            Token::KwTrue => {
                self.adv();
                Some(Expr::Bool(true))
            }
            Token::KwFalse => {
                self.adv();
                Some(Expr::Bool(false))
            }
            Token::KwNull => {
                self.adv();
                Some(Expr::Null)
            }
            Token::KwUndefined => {
                self.adv();
                Some(Expr::Undefined)
            }
            Token::KwThis => {
                self.adv();
                Some(Expr::This)
            }
            Token::KwSuper => {
                self.adv();
                Some(Expr::Super)
            }
            Token::Backtick => {
                self.adv();
                let (quasis, exprs) = self.template_body()?;
                Some(Expr::Template {
                    quasis: quasis.into_iter().map(|q| q.cooked).collect(),
                    exprs,
                })
            }
            Token::LParen => {
                // Speculatively try to parse an arrow-function parameter list.
                if let Some(arrow) = self.try_arrow(false) {
                    return Some(arrow);
                }
                // Otherwise it is a parenthesized expression.
                self.adv();
                let e = self.with_in(true, Self::expr)?;
                self.expect(&Token::RParen);
                Some(e)
            }
            Token::LBracket => {
                self.adv();
                let mut i = Vec::new();
                while self.until(&Token::RBracket) {
                    if self.eat(&Token::Comma) {
                        i.push(Expr::Undefined);
                        continue;
                    }
                    if self.eat(&Token::DotDotDot) {
                        i.push(Expr::Spread(Box::new(self.with_in(true, Self::assign)?)));
                    } else {
                        i.push(self.with_in(true, Self::assign)?);
                    }
                    if !matches!(self.cur(), Token::RBracket) {
                        self.eat(&Token::Comma);
                    }
                }
                let trailing_comma = self
                    .toks
                    .get(self.pos.saturating_sub(1))
                    .is_some_and(|(token, _)| matches!(token, Token::Comma));
                self.expect(&Token::RBracket);
                Some(Expr::Array {
                    items: i,
                    trailing_comma,
                })
            }
            Token::LBrace => {
                self.adv();
                let mut legacy_key = false;
                let mut p = Vec::new();
                while self.until(&Token::RBrace) {
                    if self.eat(&Token::DotDotDot) {
                        let s = self.with_in(true, Self::assign)?;
                        p.push(ObjectProp::Spread(s));
                        if !matches!(self.cur(), Token::RBrace) {
                            self.eat(&Token::Comma);
                        }
                        continue;
                    }
                    // `async` and `*` modify the method that follows, and are
                    // contextual for the same reason `get` is: `{ async: 1 }`
                    // names a property.
                    let is_async = matches!(self.cur(), Token::KwAsync)
                        && !self.line_break_after_current()
                        && !matches!(
                            self.peek(),
                            Token::Colon | Token::LParen | Token::Comma | Token::RBrace
                        )
                        && self.eat(&Token::KwAsync);
                    let is_generator = self.eat(&Token::Star);
                    // `get`/`set` introduce an accessor only when a property
                    // name follows. In `{ get: 1 }` and `{ get() {} }` they are
                    // the property name themselves.
                    let is_method = self.starts_accessor(&Token::KwGet) && self.eat(&Token::KwGet);
                    let is_setter = self.starts_accessor(&Token::KwSet) && self.eat(&Token::KwSet);
                    if (is_method || is_setter) && (is_async || is_generator) {
                        self.record_error("accessor cannot be async or a generator".into());
                    }
                    legacy_key |=
                        matches!(self.cur(), Token::LegacyNumber(_) | Token::LegacyString(_));
                    let key = match self.cur() {
                        Token::String(s) | Token::EscapedString(s) | Token::LegacyString(s) => {
                            let v = s.to_key();
                            self.adv();
                            v
                        }
                        Token::Number(n) | Token::LegacyNumber(n) => {
                            let v = n.to_string();
                            self.adv();
                            v
                        }
                        Token::LBracket => {
                            self.adv();
                            let e = self.with_in(true, Self::assign)?;
                            self.expect(&Token::RBracket);
                            match self.cur() {
                                Token::Colon => {
                                    self.adv();
                                    let v = self.with_in(true, Self::assign)?;
                                    p.push(ObjectProp::Computed(e, v));
                                    if !matches!(self.cur(), Token::RBrace) {
                                        self.eat(&Token::Comma);
                                    }
                                    continue;
                                }
                                Token::LParen => {
                                    self.adv();
                                    let (params, defaults, b) =
                                        self.callable_parts(is_async, is_generator);
                                    self.check_accessor_parameters(
                                        &params, &defaults, is_method, is_setter,
                                    );
                                    self.check_parameters(&params, &defaults, &b, true);
                                    let body = Self::function_body(&params, defaults, b);
                                    // If the computed key is a simple literal,
                                    // use the named Method/Getter/Setter forms.
                                    // Otherwise, retain computed method/accessor metadata.
                                    // Only a *literal* computed key has a
                                    // name known at parse time. An identifier
                                    // is a variable to evaluate — `{ [k]() {} }`
                                    // names the property `k` holds, not "k".
                                    let key_str = match &e {
                                        Expr::String(s) | Expr::EscapedString(s) => {
                                            Some(s.to_key())
                                        }
                                        Expr::Number(n) => Some(n.to_string()),
                                        _ => None,
                                    };
                                    if let Some(key_str) = key_str {
                                        if is_method {
                                            p.push(ObjectProp::Getter {
                                                name: key_str,
                                                body,
                                            });
                                        } else if is_setter {
                                            let param = params.first().cloned().unwrap_or_default();
                                            p.push(ObjectProp::Setter {
                                                name: key_str,
                                                param,
                                                body,
                                            });
                                        } else {
                                            p.push(ObjectProp::Method {
                                                name: key_str,
                                                params,
                                                body,
                                                is_async,
                                                is_generator,
                                            });
                                        }
                                    } else {
                                        // Computed method with a non-literal key
                                        // (e.g. [Symbol.iterator]() { ... }).
                                        p.push(ObjectProp::ComputedMethod {
                                            key: e,
                                            params,
                                            body,
                                            is_async,
                                            is_generator,
                                            is_getter: is_method,
                                            is_setter,
                                        });
                                    }
                                    if !matches!(self.cur(), Token::RBrace) {
                                        self.eat(&Token::Comma);
                                    }
                                    continue;
                                }
                                _ => return None,
                            }
                        }
                        // Object literal keys use IdentifierName, so reserved
                        // words such as `default` and `class` are valid here.
                        _ => match self.ident_or_keyword() {
                            Some(key) => key,
                            None => break,
                        },
                    };
                    if is_setter {
                        self.eat(&Token::LParen);
                        let (params, defaults, b) = self.callable_parts(false, false);
                        self.check_parameters(&params, &defaults, &b, true);
                        self.check_accessor_parameters(&params, &defaults, false, true);
                        let param = params.first().cloned().unwrap_or_default();
                        p.push(ObjectProp::Setter {
                            name: key,
                            param,
                            body: Self::function_body(&params, defaults, b),
                        });
                    } else if self.eat(&Token::LParen) {
                        let (params, defaults, b) = self.callable_parts(is_async, is_generator);
                        self.check_accessor_parameters(&params, &defaults, is_method, false);
                        self.check_parameters(&params, &defaults, &b, true);
                        let body = Self::function_body(&params, defaults, b);
                        if is_method {
                            p.push(ObjectProp::Getter { name: key, body });
                        } else {
                            p.push(ObjectProp::Method {
                                name: key,
                                params,
                                body,
                                is_async,
                                is_generator,
                            });
                        }
                    } else if self.eat(&Token::Colon) {
                        let v = self.with_in(true, Self::assign)?;
                        p.push(ObjectProp::KeyValue(key, v));
                    } else {
                        p.push(ObjectProp::Shorthand(key));
                    }
                    if !matches!(self.cur(), Token::RBrace) {
                        self.eat(&Token::Comma);
                    }
                }
                let trailing_comma = self
                    .toks
                    .get(self.pos.saturating_sub(1))
                    .is_some_and(|(token, _)| matches!(token, Token::Comma));
                self.expect(&Token::RBrace);
                let object = Expr::Object {
                    props: p,
                    trailing_comma,
                };
                Some(if legacy_key {
                    Expr::LegacyLiteral(Box::new(object))
                } else {
                    object
                })
            }
            Token::KwFunction => {
                self.adv();
                // Generator expression: `function*`.
                let is_generator = self.eat(&Token::Star);
                self.fn_expr_tail(is_generator, false)
            }
            Token::BigInt(digits) => {
                let literal = Expr::BigIntLiteral(digits.clone());
                self.adv();
                Some(literal)
            }
            Token::Regex(pattern, flags) => {
                let literal = Expr::Regex(pattern.clone(), flags.clone());
                self.adv();
                Some(literal)
            }
            Token::KwAsync => self.async_expr(),
            // `class` in expression position.
            Token::KwClass => {
                let Statement::ClassDecl {
                    name,
                    superclass,
                    body,
                } = self.class_decl_named(Some(ANONYMOUS_CLASS))?
                else {
                    return None;
                };
                Some(Expr::ClassExpr {
                    name: (name != ANONYMOUS_CLASS).then_some(name),
                    superclass,
                    body,
                })
            }
            Token::KwNew => {
                self.adv();
                if self.eat(&Token::Dot) {
                    if !matches!(self.cur(), Token::Identifier(name) if name == "target") {
                        self.record_error("expected target after new.".into());
                        return None;
                    }
                    self.adv();
                    return Some(Expr::NewTarget);
                }
                let c = self.new_callee()?;
                let a = if self.eat(&Token::LParen) {
                    let mut ag = Vec::new();
                    while self.until(&Token::RParen) {
                        if let Some(arg) = self.with_in(true, Self::assign) {
                            ag.push(arg);
                        } else {
                            self.record_error("expected call argument".into());
                            self.adv();
                        }
                        if !matches!(self.cur(), Token::RParen) {
                            self.expect(&Token::Comma);
                        }
                    }
                    self.expect(&Token::RParen);
                    ag
                } else {
                    vec![]
                };
                Some(Expr::New {
                    callee: Box::new(c),
                    args: a,
                })
            }
            Token::KwImport => {
                self.adv();
                let phase = if self.eat(&Token::Dot) {
                    if matches!(self.cur(), Token::EscapedIdentifier(_)) {
                        self.record_error("escaped import meta-property name".into());
                        return None;
                    }
                    let name = self.ident()?;
                    match name.as_str() {
                        "meta" => return Some(Expr::ImportMeta),
                        "source" => super::ImportPhase::Source,
                        "defer" => super::ImportPhase::Deferred,
                        _ => {
                            self.record_error("invalid import phase".into());
                            return None;
                        }
                    }
                } else {
                    super::ImportPhase::Evaluation
                };
                self.expect(&Token::LParen);
                if matches!(self.cur(), Token::DotDotDot) {
                    self.record_error("spread argument in import call".into());
                }
                let specifier = self.with_in(true, Self::assign)?;
                self.expect(&Token::RParen);
                Some(Expr::DynamicImport {
                    specifier: Box::new(specifier),
                    phase,
                })
            }
            Token::KwAwait if !self.await_expression => {
                self.adv();
                Some(Expr::Identifier("await".into()))
            }
            Token::KwYield if !self.yield_expression => {
                self.adv();
                Some(Expr::Identifier("yield".into()))
            }
            Token::Hash => {
                self.adv();
                let name = self.ident_or_keyword()?;
                Some(Expr::Identifier(format!("#{name}")))
            }
            Token::Identifier(_)
            | Token::EscapedIdentifier(_)
            | Token::KwAs
            | Token::KwLet
            | Token::KwStatic
            | Token::KwConstructor
            | Token::KwFrom
            | Token::KwGet
            | Token::KwOf
            | Token::KwSet => {
                let span = self.cur_span();
                let nm = self.ident()?;
                // Every identifier read in expression position is a reference
                // the language server can resolve back to its declaration.
                self.record(&nm, span, crate::parser::Occurrence::Reference, None);
                Some(Expr::Identifier(nm))
            }
            Token::DotDotDot => {
                self.adv();
                let i = self.with_in(true, Self::assign)?;
                Some(Expr::Spread(Box::new(i)))
            }
            _ => None,
        }
    }

    /// Parses the callee of a `new` expression: a primary expression followed
    /// by member access (dot / computed), but stopping before call arguments so
    /// that `new Foo(1, 2)` treats `(1, 2)` as the constructor's arguments.
    fn new_callee(&mut self) -> Option<Expr> {
        let direct_import = matches!(self.cur(), Token::KwImport);
        let mut e = self.primary()?;
        if direct_import && matches!(e, Expr::DynamicImport { .. }) {
            self.record_error("import call cannot be a new expression callee".into());
        }
        loop {
            match self.cur() {
                Token::Dot => {
                    self.adv();
                    let p = self.ident_or_keyword()?;
                    e = Expr::Member {
                        object: Box::new(e),
                        property: Box::new(Expr::String((p).into())),
                        computed: false,
                    };
                }
                Token::LBracket => {
                    self.adv();
                    let p = self.with_in(true, Self::expr)?;
                    self.expect(&Token::RBracket);
                    e = Expr::Member {
                        object: Box::new(e),
                        property: Box::new(p),
                        computed: true,
                    };
                }
                _ => break,
            }
        }
        Some(e)
    }

    /// Consume a `TemplateQuasi` token if present, returning both its cooked
    /// and raw text (empty when the quasi is absent).
    pub(crate) fn take_quasi(&mut self) -> crate::lexer::TemplateChunk {
        if let Token::TemplateQuasi(q) = self.cur() {
            let s = q.clone();
            self.adv();
            s
        } else {
            crate::lexer::TemplateChunk::default()
        }
    }

    /// Parse the body of a template literal, positioned just after the opening
    /// backtick. Shared by plain and tagged templates.
    pub(crate) fn template_body(
        &mut self,
    ) -> Option<(Vec<crate::lexer::TemplateChunk>, Vec<Expr>)> {
        let mut quasis = Vec::new();
        let mut exprs = Vec::new();
        // Leading quasi (possibly empty).
        quasis.push(self.take_quasi());
        while matches!(self.cur(), Token::DollarLBrace) {
            self.adv();
            exprs.push(self.expr()?);
            self.expect(&Token::RBrace);
            quasis.push(self.take_quasi());
        }
        self.eat(&Token::Backtick);
        Some((quasis, exprs))
    }

    /// Speculatively parse `( params ) =>`. On any failure, restore the parser
    /// position and return `None` so the caller can parse a parenthesized expr.
    fn try_arrow(&mut self, is_async: bool) -> Option<Expr> {
        let saved = self.await_expression;
        self.await_expression |= is_async;
        let result = self.try_arrow_parameters(is_async);
        self.await_expression = saved;
        result
    }

    fn try_arrow_parameters(&mut self, is_async: bool) -> Option<Expr> {
        let save = self.pos;
        if !self.eat(&Token::LParen) {
            return None;
        }
        let mut params = Vec::new();
        let mut defaults = Vec::new();
        if self.eat(&Token::RParen) {
            if self.eat(&Token::Arrow) {
                return Some(self.arrow_body_async(params, defaults, is_async));
            }
            self.pos = save;
            return None;
        }
        loop {
            match self.cur() {
                Token::DotDotDot => {
                    self.adv();
                    if let Some(name) = self.ident() {
                        params.push(format!("...{}", name));
                    } else {
                        self.pos = save;
                        return None;
                    }
                }
                // A destructured arrow parameter: `({ a }) => a`.
                Token::LBracket | Token::LBrace => {
                    let Some(pattern) = self.pattern() else {
                        self.pos = save;
                        return None;
                    };
                    let slot = format!("*pattern{}*", params.len());
                    if self.eat(&Token::Equal)
                        && let Some(default) = self.assign()
                    {
                        defaults.push(Parser::default_guard(&slot, default));
                    }
                    defaults.push(Statement::VarDecl {
                        kind: crate::parser::VarKind::Let,
                        name: String::new(),
                        init: Some(Box::new(Expr::Identifier(slot.clone()))),
                        destructuring: Some(Box::new(pattern)),
                    });
                    params.push(slot);
                }
                _ => {
                    let Some(name) = self.ident() else {
                        self.pos = save;
                        return None;
                    };
                    if self.eat(&Token::Equal) {
                        match self.assign() {
                            Some(d) => defaults.push(Parser::default_guard(&name, d)),
                            None => {
                                self.pos = save;
                                return None;
                            }
                        }
                    }
                    params.push(name);
                }
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        if !self.eat(&Token::RParen) {
            self.pos = save;
            return None;
        }
        let newline = self.line_break_before_current();
        if !self.eat(&Token::Arrow) {
            self.pos = save;
            return None;
        }
        if newline {
            self.record_error("line terminator before arrow".into());
        }
        Some(self.arrow_body_async(params, defaults, is_async))
    }

    /// Parse a function expression after its `function` (and any `*`) token.
    fn fn_expr_tail(&mut self, is_generator: bool, is_async: bool) -> Option<Expr> {
        let n = self.with_grammar(is_async, is_generator, Self::ident);
        self.eat(&Token::LParen);
        let (p, defaults, b) = self.callable_parts(is_async, is_generator);
        self.check_parameters(&p, &defaults, &b, is_async || is_generator);
        let body = Self::function_body(&p, defaults, b);
        Some(Expr::FnExpr {
            name: n,
            params: p,
            body,
            is_async,
            is_generator,
        })
    }

    pub(super) fn arrow_body(&mut self, params: Vec<String>, defaults: Vec<Statement>) -> Expr {
        self.arrow_body_async(params, defaults, false)
    }

    pub(super) fn arrow_body_async(
        &mut self,
        params: Vec<String>,
        defaults: Vec<Statement>,
        is_async: bool,
    ) -> Expr {
        // The parameters were consumed before the scope existed, so they are
        // recorded here, in the body's scope, where they belong.
        let arrow_scope = self.push_scope(true);
        let in_expression = self.in_expression;
        let expr = self.with_grammar(is_async, false, |parser| {
            parser.with_in(in_expression, |parser| {
                parser.arrow_body_in_scope(params, defaults, is_async)
            })
        });
        self.pop_scope(arrow_scope);
        expr
    }

    fn arrow_body_in_scope(
        &mut self,
        params: Vec<String>,
        defaults: Vec<Statement>,
        is_async: bool,
    ) -> Expr {
        if self.eat(&Token::LBrace) {
            let b = self.with_in(true, Self::block_body);
            self.expect(&Token::RBrace);
            self.check_parameters(&params, &defaults, &b, true);
            let body = Self::function_body(&params, defaults, b);
            Expr::ArrowFn {
                params,
                body: Box::new(ExprOrBlock::Block(body)),
                is_async,
            }
        } else {
            let e = self.assign().unwrap_or(Expr::Undefined);
            self.check_parameters(&params, &defaults, &[], true);
            if defaults.is_empty() {
                Expr::ArrowFn {
                    params,
                    body: Box::new(ExprOrBlock::Expr(Box::new(e))),
                    is_async,
                }
            } else {
                let body = Self::function_body(
                    &params,
                    defaults,
                    vec![Statement::Return(Some(Box::new(e)))],
                );
                Expr::ArrowFn {
                    params,
                    body: Box::new(ExprOrBlock::Block(body)),
                    is_async,
                }
            }
        }
    }

    /// `async` in expression position: an async arrow (`async () => …`,
    /// `async x => …`) or an async function expression.
    ///
    /// `async` is contextual, so anything else — `async + 1`, a variable
    /// actually named `async` — falls back to the identifier.
    fn async_expr(&mut self) -> Option<Expr> {
        let save = self.pos;
        let newline = self.line_break_after_current();
        self.adv();
        if newline {
            return Some(Expr::Identifier("async".into()));
        }
        match self.cur() {
            Token::KwFunction => {
                self.adv();
                let is_generator = self.eat(&Token::Star);
                return self.fn_expr_tail(is_generator, true);
            }
            // `async x => …`
            Token::Identifier(name) | Token::EscapedIdentifier(name) => {
                let name = name.clone();
                self.adv();
                if self.eat(&Token::Arrow) {
                    return Some(self.arrow_body_async(vec![name], Vec::new(), true));
                }
            }
            Token::LParen => {
                if let Some(Expr::ArrowFn { params, body, .. }) = self.try_arrow(true) {
                    return Some(Expr::ArrowFn {
                        params,
                        body,
                        is_async: true,
                    });
                }
            }
            _ => {}
        }
        self.pos = save;
        self.adv();
        Some(Expr::Identifier("async".to_string()))
    }
}
