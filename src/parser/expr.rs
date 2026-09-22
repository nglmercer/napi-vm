//! The expression precedence ladder, from comma (lowest) down to postfix.
//! Primary expressions live in `primary.rs`.

use super::{AssignOp, BinOp, Expr, LogicalAssignOp, Parser, UnOp};
use crate::lexer::Token;

impl Parser {
    pub(crate) fn expr(&mut self) -> Option<Expr> {
        // Nesting guard: parenthesized sub-expressions, call arguments and
        // similar re-enter `expr()` recursively, so this counter bounds the
        // parser's native stack depth. See `Parser::enter`.
        if !self.enter() {
            return None;
        }
        let r = self.comma();
        self.leave();
        r
    }

    /// The comma operator sits at the lowest precedence. It is only parsed in
    /// contexts that explicitly call `expr()`; list contexts (array elements,
    /// call arguments, object values, ...) call `assign()` so the comma is
    /// treated as a separator instead.
    fn comma(&mut self) -> Option<Expr> {
        let mut l = self.assign()?;
        while self.eat(&Token::Comma) {
            let r = self.assign()?;
            l = Expr::Binary {
                op: BinOp::Comma,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    pub(crate) fn assign(&mut self) -> Option<Expr> {
        // `yield` sits at assignment precedence. A bare `yield` (followed by a
        // terminator) yields `undefined`; otherwise it yields the operand.
        if matches!(self.cur(), Token::KwYield) {
            self.adv();
            // `yield* iterable` delegates to another iterator.
            if self.eat(&Token::Star) {
                let inner = self.assign()?;
                return Some(Expr::YieldFrom(Box::new(inner)));
            }
            let arg = if matches!(
                self.cur(),
                Token::Semicolon
                    | Token::RParen
                    | Token::RBrace
                    | Token::RBracket
                    | Token::Comma
                    | Token::EOF
            ) {
                None
            } else {
                Some(Box::new(self.assign()?))
            };
            return Some(Expr::Yield(arg));
        }
        let l = self.cond()?;
        match self.cur() {
            Token::Equal => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Assign,
                    value: Box::new(v),
                })
            }
            Token::PlusEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Add,
                    value: Box::new(v),
                })
            }
            Token::MinusEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Sub,
                    value: Box::new(v),
                })
            }
            Token::StarEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Mul,
                    value: Box::new(v),
                })
            }
            Token::SlashEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Div,
                    value: Box::new(v),
                })
            }
            Token::PercentEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Mod,
                    value: Box::new(v),
                })
            }
            Token::StarStarEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Pow,
                    value: Box::new(v),
                })
            }
            Token::AmpEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::BitAnd,
                    value: Box::new(v),
                })
            }
            Token::PipeEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::BitOr,
                    value: Box::new(v),
                })
            }
            Token::CaretEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::BitXor,
                    value: Box::new(v),
                })
            }
            Token::ShlEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Shl,
                    value: Box::new(v),
                })
            }
            Token::ShrEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::Shr,
                    value: Box::new(v),
                })
            }
            Token::UShrEqual => {
                self.adv();
                let v = self.assign()?;
                Some(Expr::Assignment {
                    target: Box::new(l),
                    op: AssignOp::UShr,
                    value: Box::new(v),
                })
            }
            Token::AndEqual | Token::OrEqual | Token::NullishEqual => {
                let op = match self.cur() {
                    Token::AndEqual => LogicalAssignOp::And,
                    Token::OrEqual => LogicalAssignOp::Or,
                    _ => LogicalAssignOp::Nullish,
                };
                self.adv();
                let v = self.assign()?;
                Some(Expr::LogicalAssignment {
                    target: Box::new(l),
                    op,
                    value: Box::new(v),
                })
            }
            _ => Some(l),
        }
    }

    fn cond(&mut self) -> Option<Expr> {
        let t = self.nullish()?;
        if self.eat(&Token::Question) {
            let c = self.assign()?;
            self.eat(&Token::Colon);
            let a = self.assign()?;
            Some(Expr::Conditional {
                test: Box::new(t),
                consequent: Box::new(c),
                alternate: Box::new(a),
            })
        } else {
            Some(t)
        }
    }

    fn nullish(&mut self) -> Option<Expr> {
        let mut l = self.or()?;
        while self.eat(&Token::QuestionQuestion) {
            let r = self.or()?;
            l = Expr::Binary {
                op: BinOp::Nullish,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    fn or(&mut self) -> Option<Expr> {
        let mut l = self.and()?;
        while self.eat(&Token::Or) {
            let r = self.and()?;
            l = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    fn and(&mut self) -> Option<Expr> {
        let mut l = self.bitor()?;
        while self.eat(&Token::And) {
            let r = self.bitor()?;
            l = Expr::Binary {
                op: BinOp::And,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    fn bitor(&mut self) -> Option<Expr> {
        let mut l = self.bitxor()?;
        while self.eat(&Token::BitOr) {
            let r = self.bitxor()?;
            l = Expr::Binary {
                op: BinOp::BitOr,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    fn bitxor(&mut self) -> Option<Expr> {
        let mut l = self.bitand()?;
        while self.eat(&Token::BitXor) {
            let r = self.bitand()?;
            l = Expr::Binary {
                op: BinOp::BitXor,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    fn bitand(&mut self) -> Option<Expr> {
        let mut l = self.eq()?;
        while self.eat(&Token::BitAnd) {
            let r = self.eq()?;
            l = Expr::Binary {
                op: BinOp::BitAnd,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    fn eq(&mut self) -> Option<Expr> {
        let mut l = self.cmp()?;
        loop {
            match self.cur() {
                Token::EqualEqual => {
                    self.adv();
                    let r = self.cmp()?;
                    l = Expr::Binary {
                        op: BinOp::Eq,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::NotEqual => {
                    self.adv();
                    let r = self.cmp()?;
                    l = Expr::Binary {
                        op: BinOp::Neq,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::EqualEqualEqual => {
                    self.adv();
                    let r = self.cmp()?;
                    l = Expr::Binary {
                        op: BinOp::Seq,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::NotEqualEqual => {
                    self.adv();
                    let r = self.cmp()?;
                    l = Expr::Binary {
                        op: BinOp::Sneq,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                _ => break,
            }
        }
        Some(l)
    }

    fn cmp(&mut self) -> Option<Expr> {
        let mut l = self.shift()?;
        loop {
            match self.cur() {
                Token::Less => {
                    self.adv();
                    let r = self.shift()?;
                    l = Expr::Binary {
                        op: BinOp::Lt,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::Greater => {
                    self.adv();
                    let r = self.shift()?;
                    l = Expr::Binary {
                        op: BinOp::Gt,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::LessEqual => {
                    self.adv();
                    let r = self.shift()?;
                    l = Expr::Binary {
                        op: BinOp::Le,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::GreaterEqual => {
                    self.adv();
                    let r = self.shift()?;
                    l = Expr::Binary {
                        op: BinOp::Ge,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::KwInstanceof => {
                    self.adv();
                    let r = self.shift()?;
                    l = Expr::Binary {
                        op: BinOp::Instanceof,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::KwIn => {
                    self.adv();
                    let r = self.shift()?;
                    l = Expr::Binary {
                        op: BinOp::In,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                _ => break,
            }
        }
        Some(l)
    }

    fn shift(&mut self) -> Option<Expr> {
        let mut l = self.add()?;
        loop {
            let op = match self.cur() {
                Token::Shl => BinOp::Shl,
                Token::Shr => BinOp::Shr,
                Token::UShr => BinOp::UShr,
                _ => break,
            };
            self.adv();
            let r = self.add()?;
            l = Expr::Binary {
                op,
                left: Box::new(l),
                right: Box::new(r),
            };
        }
        Some(l)
    }

    fn add(&mut self) -> Option<Expr> {
        let mut l = self.mul()?;
        loop {
            match self.cur() {
                Token::Plus => {
                    self.adv();
                    let r = self.mul()?;
                    l = Expr::Binary {
                        op: BinOp::Add,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::Minus => {
                    self.adv();
                    let r = self.mul()?;
                    l = Expr::Binary {
                        op: BinOp::Sub,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                _ => break,
            }
        }
        Some(l)
    }

    fn mul(&mut self) -> Option<Expr> {
        let mut l = self.exponent()?;
        loop {
            match self.cur() {
                Token::Star => {
                    self.adv();
                    let r = self.exponent()?;
                    l = Expr::Binary {
                        op: BinOp::Mul,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::Slash => {
                    self.adv();
                    let r = self.exponent()?;
                    l = Expr::Binary {
                        op: BinOp::Div,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                Token::Percent => {
                    self.adv();
                    let r = self.exponent()?;
                    l = Expr::Binary {
                        op: BinOp::Mod,
                        left: Box::new(l),
                        right: Box::new(r),
                    };
                }
                _ => break,
            }
        }
        Some(l)
    }

    fn exponent(&mut self) -> Option<Expr> {
        let base = self.unary()?;
        if self.eat(&Token::StarStar) {
            // Right-associative: 2 ** 3 ** 2 === 2 ** (3 ** 2)
            let exp = self.exponent()?;
            Some(Expr::Binary {
                op: BinOp::Pow,
                left: Box::new(base),
                right: Box::new(exp),
            })
        } else {
            Some(base)
        }
    }

    fn unary(&mut self) -> Option<Expr> {
        match self.cur() {
            Token::Not => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Not,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::Tilde => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::BitNot,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::Minus => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Neg,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::Plus => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Pos,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::KwTypeof => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Typeof,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::KwVoid => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Void,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::KwDelete => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Delete,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::PlusPlus => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Inc,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::MinusMinus => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Unary {
                    op: UnOp::Dec,
                    operand: Box::new(o),
                    prefix: true,
                })
            }
            Token::KwAwait => {
                self.adv();
                let o = self.unary()?;
                Some(Expr::Await(Box::new(o)))
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Option<Expr> {
        let mut e = self.primary()?;
        loop {
            match self.cur() {
                Token::LParen => {
                    self.adv();
                    let mut a = Vec::new();
                    while self.until(&Token::RParen) {
                        if let Some(arg) = self.assign() {
                            a.push(arg);
                        } else {
                            self.adv();
                        }
                        if !matches!(self.cur(), Token::RParen) {
                            self.eat(&Token::Comma);
                        }
                    }
                    self.expect(&Token::RParen);
                    e = Expr::Call {
                        callee: Box::new(e),
                        args: a,
                    };
                }
                Token::Dot => {
                    self.adv();
                    // `obj.#name`: a private member. The `#` is part of the
                    // property name, so nothing outside the class body can
                    // name it — that is the whole of the privacy.
                    if self.eat(&Token::Hash) {
                        let p = self.ident()?;
                        e = Expr::Member {
                            object: Box::new(e),
                            property: Box::new(Expr::String(format!("#{}", p))),
                            computed: false,
                        };
                        continue;
                    }
                    if self.eat(&Token::QuestionDot) {
                        // optional chaining: obj?.prop
                        let p = self.ident_or_keyword()?;
                        e = Expr::OptionalChain {
                            object: Box::new(e),
                            property: Box::new(Expr::String(p)),
                            computed: false,
                        };
                    } else {
                        let p = self.ident_or_keyword()?;
                        e = Expr::Member {
                            object: Box::new(e),
                            property: Box::new(Expr::String(p)),
                            computed: false,
                        };
                    }
                }
                Token::LBracket => {
                    self.adv();
                    let p = self.expr()?;
                    self.expect(&Token::RBracket);
                    e = Expr::Member {
                        object: Box::new(e),
                        property: Box::new(p),
                        computed: true,
                    };
                }
                Token::PlusPlus => {
                    self.adv();
                    e = Expr::Unary {
                        op: UnOp::Inc,
                        operand: Box::new(e),
                        prefix: false,
                    };
                }
                Token::MinusMinus => {
                    self.adv();
                    e = Expr::Unary {
                        op: UnOp::Dec,
                        operand: Box::new(e),
                        prefix: false,
                    };
                }
                Token::QuestionDot => {
                    self.adv();
                    if self.eat(&Token::LParen) {
                        // Optional call: obj?.(args)
                        let mut a = Vec::new();
                        while self.until(&Token::RParen) {
                            if let Some(arg) = self.assign() {
                                a.push(arg);
                            } else {
                                self.adv();
                            }
                            if !matches!(self.cur(), Token::RParen) {
                                self.eat(&Token::Comma);
                            }
                        }
                        self.expect(&Token::RParen);
                        e = Expr::Call {
                            callee: Box::new(Expr::OptionalChain {
                                object: Box::new(e),
                                property: Box::new(Expr::Undefined),
                                computed: false,
                            }),
                            args: a,
                        };
                    } else if self.eat(&Token::LBracket) {
                        // Optional computed member: obj?.[expr]
                        let p = self.assign()?;
                        self.expect(&Token::RBracket);
                        e = Expr::OptionalChain {
                            object: Box::new(e),
                            property: Box::new(p),
                            computed: true,
                        };
                    } else {
                        // Optional member: obj?.prop
                        let p = self.ident_or_keyword()?;
                        e = Expr::OptionalChain {
                            object: Box::new(e),
                            property: Box::new(Expr::String(p)),
                            computed: false,
                        };
                    }
                }
                // A template literal directly after an expression is a
                // *tagged* template: the tag receives the literal chunks and
                // the interpolated values as separate arguments.
                Token::Backtick => {
                    self.adv();
                    let (quasis, exprs) = self.template_body()?;
                    e = Expr::TaggedTemplate {
                        tag: Box::new(e),
                        cooked: quasis.iter().map(|q| q.cooked.clone()).collect(),
                        raw: quasis.into_iter().map(|q| q.raw).collect(),
                        exprs,
                    };
                }
                Token::Arrow => {
                    if let Expr::Identifier(n) = e {
                        self.adv();
                        e = self.arrow_body(vec![n], vec![]);
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        Some(e)
    }
}
