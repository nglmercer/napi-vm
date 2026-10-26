//! Compound statement parsers: classes, `import` / `export`, and the shared
//! block-body / default-guard helpers used across the parser.

use super::{AssignOp, BinOp, ClassMember, Expr, MemberName, Parser, Statement, VarKind};
use crate::lexer::Token;

/// Binding name synthesized for an anonymous `export default class`/`function`.
/// Not a valid JavaScript identifier, so guest code can never reference it.
pub(super) const DEFAULT_BINDING: &str = "*default*";

/// Collect the bound identifier names from a variable declaration statement
/// (which may be a single `VarDecl` or a block of them for `let a, b`).
fn declared_names(stmt: &Statement) -> Vec<String> {
    match stmt {
        Statement::VarDecl {
            destructuring: Some(pattern),
            ..
        } => super::pattern_names(pattern),
        Statement::VarDecl { name, .. } if !name.is_empty() => vec![name.clone()],
        Statement::Block(stmts) | Statement::Declarations(stmts) => {
            stmts.iter().flat_map(declared_names).collect()
        }
        _ => vec![],
    }
}

/// The declared name of a function or class declaration, as a one-element list.
fn decl_name(stmt: &Statement) -> Vec<String> {
    match stmt {
        Statement::FnDecl { name, .. } | Statement::ClassDecl { name, .. } => vec![name.clone()],
        _ => vec![],
    }
}

impl Parser {
    /// Consume `tok` only when it acts as a class-member modifier — that is,
    /// when it is not itself the member name (`get() {}`, `static = 1`).
    fn eat_modifier(&mut self, tok: &Token) -> bool {
        if matches!(self.cur(), t if t == tok)
            && !(matches!(tok, Token::KwGet | Token::KwSet) && matches!(self.peek(), Token::Star))
            && !matches!(
                self.peek(),
                Token::LParen | Token::Equal | Token::Semicolon | Token::RBrace
            )
        {
            self.adv();
            return true;
        }
        false
    }

    pub(super) fn decorator_list(&mut self) -> Option<Vec<Expr>> {
        let mut decorators = Vec::new();
        while self.eat(&Token::Unknown('@')) {
            let expression = if self.eat(&Token::LParen) {
                let expression = self.with_in(true, Self::expr)?;
                self.expect(&Token::RParen);
                Expr::Parenthesized(Box::new(expression))
            } else {
                if !matches!(
                    self.cur(),
                    Token::Identifier(_)
                        | Token::EscapedIdentifier(_)
                        | Token::KwYield
                        | Token::KwAwait
                        | Token::KwAsync
                        | Token::KwAs
                        | Token::KwLet
                        | Token::KwStatic
                        | Token::KwConstructor
                        | Token::KwFrom
                        | Token::KwGet
                        | Token::KwOf
                        | Token::KwSet
                ) {
                    self.record_error(
                        "expected decorator identifier or parenthesized expression".into(),
                    );
                    return None;
                }
                let mut expression = self.primary()?;
                while self.eat(&Token::Dot) {
                    let property = if let Token::PrivateIdentifier(name) = self.cur() {
                        let name = format!("#{name}");
                        self.adv();
                        name
                    } else {
                        self.ident_or_keyword()?
                    };
                    expression = Expr::Member {
                        object: Box::new(expression),
                        property: Box::new(Expr::String(property.into())),
                        computed: false,
                    };
                }
                if self.eat(&Token::LParen) {
                    let mut args = Vec::new();
                    while self.until(&Token::RParen) {
                        args.push(self.with_in(true, Self::assign)?);
                        if !matches!(self.cur(), Token::RParen) {
                            self.expect(&Token::Comma);
                        }
                    }
                    self.expect(&Token::RParen);
                    expression = Expr::Call {
                        callee: Box::new(expression),
                        args,
                    };
                }
                expression
            };
            decorators.push(expression);
        }
        Some(decorators)
    }

    pub(super) fn class_decl(&mut self) -> Option<Statement> {
        self.class_decl_named(None)
    }

