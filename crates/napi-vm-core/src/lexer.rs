/// One literal chunk of a template literal, in both forms: `cooked` has
/// escape sequences resolved, `raw` is the source text verbatim. Tagged
/// templates expose both; an ordinary template literal uses only `cooked`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateChunk {
    pub invalid_escape: bool,
    pub cooked: crate::JsString,
    pub raw: crate::JsString,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Number(f64),
    LegacyNumber(f64),
    /// A `BigInt` literal, carrying its digits (`123n` → `"123"`).
    BigInt(String),
    /// A character that begins no valid token, e.g. `@` or `#`.
    ///
    /// Carried through as a token rather than skipped so the parser can report
    /// a `SyntaxError` pointing at it. Dropping it silently let `x = #foo`
    /// parse as `x = foo` and run.
    Unknown(char),
    String(crate::JsString),
    /// A string literal containing escapes; it cannot be a Use Strict Directive.
    EscapedString(crate::JsString),
    LegacyString(crate::JsString),
    Identifier(String),
    /// IdentifierName containing a Unicode escape. It cannot act as a keyword.
    EscapedIdentifier(String),
    Plus,
    Minus,
    Star,
    Slash,
    /// A regular-expression literal: its source and its flags.
    Regex(crate::JsString, String),
    Percent,
    PlusPlus,
    MinusMinus,
    Equal,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    EqualEqual,
    NotEqual,
    EqualEqualEqual,
    NotEqualEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    And,
    Or,
    Not,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semicolon,
    Comma,
    Dot,
    Colon,
    Question,
    Arrow,
    DotDotDot,
    KwVar,
    KwLet,
    KwConst,
    KwFunction,
    KwReturn,
    KwIf,
    KwElse,
    KwFor,
    KwWhile,
    KwWith,
    KwDo,
    KwSwitch,
    KwCase,
    KwDefault,
    KwBreak,
    KwContinue,
    KwClass,
    KwExtends,
    KwNew,
    KwThis,
    KwSuper,
    KwImport,
    KwExport,
    KwFrom,
    KwAs,
    KwAsync,
    KwAwait,
    KwYield,
    KwTry,
    KwCatch,
    KwFinally,
    KwThrow,
    KwTypeof,
    KwInstanceof,
    KwIn,
    KwOf,
    KwTrue,
    KwFalse,
    KwNull,
    KwUndefined,
    KwDelete,
    KwVoid,
    KwDebugger,
    KwEnum,
    KwStatic,
    KwGet,
    KwSet,
    KwConstructor,
    BitAnd,
    BitOr,
    BitXor,
    Tilde,
    /// `#`, which begins a private class member name.
    Hash,
    /// A contiguous #IdentifierName; whitespace cannot split this token.
    PrivateIdentifier(String),
    Shl,
    Shr,
    UShr,
    StarStar,
    QuestionQuestion,
    /// `&&=`, `||=` and `??=`: the logical assignments, which short-circuit —
    /// the right side is not evaluated, and no write happens, unless the
    /// existing value calls for it.
    AndEqual,
    OrEqual,
    NullishEqual,
    QuestionDot,
    PercentEqual,
    AmpEqual,
    PipeEqual,
    CaretEqual,
    ShlEqual,
    ShrEqual,
    UShrEqual,
    StarStarEqual,
    Backtick,
    /// One literal chunk of a template. Ordinary template literals use the
    /// cooked text (escapes resolved); a *tagged* template also receives the
    /// raw text, which is why both are carried.
    TemplateQuasi(TemplateChunk),
    DollarLBrace,
    EOF,
}

