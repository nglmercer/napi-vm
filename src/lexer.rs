/// One literal chunk of a template literal, in both forms: `cooked` has
/// escape sequences resolved, `raw` is the source text verbatim. Tagged
/// templates expose both; an ordinary template literal uses only `cooked`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateChunk {
    pub cooked: String,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Number(f64),
    /// A `BigInt` literal, carrying its digits (`123n` → `"123"`).
    BigInt(String),
    /// A character that begins no valid token, e.g. `@` or `#`.
    ///
    /// Carried through as a token rather than skipped so the parser can report
    /// a `SyntaxError` pointing at it. Dropping it silently let `x = #foo`
    /// parse as `x = foo` and run.
    Unknown(char),
    String(String),
    Identifier(String),
    Plus,
    Minus,
    Star,
    Slash,
    /// A regular-expression literal: its source and its flags.
    Regex(String, String),
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

pub struct Lexer {
    src: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    /// Tokens produced ahead of time (e.g. by template scanning), drained in
    /// FIFO order before lexing more source.
    pending: Vec<Token>,
    /// Spans corresponding to tokens in `pending`.
    pending_spans: Vec<crate::span::Span>,
    /// The last token emitted, which decides whether a `/` starts a regular
    /// expression or is a division operator. The two are indistinguishable
    /// from the character alone, so the grammar resolves it by context: after
    /// something that *ends a value* it is division, and otherwise it begins a
    /// literal.
    previous: Option<Token>,
}