    /// Parse a class declaration whose name may be omitted (`export default
    /// class { … }`), in which case `fallback` supplies the binding name.
    pub(crate) fn class_decl_named(&mut self, fallback: Option<&str>) -> Option<Statement> {
        let source_start = self.pos;
        let decorators = self.decorator_list()?;
        if matches!(self.cur(), Token::KwExport) && !decorators.is_empty() {
            let mut statement = self.export()?;
            let target = match &mut statement {
                Statement::Declarations(statements) => statements.first_mut()?,
                _ => {
                    self.record_error("decorators require an exported class".into());
                    return None;
                }
            };
            if let Statement::ClassDecl {
                decorators: inner, ..
            } = target
            {
                if !inner.is_empty() {
                    self.record_error("decorators cannot occur on both sides of export".into());
                }
                *inner = decorators;
                return Some(statement);
            }
            self.record_error("decorators require an exported class".into());
            return None;
        }
        if !self.eat(&Token::KwClass) {
            self.record_error("expected class after decorators".into());
            return None;
        }
        let name_span = self.cur_span();
        let n = match (self.cur(), fallback) {
            (Token::LBrace | Token::KwExtends, Some(name)) => name.to_string(),
            _ => self.ident()?,
        };
        self.record(
            &n,
            name_span,
            crate::parser::Occurrence::Declaration(crate::parser::DeclKind::Class),
            None,
        );
        let sc = if self.eat(&Token::KwExtends) {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.eat(&Token::LBrace);
        // The class body is its own scope: a method's name does not collide
        // with a binding outside the class.
        let class_scope = self.push_scope(true);
        let mut b = Vec::new();
        while self.until(&Token::RBrace) {
            if self.eat(&Token::Semicolon) {
                continue;
            }
            if self.eof() {
                break;
            }
            let member_decorators = self.decorator_list()?;
            // `static` / `get` / `set` are modifiers only when a member name
            // follows; `get() {}` and `set = 1` declare members named `get`.
            let st = self.eat_modifier(&Token::KwStatic);
            let member_start = self.pos;
            // `static { … }`: a static initialization block, not a member.
            if st && matches!(self.cur(), Token::LBrace) {
                if !member_decorators.is_empty() {
                    self.record_error("static blocks cannot be decorated".into());
                }
                self.adv();
                let body = self.with_grammar(false, false, Self::block_body);
                self.expect(&Token::RBrace);
                b.push(ClassMember::StaticBlock { body });
                continue;
            }
            let auto_accessor = matches!(self.cur(), Token::Identifier(name) if name == "accessor")
                && !self.line_break_after_current()
                && !matches!(
                    self.peek(),
                    Token::LParen | Token::Equal | Token::Semicolon | Token::RBrace
                );
            if auto_accessor {
                self.adv();
            }
            let is_async = !self.line_break_after_current() && self.eat_modifier(&Token::KwAsync);
            let is_generator = self.eat(&Token::Star);
            let is_getter = self.eat_modifier(&Token::KwGet);
            let is_setter = self.eat_modifier(&Token::KwSet);
            let member_span = self.cur_span();
            if matches!(self.cur(), Token::LegacyNumber(_) | Token::LegacyString(_)) {
                self.record_error("legacy literal class member name".into());
            }
            // `[expr]` evaluates when the class is defined; string and
            // numeric spellings name the same property as their bare form.
            let private = matches!(self.cur(), Token::PrivateIdentifier(_));
            let mn = if self.eat(&Token::LBracket) {
                let expr = self.with_in(true, Self::assign)?;
                self.expect(&Token::RBracket);
                MemberName::Computed(expr)
            } else {
                let name = match self.cur() {
                    Token::Identifier(x) | Token::EscapedIdentifier(x) => {
                        let v = x.clone();
                        self.adv();
                        v
                    }
                    Token::String(s) | Token::EscapedString(s) | Token::LegacyString(s) => {
                        let v = s.to_key();
                        self.adv();
                        v
                    }
                    Token::BigInt(digits) => {
                        let name = crate::bigint::BigInt::parse(digits).ok()?.to_string();
                        self.adv();
                        name
                    }
                    Token::Number(n) | Token::LegacyNumber(n) => {
                        let v = crate::format::ecmascript_number_string(*n);
                        self.adv();
                        v
                    }
                    Token::KwConstructor => {
                        self.adv();
                        "constructor".to_string()
                    }
                    Token::KwStatic => {
                        self.adv();
                        "static".to_string()
                    }
                    Token::KwGet => {
                        self.adv();
                        "get".to_string()
                    }
                    Token::KwSet => {
                        self.adv();
                        "set".to_string()
                    }
                    Token::KwAsync => {
                        self.adv();
                        "async".to_string()
                    }
                    // `#x`: a private field or method. The `#` is part of the
                    // name, which is what keeps it out of reach of ordinary
                    // property access — there is no way to write the name from
                    // outside the class body.
                    Token::PrivateIdentifier(name) => {
                        let name = format!("#{name}");
                        self.adv();
                        name
                    }
                    _ => self.ident_or_keyword()?,
                };
                if private {
                    MemberName::Private(name)
                } else {
                    MemberName::Static(name)
                }
            };
            if let MemberName::Static(name) | MemberName::Private(name) = &mn {
                self.record(
                    name,
                    member_span,
                    crate::parser::Occurrence::Declaration(
                        if matches!(self.cur(), Token::LParen) {
                            crate::parser::DeclKind::Method
                        } else {
                            crate::parser::DeclKind::Property
                        },
                    ),
                    None,
                );
            }
            if self.eat(&Token::LParen) {
                if auto_accessor {
                    self.record_error("auto-accessor cannot be a method".into());
                }
                let method_scope = self.push_scope(true);
                let (p, defaults, bd) = self.callable_parts(is_async, is_generator);
                self.check_accessor_parameters(&p, &defaults, is_getter, is_setter);
                if (is_getter || is_setter) && (is_async || is_generator) {
                    self.record_error("accessor cannot be async or a generator".into());
                }
                self.pop_scope(method_scope);
                self.check_parameters(&p, &defaults, &bd, true);
                let body = Self::function_body(&p, defaults, bd);
                if is_getter {
                    b.push(ClassMember::Getter {
                        decorators: member_decorators,
                        source_text: self.source_since(member_start),
                        name: mn,
                        is_static: st,
                        body,
                    });
                } else if is_setter {
                    let param = p.first().cloned().unwrap_or_default();
                    b.push(ClassMember::Setter {
                        decorators: member_decorators,
                        source_text: self.source_since(member_start),
                        name: mn,
                        param,
                        is_static: st,
                        body,
                    });
                } else {
                    b.push(ClassMember::Method {
                        decorators: member_decorators,
                        source_text: self.source_since(member_start),
                        name: mn,
                        is_static: st,
                        params: p,
                        body,
                        is_async,
                        is_generator,
                    });
                }
            } else {
                if is_async || is_generator || is_getter || is_setter {
                    self.record_error("method modifier on a field".into());
                }
                let i = if self.eat(&Token::Equal) {
                    Some(self.with_grammar(false, false, Self::assign)?)
                } else {
                    None
                };
                self.semi();
                b.push(ClassMember::Field {
                    decorators: member_decorators,
                    name: mn,
                    is_static: st,
                    auto_accessor,
                    init: i,
                });
            }
        }
        self.expect(&Token::RBrace);
        self.pop_scope(class_scope);
        Some(Statement::ClassDecl {
            decorators,
            source_text: self.source_since(source_start),
            name: n,
            superclass: sc,
            body: b,
        })
    }

    pub(super) fn export(&mut self) -> Option<Statement> {
        self.adv();
        if self.eat(&Token::KwDefault) {
            // `export default class …` / `export default function …` are
            // *declarations*, not expressions: bind the (possibly synthesized)
            // name first, then export that binding as the default.
            let decl = match self.cur() {
                Token::KwClass | Token::Unknown('@') => {
                    self.class_decl_named(Some(DEFAULT_BINDING))
                }
                Token::KwFunction => self.fn_decl_named(false, Some(DEFAULT_BINDING)),
                Token::KwAsync
                    if matches!(self.peek(), Token::KwFunction)
                        && !self.line_break_after_current() =>
                {
                    self.adv();
                    self.fn_decl_named(true, Some(DEFAULT_BINDING))
                }
                _ => None,
            };
            if let Some(decl) = decl {
                let name = decl_name(&decl).pop()?;
                // `Declarations` and not `Block`: the desugaring must leave
                // the binding in the scope the `export` was written in, not in
                // a nested one that ends at the semicolon.
                return Some(Statement::Declarations(vec![
                    decl,
                    Statement::ExportDefault(Box::new(Expr::Identifier(name))),
                ]));
            }
            let e = self.assign()?;
            self.semi();
            Some(Statement::ExportDefault(Box::new(e)))
        } else if self.eat(&Token::Star) {
            // `export * from 'm'` / `export * as ns from 'm'`.
            let alias = if self.eat(&Token::KwAs) {
                Some(self.import_specifier_name()?)
            } else {
                None
            };
            if !self.eat(&Token::KwFrom) {
                return None;
            }
            let source = match self.cur() {
                Token::String(x) | Token::EscapedString(x) => {
                    let v = x.clone();
                    self.adv();
                    v
                }
                _ => return None,
            };
            let attributes = self.import_attributes()?;
            self.semi();
            Some(Statement::ExportAll {
                attributes,
                source,
                alias,
            })
        } else if self.eat(&Token::LBrace) {
            let mut sp = Vec::new();
            let mut string_local = false;
            while self.until(&Token::RBrace) {
                string_local |= matches!(self.cur(), Token::String(_) | Token::EscapedString(_));
                // `export { default as x }` names the default export, so the
                // keyword is a valid specifier here.
                let l = self.import_specifier_name()?;
                let e = if self.eat(&Token::KwAs) {
                    self.import_specifier_name()?
                } else {
                    l.clone()
                };
                sp.push((l, e));
                if !matches!(self.cur(), Token::RBrace) {
                    self.eat(&Token::Comma);
                }
            }
            self.expect(&Token::RBrace);
            let s = if self.eat(&Token::KwFrom) {
                match self.cur() {
                    Token::String(x) | Token::EscapedString(x) => {
                        let v = x.clone();
                        self.adv();
                        Some(v)
                    }
                    _ => None,
                }
            } else {
                None
            };
            if s.is_none() && string_local {
                self.record_error("string export names require a source module".into());
            }
            let attributes = if s.is_some() {
                self.import_attributes()?
            } else {
                Vec::new()
            };
            self.semi();
            Some(Statement::ExportNamed {
                attributes,
                specifiers: sp,
                source: s,
            })
        } else {
            self.export_decl()
        }
    }

    /// Parse `export <declaration>` (`var`/`let`/`const`/`function`/`class`),
    /// declaring the binding and re-exporting the declared names. Desugars to a
    /// block of the declaration followed by an `export { name }` so the existing
    /// named-export evaluation (which reads from the global scope) applies.
    fn export_decl(&mut self) -> Option<Statement> {
        let (decl, names) = match self.cur() {
            Token::KwVar => {
                let d = self.var_decl(VarKind::Var)?;
                let n = declared_names(&d);
                (d, n)
            }
            Token::KwLet => {
                let d = self.var_decl(VarKind::Let)?;
                let n = declared_names(&d);
                (d, n)
            }
            Token::KwConst => {
                let d = self.var_decl(VarKind::Const)?;
                let n = declared_names(&d);
                (d, n)
            }
            Token::KwFunction => {
                let d = self.fn_decl(false)?;
                let n = decl_name(&d);
                (d, n)
            }
            Token::KwAsync
                if matches!(self.peek(), Token::KwFunction) && !self.line_break_after_current() =>
            {
                self.adv();
                let d = self.fn_decl(true)?;
                let n = decl_name(&d);
                (d, n)
            }
            Token::KwClass | Token::Unknown('@') => {
                let d = self.class_decl()?;
                let n = decl_name(&d);
                (d, n)
            }
            _ => {
                return Some(Statement::ExportNamed {
                    attributes: Vec::new(),
                    specifiers: vec![],
                    source: None,
                });
            }
        };
        let specifiers: Vec<(String, String)> = names.into_iter().map(|n| (n.clone(), n)).collect();
        // `Declarations` and not `Block`, so `export let x = 1` declares `x`
        // in the module scope rather than in the desugaring's own block.
        Some(Statement::Declarations(vec![
            decl,
            Statement::ExportNamed {
                attributes: Vec::new(),
                specifiers,
                source: None,
            },
        ]))
    }

    /// A name inside an `import { … }` list. `default` is a keyword but a
    /// legal specifier: `import { default as x } from 'm'`.
    fn import_specifier_name(&mut self) -> Option<String> {
        if let Token::String(value) | Token::EscapedString(value) = self.cur() {
            let value = value.clone();
            self.adv();
            return match value.to_utf8() {
                Ok(name) => Some(name),
                Err(_) => {
                    self.record_error("module export names must be well-formed Unicode".into());
                    None
                }
            };
        }
        self.ident_or_keyword()
    }

    /// Named imports share grammar whether or not a default import precedes
    /// them. A quoted ModuleExportName always requires an explicit local name.
    fn named_imports(&mut self) -> Option<Vec<(String, String)>> {
        let mut nd = Vec::new();
        while self.until(&Token::RBrace) {
            let string_import = matches!(self.cur(), Token::String(_) | Token::EscapedString(_));
            let imported = self.import_specifier_name()?;
            let local = if self.eat(&Token::KwAs) {
                self.ident()?
            } else {
                if string_import {
                    self.record_error("string import names require a local binding".into());
                }
                imported.clone()
            };
            nd.push((imported, local));
            if !matches!(self.cur(), Token::RBrace) {
                self.expect(&Token::Comma);
            }
        }
        self.expect(&Token::RBrace);
        Some(nd)
    }

    pub(super) fn import(&mut self) -> Option<Statement> {
        self.adv();
        let mut phase = super::ImportPhase::Evaluation;
        let def = if self.cur().identifier_name().is_some() {
            let source_modifier = matches!(self.cur(), Token::Identifier(name) if name == "source")
                && self.peek().identifier_name().is_some()
                && (!matches!(self.peek(), Token::KwFrom)
                    || self
                        .toks
                        .get(self.pos + 2)
                        .is_some_and(|(token, _)| matches!(token, Token::KwFrom)));
            let defer_modifier = matches!(self.cur(), Token::Identifier(name) if name == "defer");
            let nm = self.ident()?;
            if source_modifier {
                phase = super::ImportPhase::Source;
                let binding = self.ident()?;
                let module = self.from()?;
                Some(Statement::Import {
                    attributes: Vec::new(),
                    module,
                    phase,
                    default: Some(binding),
                    named: vec![],
                    namespace: None,
                })
            } else if defer_modifier && self.eat(&Token::Star) {
                phase = super::ImportPhase::Deferred;
                self.expect(&Token::KwAs);
                let ns = self.ident()?;
                let m = self.from()?;
                Some(Statement::Import {
                    attributes: Vec::new(),
                    module: m,
                    phase,
                    default: None,
                    named: vec![],
                    namespace: Some(ns),
                })
            } else if self.eat(&Token::Comma) {
                if self.eat(&Token::LBrace) {
                    let nd = self.named_imports()?;
                    let m = self.from()?;
                    Some(Statement::Import {
                        attributes: Vec::new(),
                        module: m,
                        phase,
                        default: Some(nm),
                        named: nd,
                        namespace: None,
                    })
                } else if self.eat(&Token::Star) {
                    self.expect(&Token::KwAs);
                    let ns = self.ident()?;
                    let m = self.from()?;
                    Some(Statement::Import {
                        attributes: Vec::new(),
                        module: m,
                        phase,
                        default: Some(nm),
                        named: vec![],
                        namespace: Some(ns),
                    })
                } else {
                    None
                }
            } else if matches!(self.cur(), Token::KwFrom) {
                let m = self.from()?;
                Some(Statement::Import {
                    attributes: Vec::new(),
                    module: m,
                    phase,
                    default: Some(nm),
                    named: vec![],
                    namespace: None,
                })
            } else {
                None
            }
        } else if self.eat(&Token::Star) {
            self.expect(&Token::KwAs);
            let ns = self.ident()?;
            let m = self.from()?;
            Some(Statement::Import {
                attributes: Vec::new(),
                module: m,
                phase,
                default: None,
                named: vec![],
                namespace: Some(ns),
            })
        } else if self.eat(&Token::LBrace) {
            let nd = self.named_imports()?;
            let m = self.from()?;
            Some(Statement::Import {
                attributes: Vec::new(),
                module: m,
                phase,
                default: None,
                named: nd,
                namespace: None,
            })
        } else if let Token::String(s) | Token::EscapedString(s) = self.cur() {
            let m = s.clone();
            self.adv();
            Some(Statement::Import {
                attributes: Vec::new(),
                module: m,
                phase,
                default: None,
                named: vec![],
                namespace: None,
            })
        } else {
            None
        };
        let attributes = self.import_attributes()?;
        let def = def.map(|mut declaration| {
            if let Statement::Import {
                attributes: stored, ..
            } = &mut declaration
            {
                *stored = attributes;
            }
            declaration
        });
        self.semi();
        def
    }

    fn import_attributes(&mut self) -> Option<Vec<(String, crate::JsString)>> {
        let mut attributes = Vec::new();
        if !matches!(self.cur(), Token::KwWith) {
            return Some(attributes);
        }
        self.adv();
        self.expect(&Token::LBrace);
        while self.until(&Token::RBrace) {
            let key = match self.cur().clone() {
                Token::String(value) | Token::EscapedString(value) => {
                    self.adv();
                    value.to_key()
                }
                _ => self.ident_or_keyword()?,
            };
            self.expect(&Token::Colon);
            let value = match self.cur().clone() {
                Token::String(value) | Token::EscapedString(value) => {
                    self.adv();
                    value
                }
                _ => {
                    self.record_error("import attribute values must be strings".into());
                    return None;
                }
            };
            if attributes.iter().any(|(name, _)| name == &key) {
                self.record_error("duplicate import attribute".into());
            }
            attributes.push((key, value));
            if !matches!(self.cur(), Token::RBrace) {
                self.expect(&Token::Comma);
            }
        }
        self.expect(&Token::RBrace);
        Some(attributes)
    }

    // Preserve the ModuleSpecifier StringValue independently of the host
    // loader's encoding; validation belongs to module resolution.
    fn from(&mut self) -> Option<crate::JsString> {
        self.expect(&Token::KwFrom);
        match self.cur() {
            Token::String(s) | Token::EscapedString(s) | Token::LegacyString(s) => {
                let v = s.clone();
                self.adv();
                Some(v)
            }
            _ => None,
        }
    }

    /// Parse statements up to a closing brace, in a *new* lexical scope.
    ///
    /// The scope is what keeps two same-named `let`s in sibling blocks apart,
    /// which is what makes rename safe.
    pub(crate) fn block_body(&mut self) -> Vec<Statement> {
        let outer = self.push_scope(false);
        let body = self.with_statement_list(Self::block_body_inner);
        self.pop_scope(outer);
        body
    }

    fn block_body_inner(&mut self) -> Vec<Statement> {
        let mut s = Vec::new();
        while self.until(&Token::RBrace) {
            if self.eof() {
                break;
            }
            if let Some(st) = self.statement_list_item() {
                s.push(st);
            } else {
                self.record_error("invalid statement in block".into());
                break;
            }
        }
        s
    }

    pub(crate) fn function_body(
        params: &[String],
        initializers: Vec<Statement>,
        mut body: Vec<Statement>,
    ) -> Vec<Statement> {
        if !initializers.is_empty() {
            body.insert(
                0,
                Statement::ParameterInitialization {
                    params: params.to_vec(),
                    initializers,
                    fields: vec![],
                },
            );
        }
        body
    }

    /// Build the guard statement implementing a parameter default value:
    /// `if (name === undefined) name = <default>;`
    pub(crate) fn default_guard(name: &str, d: Expr) -> Statement {
        Statement::If {
            test: Box::new(Expr::Binary {
                op: BinOp::Seq,
                left: Box::new(Expr::Identifier(name.to_string())),
                right: Box::new(Expr::Undefined),
            }),
            then: vec![Statement::Expr(Expr::Assignment {
                target: Box::new(Expr::Identifier(name.to_string())),
                op: AssignOp::Assign,
                value: Box::new(d),
            })],
            else_: None,
        }
    }
}