impl Token {
    /// IdentifierName includes keywords; binding contexts impose their own restrictions.
    pub(crate) fn identifier_name(&self) -> Option<&str> {
        Some(match self {
            Self::Identifier(name) | Self::EscapedIdentifier(name) => name,
            Self::KwVar => "var",
            Self::KwLet => "let",
            Self::KwConst => "const",
            Self::KwFunction => "function",
            Self::KwReturn => "return",
            Self::KwIf => "if",
            Self::KwElse => "else",
            Self::KwFor => "for",
            Self::KwWhile => "while",
            Self::KwWith => "with",
            Self::KwDo => "do",
            Self::KwSwitch => "switch",
            Self::KwCase => "case",
            Self::KwDefault => "default",
            Self::KwBreak => "break",
            Self::KwContinue => "continue",
            Self::KwClass => "class",
            Self::KwExtends => "extends",
            Self::KwNew => "new",
            Self::KwThis => "this",
            Self::KwSuper => "super",
            Self::KwImport => "import",
            Self::KwExport => "export",
            Self::KwFrom => "from",
            Self::KwAs => "as",
            Self::KwAsync => "async",
            Self::KwAwait => "await",
            Self::KwYield => "yield",
            Self::KwTry => "try",
            Self::KwCatch => "catch",
            Self::KwFinally => "finally",
            Self::KwThrow => "throw",
            Self::KwTypeof => "typeof",
            Self::KwInstanceof => "instanceof",
            Self::KwIn => "in",
            Self::KwOf => "of",
            Self::KwTrue => "true",
            Self::KwFalse => "false",
            Self::KwNull => "null",
            Self::KwUndefined => "undefined",
            Self::KwDelete => "delete",
            Self::KwVoid => "void",
            Self::KwDebugger => "debugger",
            Self::KwEnum => "enum",
            Self::KwStatic => "static",
            Self::KwGet => "get",
            Self::KwSet => "set",
            Self::KwConstructor => "constructor",
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
struct LexicalCallable {
    depth: usize,
    body_depth: Option<usize>,
    expression: bool,
    is_async: bool,
    is_generator: bool,
    arrow: bool,
}

#[derive(Clone, Copy)]
struct LexicalMember {
    depth: usize,
    head: bool,
    async_names: usize,
    generator: bool,
}

pub struct Lexer {
    src: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    /// Tokens produced ahead of time (e.g. by template scanning), drained in
    /// FIFO order before lexing more source.
    pending: Vec<Token>,
    lexical_errors: Vec<crate::span::SpannedToken>,
    /// Spans corresponding to tokens in `pending`.
    pending_spans: Vec<crate::span::Span>,
    /// The last token emitted, which decides whether a `/` starts a regular
    /// expression or is a division operator. The two are indistinguishable
    /// from the character alone, so the grammar resolves it by context: after
    /// something that *ends a value* it is division, and otherwise it begins a
    /// literal.
    previous: Option<Token>,
    before_previous: Option<Token>,
    async_statement_start: bool,
    async_line_break: bool,
    last_token_end_line: usize,
    // Closing statement delimiters select InputElementRegExp; expression
    // delimiters select InputElementDiv. Function/class expressions retain
    // their expression goal even though their bodies contain statements.
    delimiters: Vec<(Token, bool)>,
    closed_statement: bool,
    callables: Vec<LexicalCallable>,
    members: Vec<LexicalMember>,
    async_arrow_depth: Option<usize>,
    pending_class: Option<(usize, bool)>,
    function_body: Option<bool>,
    encoded_source: bool,
    module_goal: bool,
    line_has_token: bool,
}

impl Lexer {
    pub fn new(s: &str) -> Self {
        Self {
            src: s.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            pending: Vec::new(),
            lexical_errors: Vec::new(),
            pending_spans: Vec::new(),
            previous: None,
            before_previous: None,
            async_statement_start: false,
            async_line_break: false,
            last_token_end_line: 1,
            delimiters: Vec::new(),
            closed_statement: false,
            callables: Vec::new(),
            members: Vec::new(),
            async_arrow_depth: None,
            pending_class: None,
            function_body: None,
            encoded_source: false,
            module_goal: false,
            line_has_token: false,
        }
    }

    /// Annex B HTML comments belong to Script/Function goals, never Module.
    pub fn with_module_goal(mut self, module: bool) -> Self {
        self.module_goal = module;
        self
    }

    /// Parse source supplied by JavaScript without replacing lone surrogates.
    pub fn from_js_string(source: &crate::JsString) -> Self {
        let mut lexer = Self::new(&source.to_key());
        lexer.encoded_source = true;
        lexer
    }
    fn source_unit_string(&mut self, c: char) -> crate::JsString {
        if self.encoded_source && c == '\u{FDD0}' {
            if self.src.get(self.pos) == Some(&'\u{FDD0}') {
                self.pos += 1;
                self.col += 1;
                return crate::JsString::from("\u{FDD0}");
            }
            if self.src.get(self.pos) == Some(&'s') && self.pos + 5 <= self.src.len() {
                let hex = self.src[self.pos + 1..self.pos + 5]
                    .iter()
                    .collect::<String>();
                if let Ok(u) = u16::from_str_radix(&hex, 16) {
                    self.pos += 5;
                    self.col += 5;
                    return crate::JsString::from_units(vec![u]);
                }
            }
        }
        c.to_string().into()
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        self.tokenize_with_spans()
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    pub fn tokenize_with_spans(&mut self) -> Vec<crate::span::SpannedToken> {
        let mut toks = Vec::new();
        loop {
            if let Some(t) = self.pending.pop() {
                let span = self
                    .pending_spans
                    .pop()
                    .unwrap_or(crate::span::Span::unknown());
                toks.push((t, span));
                self.line_has_token = true;
                continue;
            }
            self.skip_ws();
            if self.pos >= self.src.len() {
                break;
            }
            if let Some((t, span)) = self.next_with_span() {
                self.record_token(&t, span.line, span.end_line);
                self.line_has_token = true;
                toks.push((t, span));
            }
        }
        toks.append(&mut self.lexical_errors);
        let eof_span = crate::span::Span::new(self.line, self.col);
        toks.push((Token::EOF, eof_span));
        toks
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            let html_open = self.src[self.pos..].starts_with(&['<', '!', '-', '-']);
            let html_close =
                !self.line_has_token && self.src[self.pos..].starts_with(&['-', '-', '>']);
            if self.pos == 0 && self.src.starts_with(&['#', '!']) {
                while self.pos < self.src.len()
                    && !matches!(self.src[self.pos], '\n' | '\r' | '\u{2028}' | '\u{2029}')
                {
                    self.pos += 1;
                    self.col += 1;
                }
            } else if !self.module_goal && (html_open || html_close) {
                while self.pos < self.src.len()
                    && !matches!(self.src[self.pos], '\n' | '\r' | '\u{2028}' | '\u{2029}')
                {
                    self.pos += 1;
                    self.col += 1;
                }
            } else if matches!(c, '\n' | '\u{2028}' | '\u{2029}') {
                self.pos += 1;
                self.line += 1;
                self.col = 1;
                self.line_has_token = false;
            } else if c == '\r' {
                self.pos += 1;
                self.line += 1;
                self.col = 1;
                self.line_has_token = false;
                if self.pos < self.src.len() && self.src[self.pos] == '\n' {
                    self.pos += 1;
                }
            } else if matches!(
                c,
                '\t' | '\u{b}' | '\u{c}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                    ..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
            ) {
                self.pos += 1;
                self.col += 1;
            } else if c == '/' && self.pos + 1 < self.src.len() {
                let n = self.src[self.pos + 1];
                if n == '/' {
                    self.pos += 2;
                    self.col += 2;
                    while self.pos < self.src.len()
                        && !matches!(self.src[self.pos], '\n' | '\r' | '\u{2028}' | '\u{2029}')
                    {
                        self.pos += 1;
                        self.col += 1;
                    }
                } else if n == '*' {
                    let start = crate::span::Span::new(self.line, self.col);
                    self.pos += 2;
                    self.col += 2;
                    let mut closed = false;
                    while self.pos < self.src.len() {
                        if self.src[self.pos] == '*' && self.src.get(self.pos + 1) == Some(&'/') {
                            self.pos += 2;
                            self.col += 2;
                            closed = true;
                            break;
                        }
                        let c = self.src[self.pos];
                        self.pos += 1;
                        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                            if c == '\r' && self.src.get(self.pos) == Some(&'\n') {
                                self.pos += 1;
                            }
                            self.line += 1;
                            self.col = 1;
                            self.line_has_token = false;
                        } else {
                            self.col += 1;
                        }
                    }
                    if !closed {
                        self.lexical_errors.push((Token::Unknown('/'), start));
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Scan a template literal starting at the opening backtick, preserving the
    /// raw text of each quasi, with ECMAScript line-ending normalization. Emits:
    /// `Backtick Quasi (DollarLBrace <expr tokens> RBrace Quasi)* Backtick`.
    fn read_template(&mut self) -> Vec<crate::span::SpannedToken> {
        self.pos += 1; // consume opening backtick
        self.col += 1;
        let mut toks = vec![(
            Token::Backtick,
            crate::span::Span::new(self.line, self.col - 1),
        )];
        let mut quasi = TemplateChunk::default();
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == '`' {
                self.pos += 1;
                self.col += 1;
                let span = crate::span::Span::new(self.line, self.col - 1);
                toks.push((Token::TemplateQuasi(quasi), span));
                toks.push((Token::Backtick, span));
                return toks;
            } else if c == '$' && self.pos + 1 < self.src.len() && self.src[self.pos + 1] == '{' {
                self.pos += 2;
                self.col += 2;
                let span = crate::span::Span::new(self.line, self.col - 2);
                toks.push((Token::TemplateQuasi(quasi), span));
                quasi = TemplateChunk::default();
                toks.push((Token::DollarLBrace, span));
                self.lex_interp(&mut toks);
            } else if c == '\\' && self.pos + 1 < self.src.len() {
                let start = self.pos;
                self.pos += 1;
                self.col += 1;
                match self.read_escape(false) {
                    Ok(text) => quasi.cooked.push_str(text),
                    Err(()) => quasi.invalid_escape = true,
                }
                quasi.raw.push_str(
                    self.src[start..self.pos]
                        .iter()
                        .collect::<String>()
                        .replace("\r\n", "\n")
                        .replace('\r', "\n"),
                );
            } else {
                self.pos += 1;
                let text = if c == '\r' {
                    if self.src.get(self.pos) == Some(&'\n') {
                        self.pos += 1;
                    }
                    crate::JsString::from("\n")
                } else {
                    self.source_unit_string(c)
                };
                quasi.cooked.push_str(&text);
                quasi.raw.push_str(text);
                if matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}') {
                    self.line += 1;
                    self.col = 1;
                } else {
                    self.col += 1;
                }
            }
        }
        // Unterminated template: flush what we have.
        let span = crate::span::Span::new(self.line, self.col);
        toks.push((Token::TemplateQuasi(quasi), span));
        toks.push((Token::Unknown('`'), span));
        toks
    }

    /// Lex the expression inside a `${ ... }` interpolation, tracking brace depth
    /// so nested object literals and templates terminate correctly. Consumes the
    /// matching closing brace and appends `RBrace`.
    fn lex_interp(&mut self, toks: &mut Vec<crate::span::SpannedToken>) {
        // Each substitution starts in an expression lexical goal. Restore the
        // enclosing goal after scanning it, including nested template literals.
        let previous = self.previous.take();
        let before_previous = self.before_previous.take();
        let closed_statement = std::mem::replace(&mut self.closed_statement, false);
        let callables = self.callables.clone();
        let members = self.members.clone();
        let async_arrow_depth = self.async_arrow_depth;
        let pending_class = self.pending_class.take();
        let function_body = self.function_body.take();
        let async_statement_start = self.async_statement_start;
        let async_line_break = self.async_line_break;
        let last_token_end_line = self.last_token_end_line;
        let delimiter_depth = self.delimiters.len();
        let mut depth = 1i32;
        while self.pos < self.src.len() && depth > 0 {
            self.skip_ws();
            if self.pos >= self.src.len() {
                break;
            }
            let c = self.src[self.pos];
            if c == '`' {
                let start_line = self.line;
                let nested = self.read_template();
                toks.extend(nested);
                self.record_token(&Token::Backtick, start_line, self.line);
                continue;
            }
            if let Some((token, span)) = self.next_with_span() {
                match token {
                    Token::LBrace => depth += 1,
                    Token::RBrace => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    self.record_token(&token, span.line, span.end_line);
                }
                toks.push((token, span));
            }
        }
        self.previous = previous;
        self.before_previous = before_previous;
        self.closed_statement = closed_statement;
        self.callables = callables;
        self.members = members;
        self.async_arrow_depth = async_arrow_depth;
        self.pending_class = pending_class;
        self.function_body = function_body;
        self.async_statement_start = async_statement_start;
        self.async_line_break = async_line_break;
        self.last_token_end_line = last_token_end_line;
        self.delimiters.truncate(delimiter_depth);
    }

    fn next_with_span(&mut self) -> Option<crate::span::SpannedToken> {
        let mut span = crate::span::Span::new(self.line, self.col);
        let mut t = self.next()?;
        if matches!(
            t,
            Token::Number(_) | Token::LegacyNumber(_) | Token::BigInt(_)
        ) && let Some(&following) = self.src.get(self.pos)
            && (is_identifier_start(following) || following.is_ascii_digit() || following == '\\')
        {
            t = Token::Unknown(following);
        }
        // Templates emit individual spans for their delimiters/interpolations.
        if !matches!(t, Token::Backtick) {
            span.end_line = self.line;
        }
        Some((t, span))
    }

    fn next(&mut self) -> Option<Token> {
        let c = *self.src.get(self.pos)?;
        Some(match c {
            '(' => {
                self.pos += 1;
                self.col += 1;
                Token::LParen
            }
            ')' => {
                self.pos += 1;
                self.col += 1;
                Token::RParen
            }
            '{' => {
                self.pos += 1;
                self.col += 1;
                Token::LBrace
            }
            '}' => {
                self.pos += 1;
                self.col += 1;
                Token::RBrace
            }
            '[' => {
                self.pos += 1;
                self.col += 1;
                Token::LBracket
            }
            ']' => {
                self.pos += 1;
                self.col += 1;
                Token::RBracket
            }
            ';' => {
                self.pos += 1;
                self.col += 1;
                Token::Semicolon
            }
            ',' => {
                self.pos += 1;
                self.col += 1;
                Token::Comma
            }
            ':' => {
                self.pos += 1;
                self.col += 1;
                Token::Colon
            }
            '?' => match self.src.get(self.pos + 1) {
                Some('?') => {
                    if self.src.get(self.pos + 2) == Some(&'=') {
                        self.pos += 3;
                        self.col += 3;
                        Token::NullishEqual
                    } else {
                        self.pos += 2;
                        self.col += 2;
                        Token::QuestionQuestion
                    }
                }
                Some('.') if !self.src.get(self.pos + 2).is_some_and(char::is_ascii_digit) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::QuestionDot
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Question
                }
            },
            '`' => {
                let toks = self.read_template();
                let mut it = toks.into_iter();
                let first = it
                    .next()
                    .unwrap_or((Token::Backtick, crate::span::Span::unknown()));
                // Buffer the rest (reversed, since `pending` is popped from the back).
                for (t, s) in it.rev() {
                    self.pending.push(t);
                    self.pending_spans.push(s);
                }
                return Some(first.0);
            }
            '$' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == '{' {
                    self.pos += 2;
                    self.col += 2;
                    Token::DollarLBrace
                } else {
                    self.read_ident()
                }
            }
            '.' if self.src.get(self.pos + 1).is_some_and(char::is_ascii_digit) => self.read_num(),
            '.' => {
                if self.pos + 2 < self.src.len()
                    && self.src[self.pos + 1] == '.'
                    && self.src[self.pos + 2] == '.'
                {
                    self.pos += 3;
                    self.col += 3;
                    Token::DotDotDot
                } else {
                    self.pos += 1;
                    self.col += 1;
                    Token::Dot
                }
            }
            '+' => match self.src.get(self.pos + 1) {
                Some('+') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::PlusPlus
                }
                Some('=') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::PlusEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Plus
                }
            },
            '-' => match self.src.get(self.pos + 1) {
                Some('-') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::MinusMinus
                }
                Some('=') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::MinusEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Minus
                }
            },
            '*' => match (self.src.get(self.pos + 1), self.src.get(self.pos + 2)) {
                (Some('*'), Some('=')) => {
                    self.pos += 3;
                    self.col += 3;
                    Token::StarStarEqual
                }
                (Some('*'), _) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::StarStar
                }
                (Some('='), _) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::StarEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Star
                }
            },
            '/' if self.regex_allowed() => match self.read_regex() {
                Some(regex) => regex,
                // Not a terminated regular expression after all, so it was a
                // division operator in a position that merely looked like an
                // expression start.
                None => self.read_slash(),
            },
            '/' => self.read_slash(),
            '%' => match self.src.get(self.pos + 1) {
                Some('=') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::PercentEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Percent
                }
            },
            '=' => match (self.src.get(self.pos + 1), self.src.get(self.pos + 2)) {
                (Some('='), Some('=')) => {
                    self.pos += 3;
                    self.col += 3;
                    Token::EqualEqualEqual
                }
                (Some('='), _) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::EqualEqual
                }
                (Some('>'), _) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::Arrow
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Equal
                }
            },
            '!' => match (self.src.get(self.pos + 1), self.src.get(self.pos + 2)) {
                (Some('='), Some('=')) => {
                    self.pos += 3;
                    self.col += 3;
                    Token::NotEqualEqual
                }
                (Some('='), _) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::NotEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Not
                }
            },
            '<' => match (self.src.get(self.pos + 1), self.src.get(self.pos + 2)) {
                (Some('<'), Some('=')) => {
                    self.pos += 3;
                    self.col += 3;
                    Token::ShlEqual
                }
                (Some('<'), _) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::Shl
                }
                (Some('='), _) => {
                    self.pos += 2;
                    self.col += 2;
                    Token::LessEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::Less
                }
            },
            '>' => {
                let a = self.src.get(self.pos + 1);
                let b = self.src.get(self.pos + 2);
                let c = self.src.get(self.pos + 3);
                match (a, b, c) {
                    (Some('>'), Some('>'), Some('=')) => {
                        self.pos += 4;
                        self.col += 4;
                        Token::UShrEqual
                    }
                    (Some('>'), Some('>'), _) => {
                        self.pos += 3;
                        self.col += 3;
                        Token::UShr
                    }
                    (Some('>'), Some('='), _) => {
                        self.pos += 3;
                        self.col += 3;
                        Token::ShrEqual
                    }
                    (Some('>'), _, _) => {
                        self.pos += 2;
                        self.col += 2;
                        Token::Shr
                    }
                    (Some('='), _, _) => {
                        self.pos += 2;
                        self.col += 2;
                        Token::GreaterEqual
                    }
                    _ => {
                        self.pos += 1;
                        self.col += 1;
                        Token::Greater
                    }
                }
            }
            '&' => match self.src.get(self.pos + 1) {
                Some('&') => {
                    if self.src.get(self.pos + 2) == Some(&'=') {
                        self.pos += 3;
                        self.col += 3;
                        Token::AndEqual
                    } else {
                        self.pos += 2;
                        self.col += 2;
                        Token::And
                    }
                }
                Some('=') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::AmpEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::BitAnd
                }
            },
            '|' => match self.src.get(self.pos + 1) {
                Some('|') => {
                    if self.src.get(self.pos + 2) == Some(&'=') {
                        self.pos += 3;
                        self.col += 3;
                        Token::OrEqual
                    } else {
                        self.pos += 2;
                        self.col += 2;
                        Token::Or
                    }
                }
                Some('=') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::PipeEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::BitOr
                }
            },
            '^' => match self.src.get(self.pos + 1) {
                Some('=') => {
                    self.pos += 2;
                    self.col += 2;
                    Token::CaretEqual
                }
                _ => {
                    self.pos += 1;
                    self.col += 1;
                    Token::BitXor
                }
            },
            '~' => {
                self.pos += 1;
                self.col += 1;
                Token::Tilde
            }
            // `#` introduces a private class member name.
            '#' => {
                self.pos += 1;
                self.col += 1;
                if self
                    .src
                    .get(self.pos)
                    .is_some_and(|c| is_identifier_start(*c) || *c == '\\')
                {
                    let token = self.read_ident();
                    match token.identifier_name() {
                        Some(name) => Token::PrivateIdentifier(name.to_owned()),
                        None => token,
                    }
                } else {
                    Token::Unknown('#')
                }
            }
            '"' | '\'' => self.read_str(c),
            c if c.is_ascii_digit() => self.read_num(),
            c if is_identifier_start(c) || c == '\\' => self.read_ident(),
            _ => {
                self.pos += 1;
                self.col += 1;
                Token::Unknown(c)
            }
        })
    }

    /// Would a `/` here begin a regular-expression literal?
    ///
    /// It is division only when the previous token ends a value. Everything
    /// else — the start of the program, an operator, a keyword, an opening
    /// bracket — is a position where an expression may begin.
    fn record_token(&mut self, token: &Token, line: usize, end_line: usize) {
        self.async_line_break =
            matches!(self.previous, Some(Token::KwAsync)) && line > self.last_token_end_line;
        if self.async_line_break {
            self.async_arrow_depth = None;
            self.async_statement_start = false;
            if let Some(member) = self.members.last_mut() {
                member.async_names = member.async_names.saturating_sub(1);
            }
        }
        self.observe_token(token);
        self.last_token_end_line = end_line;
        self.before_previous = self.previous.replace(token.clone());
    }

    fn observe_token(&mut self, token: &Token) {
        let previous = self.previous.as_ref();
        let statement_start = previous.is_none()
            || matches!(
                previous,
                Some(
                    Token::Semicolon
                        | Token::LBrace
                        | Token::KwElse
                        | Token::KwExport
                        | Token::KwDefault
                )
            )
            || (self.closed_statement && matches!(previous, Some(Token::RBrace | Token::RParen)));
        let member_head = self
            .members
            .last()
            .is_some_and(|member| member.depth == self.delimiters.len() && member.head);
        if matches!(token, Token::Comma | Token::Semicolon) {
            while self.callables.last().is_some_and(|callable| {
                callable.arrow
                    && callable.body_depth.is_none()
                    && callable.depth == self.delimiters.len()
            }) {
                self.callables.pop();
            }
            if self
                .async_arrow_depth
                .is_some_and(|depth| depth == self.delimiters.len())
            {
                self.async_arrow_depth = None;
            }
        }
        if let Some(member) = self.members.last_mut()
            && member.depth == self.delimiters.len()
        {
            match token {
                Token::Comma | Token::Semicolon => {
                    member.head = true;
                    member.async_names = 0;
                    member.generator = false;
                }
                Token::Colon | Token::Equal => member.head = false,
                Token::KwAsync if member.head => member.async_names += 1,
                Token::Star if member.head => member.generator = true,
                _ => {}
            }
        }
        match token {
            Token::KwAsync => {
                self.async_statement_start = statement_start;
                if !matches!(previous, Some(Token::Dot | Token::QuestionDot)) {
                    self.async_arrow_depth = Some(self.delimiters.len());
                }
            }
            Token::KwFunction
                if !member_head && !matches!(previous, Some(Token::Dot | Token::QuestionDot)) =>
            {
                let statement = statement_start
                    || self.async_line_break
                    || matches!(previous, Some(Token::KwAsync)) && self.async_statement_start;
                self.callables.push(LexicalCallable {
                    depth: self.delimiters.len(),
                    body_depth: None,
                    expression: !statement,
                    is_async: matches!(previous, Some(Token::KwAsync)) && !self.async_line_break,
                    is_generator: false,
                    arrow: false,
                });
                self.async_arrow_depth = None;
            }
            Token::KwClass
                if !member_head && !matches!(previous, Some(Token::Dot | Token::QuestionDot)) =>
            {
                self.pending_class = Some((self.delimiters.len(), !statement_start));
            }
            Token::Star if matches!(previous, Some(Token::KwFunction)) => {
                if let Some(callable) = self.callables.last_mut() {
                    callable.is_generator = true;
                }
            }
            Token::Arrow => {
                let is_async = self.async_arrow_depth.take() == Some(self.delimiters.len());
                self.callables.push(LexicalCallable {
                    depth: self.delimiters.len(),
                    body_depth: None,
                    expression: true,
                    is_async,
                    is_generator: false,
                    arrow: true,
                });
            }
            Token::LParen => {
                if member_head
                    && previous.is_some_and(|token| {
                        token.identifier_name().is_some()
                            || matches!(
                                token,
                                Token::String(_)
                                    | Token::EscapedString(_)
                                    | Token::Number(_)
                                    | Token::BigInt(_)
                                    | Token::PrivateIdentifier(_)
                                    | Token::RBracket
                            )
                    })
                {
                    let member = self.members.last().unwrap();
                    self.callables.push(LexicalCallable {
                        depth: self.delimiters.len(),
                        body_depth: None,
                        expression: true,
                        is_async: member.async_names > 0
                            && (!matches!(previous, Some(Token::KwAsync))
                                || member.async_names > 1),
                        is_generator: member.generator,
                        arrow: false,
                    });
                    self.members.last_mut().unwrap().head = false;
                }
                let control =
                    !matches!(self.before_previous, Some(Token::Dot | Token::QuestionDot))
                        && matches!(
                            previous,
                            Some(
                                Token::KwIf
                                    | Token::KwWhile
                                    | Token::KwFor
                                    | Token::KwWith
                                    | Token::KwSwitch
                                    | Token::KwCatch
                            )
                        );
                self.delimiters.push((Token::LParen, control));
            }
            Token::RParen => {
                if let Some((Token::LParen, control)) = self.delimiters.pop() {
                    self.closed_statement = control;
                }
                if let Some(callable) = self.callables.last_mut()
                    && !callable.arrow
                    && callable.body_depth.is_none()
                    && callable.depth == self.delimiters.len()
                {
                    self.function_body = Some(callable.expression);
                    callable.body_depth = Some(self.delimiters.len() + 1);
                }
                self.finish_concise_arrows();
                return;
            }
            Token::LBracket => self.delimiters.push((Token::LBracket, false)),
            Token::RBracket => {
                self.delimiters.pop();
                self.finish_concise_arrows();
            }
            Token::LBrace => {
                let callable_body =
                    self.function_body.is_some() || matches!(previous, Some(Token::Arrow));
                let class_body = self
                    .pending_class
                    .is_some_and(|(depth, _)| depth == self.delimiters.len());
                if matches!(previous, Some(Token::Arrow))
                    && let Some(callable) = self.callables.last_mut()
                {
                    callable.body_depth = Some(self.delimiters.len() + 1);
                }
                let statement = if let Some(expression) = self.function_body.take() {
                    !expression
                } else if let Some((depth, expression)) = self.pending_class
                    && depth == self.delimiters.len()
                {
                    self.pending_class = None;
                    !expression
                } else {
                    statement_start
                        || matches!(
                            previous,
                            Some(Token::RParen | Token::KwTry | Token::KwFinally | Token::KwDo)
                        )
                };
                self.delimiters.push((Token::LBrace, statement));
                if class_body || !statement && !callable_body {
                    self.members.push(LexicalMember {
                        depth: self.delimiters.len(),
                        head: true,
                        async_names: 0,
                        generator: false,
                    });
                }
            }
            Token::RBrace => {
                let depth = self.delimiters.len();
                if self
                    .members
                    .last()
                    .is_some_and(|member| member.depth == depth)
                {
                    self.members.pop();
                }
                if self
                    .callables
                    .last()
                    .is_some_and(|callable| callable.body_depth == Some(depth))
                {
                    self.callables.pop();
                    if let Some(member) = self.members.last_mut() {
                        member.head = true;
                        member.async_names = 0;
                        member.generator = false;
                    }
                }
                if let Some((Token::LBrace, statement)) = self.delimiters.pop() {
                    self.closed_statement = statement;
                }
                self.finish_concise_arrows();
                return;
            }
            _ => {}
        }
        self.closed_statement = false;
    }

    fn finish_concise_arrows(&mut self) {
        while self.callables.last().is_some_and(|callable| {
            callable.arrow
                && callable.body_depth.is_none()
                && callable.depth > self.delimiters.len()
        }) {
            self.callables.pop();
        }
    }

    fn regex_allowed(&self) -> bool {
        if matches!(self.before_previous, Some(Token::Dot | Token::QuestionDot))
            && self
                .previous
                .as_ref()
                .is_some_and(|token| token.identifier_name().is_some())
        {
            return false;
        }
        if matches!(self.previous, Some(Token::KwAwait | Token::KwYield)) {
            if matches!(self.before_previous, Some(Token::Dot | Token::QuestionDot)) {
                return false;
            }
            let (await_allowed, yield_allowed) = self
                .callables
                .last()
                .map_or((self.module_goal, false), |callable| {
                    (callable.is_async, callable.is_generator)
                });
            return if matches!(self.previous, Some(Token::KwAwait)) {
                await_allowed
            } else {
                yield_allowed
            };
        }
        if self.closed_statement && matches!(self.previous, Some(Token::RParen | Token::RBrace)) {
            return true;
        }
        match &self.previous {
            None => true,
            Some(token) => !matches!(
                token,
                Token::Identifier(_)
                    | Token::EscapedIdentifier(_)
                    | Token::PrivateIdentifier(_)
                    | Token::Number(_)
                    | Token::BigInt(_)
                    | Token::LegacyNumber(_)
                    | Token::String(_)
                    | Token::EscapedString(_)
                    | Token::LegacyString(_)
                    | Token::Regex(_, _)
                    | Token::Backtick
                    | Token::RParen
                    | Token::RBracket
                    | Token::RBrace
                    | Token::PlusPlus
                    | Token::MinusMinus
                    | Token::KwAs
                    | Token::KwAsync
                    | Token::KwConstructor
                    | Token::KwFrom
                    | Token::KwGet
                    | Token::KwLet
                    | Token::KwOf
                    | Token::KwSet
                    | Token::KwStatic
                    | Token::KwUndefined
                    | Token::KwThis
                    | Token::KwSuper
                    | Token::KwNull
                    | Token::KwTrue
                    | Token::KwFalse
            ),
        }
    }

    /// The `/` and `/=` operators.
    fn read_slash(&mut self) -> Token {
        match self.src.get(self.pos + 1) {
            Some('=') => {
                self.pos += 2;
                self.col += 2;
                Token::SlashEqual
            }
            _ => {
                self.pos += 1;
                self.col += 1;
                Token::Slash
            }
        }
    }

    /// Scan `/pattern/flags`, positioned at the opening slash.
    ///
    /// A `/` inside a character class does not end the literal, which is why
    /// the scan tracks class depth rather than looking for the next slash.
    /// Returns `None` for an unterminated literal, leaving the cursor where it
    /// started so the caller can lex a division operator instead.
    fn read_regex(&mut self) -> Option<Token> {
        let start = self.pos;
        self.pos += 1;
        self.col += 1;
        let mut pattern = String::new();
        let mut in_class = false;
        loop {
            let Some(&c) = self.src.get(self.pos) else {
                self.pos = start;
                return None;
            };
            self.pos += 1;
            self.col += 1;
            match c {
                '\\' => {
                    pattern.push(c);
                    if let Some(&escaped) = self.src.get(self.pos) {
                        if matches!(escaped, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                            self.pos = start;
                            return None;
                        }
                        pattern.push(escaped);
                        self.pos += 1;
                        self.col += 1;
                    }
                }
                '[' => {
                    in_class = true;
                    pattern.push(c);
                }
                ']' => {
                    in_class = false;
                    pattern.push(c);
                }
                '/' if !in_class => break,
                // A line terminator ends a regular-expression literal's
                // reach; what looked like one is a division operator.
                '\n' | '\r' | '\u{2028}' | '\u{2029}' => {
                    self.pos = start;
                    return None;
                }
                _ => pattern.push(c),
            }
        }
        let mut flags = String::new();
        while let Some(&c) = self.src.get(self.pos) {
            if !c.is_ascii_alphabetic() {
                break;
            }
            flags.push(c);
            self.pos += 1;
            self.col += 1;
        }
        Some(Token::Regex(
            if self.encoded_source {
                crate::JsString::from_key(&pattern)
            } else {
                pattern.into()
            },
            flags,
        ))
    }

    fn read_escape(&mut self, legacy_allowed: bool) -> Result<crate::JsString, ()> {
        let e = *self.src.get(self.pos).ok_or(())?;
        self.pos += 1;
        self.col += 1;
        let text = match e {
            'n' => "\n",
            't' => "\t",
            'r' => "\r",
            'b' => "\u{0008}",
            'f' => "\u{000C}",
            'v' => "\u{000B}",
            '0' if !self.src.get(self.pos).is_some_and(char::is_ascii_digit) => "\0",
            '0'..='7' => {
                if !legacy_allowed {
                    return Err(());
                }
                let mut value = e.to_digit(8).ok_or(())?;
                let limit = if e <= '3' { 3 } else { 2 };
                for _ in 1..limit {
                    let Some(digit) = self.src.get(self.pos).and_then(|c| c.to_digit(8)) else {
                        break;
                    };
                    value = value * 8 + digit;
                    self.pos += 1;
                    self.col += 1;
                }
                return Ok(crate::JsString::from_units(vec![value as u16]));
            }
            '8' | '9' => {
                if !legacy_allowed {
                    return Err(());
                }
                return Ok(self.source_unit_string(e));
            }
            '\n' => {
                self.line += 1;
                self.col = 1;
                return Ok(crate::JsString::default());
            }
            '\u{2028}' | '\u{2029}' => {
                self.line += 1;
                self.col = 1;
                return Ok(crate::JsString::default());
            }
            '\r' => {
                if self.src.get(self.pos) == Some(&'\n') {
                    self.pos += 1;
                }
                self.line += 1;
                self.col = 1;
                return Ok(crate::JsString::default());
            }
            'u' | 'x' => {
                let braced = e == 'u' && self.src.get(self.pos) == Some(&'{');
                if braced {
                    self.pos += 1;
                    self.col += 1;
                }
                let mut value = 0u32;
                let mut digits = 0;
                let count = if e == 'x' { 2 } else { 4 };
                while self.pos < self.src.len() && (braced || digits < count) {
                    let c = self.src[self.pos];
                    if braced && c == '}' {
                        break;
                    }
                    let n = c.to_digit(16).ok_or(())?;
                    value = value
                        .checked_mul(16)
                        .and_then(|v| v.checked_add(n))
                        .ok_or(())?;
                    self.pos += 1;
                    self.col += 1;
                    digits += 1;
                }
                if braced {
                    if digits == 0 || self.src.get(self.pos) != Some(&'}') || value > 0x10FFFF {
                        return Err(());
                    }
                    self.pos += 1;
                    self.col += 1;
                } else if digits != count {
                    return Err(());
                }
                if value <= 0xFFFF {
                    return Ok(crate::JsString::from_units(vec![value as u16]));
                }
                return Ok(crate::JsString::from_units(vec![
                    (0xD800 + ((value - 0x10000) >> 10)) as u16,
                    (0xDC00 + ((value - 0x10000) & 1023)) as u16,
                ]));
            }
            other => return Ok(self.source_unit_string(other)),
        };
        Ok(text.into())
    }
    fn read_str(&mut self, q: char) -> Token {
        self.pos += 1;
        self.col += 1;
        let mut text = crate::JsString::default();
        let mut escaped = false;
        let mut legacy = false;
        while let Some(&c) = self.src.get(self.pos) {
            self.pos += 1;
            self.col += 1;
            if c == q {
                return if legacy {
                    Token::LegacyString(text)
                } else if escaped {
                    Token::EscapedString(text)
                } else {
                    Token::String(text)
                };
            }
            if c == '\n' || c == '\r' {
                return Token::Unknown(c);
            }
            if c == '\\' {
                escaped = true;
                legacy |= self
                    .src
                    .get(self.pos)
                    .is_some_and(|c| matches!(c, '1'..='9'))
                    || self.src.get(self.pos) == Some(&'0')
                        && self.src.get(self.pos + 1).is_some_and(char::is_ascii_digit);
                match self.read_escape(true) {
                    Ok(s) => text.push_str(s),
                    Err(()) => return Token::Unknown('\\'),
                }
            } else {
                let chunk = self.source_unit_string(c);
                text.push_str(chunk);
            }
        }
        Token::Unknown(q)
    }

    fn read_num(&mut self) -> Token {
        let s = self.pos;
        // Radix-prefixed literals: `0x1F`, `0b1010`, `0o17`, and their BigInt
        // forms. These have no fractional or exponent part, so they are read
        // separately from the decimal path below.
        if self.src[self.pos] == '0'
            && let Some(&prefix) = self.src.get(self.pos + 1)
            && matches!(prefix, 'x' | 'X' | 'b' | 'B' | 'o' | 'O')
        {
            let radix = match prefix {
                'x' | 'X' => 16,
                'b' | 'B' => 2,
                _ => 8,
            };
            self.pos += 2;
            self.col += 2;
            let digits_start = self.pos;
            while self.pos < self.src.len()
                && (self.src[self.pos].is_digit(radix) || self.src[self.pos] == '_')
            {
                self.pos += 1;
                self.col += 1;
            }
            let raw = &self.src[digits_start..self.pos];
            if raw.is_empty()
                || raw.iter().enumerate().any(|(index, c)| {
                    *c == '_'
                        && (index == 0
                            || !raw[index - 1].is_digit(radix)
                            || !raw.get(index + 1).is_some_and(|c| c.is_digit(radix)))
                })
            {
                return Token::Unknown('_');
            }
            let digits: String = self.src[digits_start..self.pos]
                .iter()
                .filter(|c| **c != '_')
                .collect();
            if self.pos < self.src.len() && self.src[self.pos] == 'n' {
                self.pos += 1;
                self.col += 1;
                let literal: String = self.src[s..self.pos - 1].iter().collect();
                return Token::BigInt(literal);
            }
            return Token::Number(digits.chars().fold(0.0, |value, digit| {
                value * f64::from(radix) + f64::from(digit.to_digit(radix).unwrap_or(0))
            }));
        }
        while self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_digit() || self.src[self.pos] == '_')
        {
            self.pos += 1;
            self.col += 1;
        }
        if self.pos < self.src.len() && self.src[self.pos] == '.' {
            self.pos += 1;
            self.col += 1;
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_digit() || self.src[self.pos] == '_')
            {
                self.pos += 1;
                self.col += 1;
            }
        }
        // Exponent part: e/E, optional sign, then digits (e.g. 1e3, 1.5e-2).
        if self.pos < self.src.len() && (self.src[self.pos] == 'e' || self.src[self.pos] == 'E') {
            let mut la = self.pos + 1;
            if la < self.src.len() && (self.src[la] == '+' || self.src[la] == '-') {
                la += 1;
            }
            if la < self.src.len() && self.src[la].is_ascii_digit() {
                self.pos = la;
                while self.pos < self.src.len()
                    && (self.src[self.pos].is_ascii_digit() || self.src[self.pos] == '_')
                {
                    self.pos += 1;
                    self.col += 1;
                }
            }
        }
        let legacy = self.src[s] == '0' && self.src.get(s + 1).is_some_and(char::is_ascii_digit);
        let legacy_octal = legacy
            && self.src[s..self.pos]
                .iter()
                .filter(|c| c.is_ascii_digit())
                .all(|c| *c <= '7');
        if self.src[s] == '0' && self.src.get(s + 1) == Some(&'_')
            || legacy && self.src[s..self.pos].contains(&'_')
            || legacy_octal
                && self.src[s..self.pos]
                    .iter()
                    .any(|c| matches!(c, '.' | 'e' | 'E'))
        {
            return Token::Unknown('0');
        }
        if self.src[s..self.pos]
            .iter()
            .enumerate()
            .any(|(index, character)| {
                *character == '_'
                    && (index == 0
                        || !self.src[s + index - 1].is_ascii_digit()
                        || !self
                            .src
                            .get(s + index + 1)
                            .is_some_and(char::is_ascii_digit))
            })
        {
            return Token::Unknown('_');
        }
        // A trailing `n` makes the literal a BigInt.
        if self.pos < self.src.len() && self.src[self.pos] == 'n' {
            let digits: String = self.src[s..self.pos]
                .iter()
                .filter(|c| **c != '_')
                .collect();
            self.pos += 1;
            self.col += 1;
            if digits.contains(['.', 'e', 'E']) || legacy {
                return Token::Unknown('n');
            }
            return Token::BigInt(digits);
        }
        let n: String = self.src[s..self.pos]
            .iter()
            .filter(|c| **c != '_')
            .collect();
        let value = if legacy_octal {
            n.chars().fold(0.0, |value, digit| {
                value * 8.0 + f64::from(digit.to_digit(8).unwrap_or(0))
            })
        } else {
            n.parse().unwrap_or(0.0)
        };
        if legacy {
            Token::LegacyNumber(value)
        } else {
            Token::Number(value)
        }
    }

    fn identifier_escape(&mut self) -> Result<char, ()> {
        self.pos += 1;
        self.col += 1;
        if self.src.get(self.pos) != Some(&'u') {
            return Err(());
        }
        self.pos += 1;
        self.col += 1;
        let braced = self.src.get(self.pos) == Some(&'{');
        if braced {
            self.pos += 1;
            self.col += 1;
        }
        let mut value = 0u32;
        let mut digits = 0;
        while let Some(&c) = self.src.get(self.pos) {
            if braced && c == '}' || !braced && digits == 4 {
                break;
            }
            let digit = c.to_digit(16).ok_or(())?;
            value = value
                .checked_mul(16)
                .and_then(|v| v.checked_add(digit))
                .ok_or(())?;
            self.pos += 1;
            self.col += 1;
            digits += 1;
        }
        if digits == 0 || !braced && digits != 4 {
            return Err(());
        }
        if braced {
            if self.src.get(self.pos) != Some(&'}') {
                return Err(());
            }
            self.pos += 1;
            self.col += 1;
        }
        char::from_u32(value).ok_or(())
    }

    fn read_ident(&mut self) -> Token {
        let mut i = String::new();
        let mut escaped = false;
        while let Some(&c) = self.src.get(self.pos) {
            let character = if c == '\\' {
                escaped = true;
                match self.identifier_escape() {
                    Ok(character) => character,
                    Err(()) => return Token::Unknown('\\'),
                }
            } else {
                if !(if i.is_empty() {
                    is_identifier_start(c)
                } else {
                    is_identifier_continue(c)
                }) {
                    break;
                }
                self.pos += 1;
                self.col += 1;
                c
            };
            if !(if i.is_empty() {
                is_identifier_start(character)
            } else {
                is_identifier_continue(character)
            }) {
                return Token::Unknown('\\');
            }
            i.push(character);
        }
        if escaped {
            return Token::EscapedIdentifier(i);
        }
        match i.as_str() {
            "var" => Token::KwVar,
            "let" => Token::KwLet,
            "const" => Token::KwConst,
            "function" => Token::KwFunction,
            "return" => Token::KwReturn,
            "if" => Token::KwIf,
            "else" => Token::KwElse,
            "for" => Token::KwFor,
            "while" => Token::KwWhile,
            "with" => Token::KwWith,
            "do" => Token::KwDo,
            "switch" => Token::KwSwitch,
            "case" => Token::KwCase,
            "default" => Token::KwDefault,
            "break" => Token::KwBreak,
            "continue" => Token::KwContinue,
            "class" => Token::KwClass,
            "extends" => Token::KwExtends,
            "new" => Token::KwNew,
            "this" => Token::KwThis,
            "super" => Token::KwSuper,
            "import" => Token::KwImport,
            "export" => Token::KwExport,
            "from" => Token::KwFrom,
            "as" => Token::KwAs,
            "async" => Token::KwAsync,
            "await" => Token::KwAwait,
            "yield" => Token::KwYield,
            "try" => Token::KwTry,
            "catch" => Token::KwCatch,
            "finally" => Token::KwFinally,
            "throw" => Token::KwThrow,
            "typeof" => Token::KwTypeof,
            "instanceof" => Token::KwInstanceof,
            "in" => Token::KwIn,
            "of" => Token::KwOf,
            "true" => Token::KwTrue,
            "false" => Token::KwFalse,
            "null" => Token::KwNull,
            "undefined" => Token::KwUndefined,
            "delete" => Token::KwDelete,
            "void" => Token::KwVoid,
            "debugger" => Token::KwDebugger,
            "enum" => Token::KwEnum,
            "static" => Token::KwStatic,
            "get" => Token::KwGet,
            "set" => Token::KwSet,
            "constructor" => Token::KwConstructor,
            _ => Token::Identifier(i),
        }
    }
}