impl Lexer {
    pub fn new(s: &str) -> Self {
        Self {
            src: s.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            pending: Vec::new(),
            pending_spans: Vec::new(),
            previous: None,
        }
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
                continue;
            }
            self.skip_ws();
            if self.pos >= self.src.len() {
                break;
            }
            if let Some((t, span)) = self.next_with_span() {
                self.previous = Some(t.clone());
                toks.push((t, span));
            }
        }
        let eof_span = crate::span::Span::new(self.line, self.col);
        toks.push((Token::EOF, eof_span));
        toks
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == '\n' {
                self.pos += 1;
                self.line += 1;
                self.col = 1;
            } else if c == '\r' {
                self.pos += 1;
                self.line += 1;
                self.col = 1;
                if self.pos < self.src.len() && self.src[self.pos] == '\n' {
                    self.pos += 1;
                }
            } else if c == '\t' || c == ' ' {
                self.pos += 1;
                self.col += 1;
            } else if c == '/' && self.pos + 1 < self.src.len() {
                let n = self.src[self.pos + 1];
                if n == '/' {
                    self.pos += 2;
                    self.col += 2;
                    while self.pos < self.src.len() && self.src[self.pos] != '\n' {
                        self.pos += 1;
                        self.col += 1;
                    }
                } else if n == '*' {
                    self.pos += 2;
                    self.col += 2;
                    while self.pos + 1 < self.src.len() {
                        if self.src[self.pos] == '*' && self.src[self.pos + 1] == '/' {
                            self.pos += 2;
                            self.col += 2;
                            break;
                        }
                        if self.src[self.pos] == '\n' {
                            self.line += 1;
                            self.col = 1;
                        } else {
                            self.col += 1;
                        }
                        self.pos += 1;
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
    /// raw text of each quasi (including whitespace). Emits:
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
                self.pos += 1;
                self.col += 1;
                let e = self.src[self.pos];
                quasi.raw.push('\\');
                quasi.raw.push(e);
                quasi.cooked.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '0' => '\0',
                    other => other,
                });
                self.pos += 1;
                self.col += 1;
            } else {
                quasi.cooked.push(c);
                quasi.raw.push(c);
                self.pos += 1;
                if c == '\n' {
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
        toks.push((Token::Backtick, span));
        toks
    }

    /// Lex the expression inside a `${ ... }` interpolation, tracking brace depth
    /// so nested object literals and templates terminate correctly. Consumes the
    /// matching closing brace and appends `RBrace`.
    fn lex_interp(&mut self, toks: &mut Vec<crate::span::SpannedToken>) {
        let mut depth = 1i32;
        while self.pos < self.src.len() && depth > 0 {
            self.skip_ws();
            if self.pos >= self.src.len() {
                break;
            }
            let c = self.src[self.pos];
            match c {
                '{' => {
                    depth += 1;
                    let span = crate::span::Span::new(self.line, self.col);
                    toks.push((Token::LBrace, span));
                    self.pos += 1;
                    self.col += 1;
                }
                '}' => {
                    depth -= 1;
                    self.pos += 1;
                    self.col += 1;
                    let span = crate::span::Span::new(self.line, self.col - 1);
                    toks.push((Token::RBrace, span));
                    if depth == 0 {
                        return;
                    }
                }
                '`' => {
                    let nested = self.read_template();
                    toks.extend(nested);
                }
                _ => {
                    if let Some((t, span)) = self.next_with_span() {
                        toks.push((t, span));
                    }
                }
            }
        }
    }

    fn next_with_span(&mut self) -> Option<crate::span::SpannedToken> {
        let span = crate::span::Span::new(self.line, self.col);
        let t = self.next()?;
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
                Some('.') => {
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
                Token::Hash
            }
            '"' | '\'' => self.read_str(c),
            c if c.is_ascii_digit() => self.read_num(),
            c if c.is_ascii_alphabetic() || c == '_' || c == '$' => self.read_ident(),
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
    fn regex_allowed(&self) -> bool {
        match &self.previous {
            None => true,
            Some(token) => !matches!(
                token,
                Token::Identifier(_)
                    | Token::Number(_)
                    | Token::String(_)
                    | Token::Regex(_, _)
                    | Token::RParen
                    | Token::RBracket
                    | Token::RBrace
                    | Token::PlusPlus
                    | Token::MinusMinus
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
                '\n' => {
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
        Some(Token::Regex(pattern, flags))
    }

    fn read_str(&mut self, q: char) -> Token {
        self.pos += 1;
        self.col += 1;
        let mut s = String::new();
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == q {
                self.pos += 1;
                self.col += 1;
                break;
            }
            if c == '\\' && self.pos + 1 < self.src.len() {
                self.pos += 1;
                self.col += 1;
                match self.src[self.pos] {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    '\\' => s.push('\\'),
                    '"' => s.push('"'),
                    '\'' => s.push('\''),
                    '0' => s.push('\0'),
                    o => s.push(o),
                }
            } else {
                s.push(c);
            }
            self.pos += 1;
            if c == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
        Token::String(s)
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
            return Token::Number(u128::from_str_radix(&digits, radix).unwrap_or(0) as f64);
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
                while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                    self.pos += 1;
                    self.col += 1;
                }
            }
        }
        // A trailing `n` makes the literal a BigInt.
        if self.pos < self.src.len() && self.src[self.pos] == 'n' {
            let digits: String = self.src[s..self.pos]
                .iter()
                .filter(|c| **c != '_')
                .collect();
            self.pos += 1;
            self.col += 1;
            return Token::BigInt(digits);
        }
        let n: String = self.src[s..self.pos]
            .iter()
            .filter(|c| **c != '_')
            .collect();
        Token::Number(n.parse().unwrap_or(0.0))
    }

    fn read_ident(&mut self) -> Token {
        let s = self.pos;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if !c.is_ascii_alphanumeric() && c != '_' && c != '$' {
                break;
            }
            self.pos += 1;
            self.col += 1;
        }
        let i: String = self.src[s..self.pos].iter().collect();
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
            "static" => Token::KwStatic,
            "get" => Token::KwGet,
            "set" => Token::KwSet,
            "constructor" => Token::KwConstructor,
            _ => Token::Identifier(i),
        }
    }
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
            "café",
            "\u{4f60}\u{597d}",
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
        assert_eq!(toks[0], Token::String("hello".to_string()));
        assert_eq!(toks[1], Token::String("world".to_string()));
    }

    #[test]
    fn test_string_escapes() {
        let mut lex = Lexer::new(r#""hello\nworld""#);
        let toks = lex.tokenize();
        assert_eq!(toks[0], Token::String("hello\nworld".to_string()));
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