pub(crate) fn is_identifier_start(character: char) -> bool {
    matches!(character, '$' | '_') || unicode_id_start::is_id_start(character)
}

pub(crate) fn is_identifier_continue(character: char) -> bool {
    matches!(character, '$' | '_' | '\u{200c}' | '\u{200d}')
        || unicode_id_start::is_id_continue(character)
}

/// Whether `name` may be used as a binding identifier in guest source --
/// a `function` name, a `let`/`const` binding, an export name.
///
/// Reserved words are rejected, while ECMAScript contextual words such as
/// `async`, `from`, `get` and `set` remain valid bindings even though the
/// lexer keeps dedicated tokens for their grammar roles.
pub fn is_binding_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let toks = Lexer::new(name).tokenize();
    // `tokenize` always appends `Token::EOF`, so a lone identifier is 2 tokens.
    if toks.len() != 2 || toks[1] != Token::EOF {
        return false;
    }
    matches!(&toks[0], Token::Identifier(ident) if ident == name)
        || matches!(
            &toks[0],
            Token::KwAs
                | Token::KwAsync
                | Token::KwConstructor
                | Token::KwFrom
                | Token::KwGet
                | Token::KwOf
                | Token::KwSet
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reserved words must be rejected as bindings. Contextual words are
    /// listed separately because ECMAScript permits them as names.
    const ALL_KEYWORDS: &[&str] = &[
        "var",
        "let",
        "const",
        "function",
        "return",
        "if",
        "else",
        "for",
        "while",
        "do",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "class",
        "extends",
        "new",
        "this",
        "super",
        "import",
        "export",
        "await",
        "yield",
        "try",
        "catch",
        "finally",
        "throw",
        "typeof",
        "instanceof",
        "in",
        "true",
        "false",
        "null",
        "undefined",
        "delete",
        "void",
        "static",
        "debugger",
        "enum",
    ];

    const CONTEXTUAL_IDENTIFIERS: &[&str] =
        &["as", "async", "constructor", "from", "get", "of", "set"];

    #[test]
    fn no_keyword_is_a_binding_identifier() {
        for kw in ALL_KEYWORDS {
            assert!(
                !is_binding_identifier(kw),
                "keyword `{kw}` was accepted as a binding identifier"
            );
        }
    }

    #[test]
    fn reserved_word_list_is_exhaustive() {
        // Every reserved word listed above must lex as a non-Identifier.
        for kw in ALL_KEYWORDS {
            let toks = Lexer::new(kw).tokenize();
            assert!(
                !matches!(&toks[0], Token::Identifier(_)),
                "`{kw}` is listed as a keyword but lexes as an identifier"
            );
        }
    }

    #[test]
    fn contextual_words_remain_valid_bindings() {
        for name in CONTEXTUAL_IDENTIFIERS {
            assert!(is_binding_identifier(name), "`{name}` should be accepted");
        }
    }

    #[test]
    fn ordinary_names_are_binding_identifiers() {
        for name in [
            "café",
            "你好",
            "𝒜",
            "℘",
            "a\u{200c}b",
            "read",
            "write",
            "_private",
            "$dollar",
            "camelCase",
            "with_2_digits",
            "A",
            "_",
            "$",
            // Keyword-adjacent but not keywords.
            "nullish",
            "asyncFn",
            "getter",
            "classy",
            "get",
            "set",
            "async",
            "from",
            "as",
            "of",
            "constructor",
        ] {
            assert!(is_binding_identifier(name), "`{name}` should be accepted");
        }
    }

    #[test]
    fn non_identifiers_are_rejected() {
        for name in [
            "",
            "1abc",
            "a b",
            "a-b",
            "a.b",
            "a()",
            " a",
            "a ",
            "//x",
            "a;b",
            "\"quoted\"",
        ] {
            assert!(!is_binding_identifier(name), "`{name}` should be rejected");
        }
    }

    #[test]
    fn test_numbers() {
        let mut lex = Lexer::new("42 3.15 1_000");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::Number(42.0));
        assert_eq!(toks[1], Token::Number(3.15));
        assert_eq!(toks[2], Token::Number(1000.0));
    }

    #[test]
    fn test_strings() {
        let mut lex = Lexer::new(r#""hello" 'world'"#);
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::String("hello".into()));
        assert_eq!(toks[1], Token::String("world".into()));
    }

    #[test]
    fn test_string_escapes() {
        let mut lex = Lexer::new(r#""hello\nworld""#);
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::EscapedString("hello\nworld".into()));
    }

    #[test]
    fn test_operators() {
        let mut lex = Lexer::new("+ - * / % ++ -- == != === !== < > <= >=");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::Plus);
        assert_eq!(toks[1], Token::Minus);
        assert_eq!(toks[2], Token::Star);
        assert_eq!(toks[3], Token::Slash);
        assert_eq!(toks[4], Token::Percent);
        assert_eq!(toks[5], Token::PlusPlus);
        assert_eq!(toks[6], Token::MinusMinus);
        assert_eq!(toks[7], Token::EqualEqual);
        assert_eq!(toks[8], Token::NotEqual);
        assert_eq!(toks[9], Token::EqualEqualEqual);
        assert_eq!(toks[10], Token::NotEqualEqual);
        assert_eq!(toks[11], Token::Less);
        assert_eq!(toks[12], Token::Greater);
        assert_eq!(toks[13], Token::LessEqual);
        assert_eq!(toks[14], Token::GreaterEqual);
    }

    #[test]
    fn test_keywords() {
        let mut lex = Lexer::new("var let const function return if else for while");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::KwVar);
        assert_eq!(toks[1], Token::KwLet);
        assert_eq!(toks[2], Token::KwConst);
        assert_eq!(toks[3], Token::KwFunction);
        assert_eq!(toks[4], Token::KwReturn);
        assert_eq!(toks[5], Token::KwIf);
        assert_eq!(toks[6], Token::KwElse);
        assert_eq!(toks[7], Token::KwFor);
        assert_eq!(toks[8], Token::KwWhile);
    }

    #[test]
    fn test_identifiers() {
        let mut lex = Lexer::new("foo _bar $baz myVar");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::Identifier("foo".to_string()));
        assert_eq!(toks[1], Token::Identifier("_bar".to_string()));
        assert_eq!(toks[2], Token::Identifier("$baz".to_string()));
        assert_eq!(toks[3], Token::Identifier("myVar".to_string()));
    }

    #[test]
    fn test_comments() {
        let mut lex = Lexer::new("1 // comment\n2 /* block */ 3");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::Number(1.0));
        assert_eq!(toks[1], Token::Number(2.0));
        assert_eq!(toks[2], Token::Number(3.0));
    }

    #[test]
    fn test_arrow() {
        let mut lex = Lexer::new("=>");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::Arrow);
    }

    #[test]
    fn test_spread() {
        let mut lex = Lexer::new("...");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::DotDotDot);
    }

    #[test]
    fn test_compound_assignment() {
        let mut lex = Lexer::new("+= -= *= /=");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::PlusEqual);
        assert_eq!(toks[1], Token::MinusEqual);
        assert_eq!(toks[2], Token::StarEqual);
        assert_eq!(toks[3], Token::SlashEqual);
    }

    #[test]
    fn test_punctuation() {
        let mut lex = Lexer::new("( ) { } [ ] ; , . : ?");
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::LParen);
        assert_eq!(toks[1], Token::RParen);
        assert_eq!(toks[2], Token::LBrace);
        assert_eq!(toks[3], Token::RBrace);
        assert_eq!(toks[4], Token::LBracket);
        assert_eq!(toks[5], Token::RBracket);
        assert_eq!(toks[6], Token::Semicolon);
        assert_eq!(toks[7], Token::Comma);
        assert_eq!(toks[8], Token::Dot);
        assert_eq!(toks[9], Token::Colon);
        assert_eq!(toks[10], Token::Question);
    }
}
