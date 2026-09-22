//! Pattern parser: JavaScript regular-expression syntax to a syntax tree.

use std::collections::HashMap;

/// One entry inside a character class.
#[derive(Debug, Clone)]
pub enum ClassItem {
    Char(char),
    Range(char, char),
    /// A shorthand class (`\d`, `\W`, …). `negated` distinguishes `\d` from
    /// `\D` so the pair can share one representation.
    Shorthand {
        kind: Shorthand,
        negated: bool,
    },
    /// A Unicode property escape, compiled to sorted scalar-value ranges.
    UnicodeProperty {
        ranges: Vec<(char, char)>,
        negated: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shorthand {
    Digit,
    Word,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    End,
    WordBoundary,
    NotWordBoundary,
}

#[derive(Debug, Clone)]
pub enum Node {
    /// Matches the empty string. Produced by `(?:)` and by an empty
    /// alternative such as the right side of `a|`.
    Empty,
    Char(char),
    /// `.`: any character, or any character except a line terminator when the
    /// `s` flag is absent.
    AnyChar,
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
    /// A group. `index` is `Some` for a capturing group, `None` for `(?:…)`.
    Group {
        index: Option<usize>,
        node: Box<Node>,
    },
    Backref(usize),
    Anchor(Anchor),
    /// `(?=…)`, `(?!…)`, `(?<=…)`, `(?<!…)`. A lookaround consumes nothing;
    /// it only asserts that its body does (or does not) match at this point.
    Look {
        ahead: bool,
        negative: bool,
        node: Box<Node>,
    },
}

pub struct Parsed {
    pub root: Node,
    /// Number of capturing groups, so a match can size its capture list.
    pub group_count: usize,
    /// Names from `(?<name>…)`, mapped to their group index.
    pub names: HashMap<String, usize>,
}

pub struct Parser {
    chars: Vec<char>,
    pos: usize,
    group_count: usize,
    names: HashMap<String, usize>,
    /// `u` flag: makes otherwise-tolerated escapes an error, as in a real
    /// engine's Unicode mode.
    unicode: bool,
}

pub type ParseResult<T> = Result<T, String>;

impl Parser {
    pub fn new(pattern: &str, unicode: bool) -> Self {
        Self {
            chars: pattern.chars().collect(),
            pos: 0,
            group_count: 0,
            names: HashMap::new(),
            unicode,
        }
    }

    pub fn parse(mut self) -> ParseResult<Parsed> {
        let root = self.alternation()?;
        if self.pos < self.chars.len() {
            return Err(format!(
                "Unmatched ')' at position {} in regular expression",
                self.pos
            ));
        }
        Ok(Parsed {
            root,
            group_count: self.group_count,
            names: self.names,
        })
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn alternation(&mut self) -> ParseResult<Node> {
        let mut branches = vec![self.sequence()?];
        while self.eat('|') {
            branches.push(self.sequence()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().expect("just checked")
        } else {
            Node::Alt(branches)
        })
    }

    fn sequence(&mut self) -> ParseResult<Node> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantifier(atom)?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().expect("just checked"),
            _ => Node::Concat(items),
        })
    }

    /// Apply a trailing `*`, `+`, `?` or `{…}` to `node`, if one is there.
    fn quantifier(&mut self, node: Node) -> ParseResult<Node> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => match self.try_bounds()? {
                Some(bounds) => bounds,
                // `{` that is not a valid bound is a literal brace, which the
                // atom parser already consumed as a character.
                None => return Ok(node),
            },
            _ => return Ok(node),
        };
        if let Some(max) = max
            && max < min
        {
            return Err("numbers out of order in {} quantifier".to_string());
        }
        // A quantifier may not apply to an anchor or a lookaround.
        if matches!(node, Node::Anchor(_)) {
            return Err("Nothing to repeat".to_string());
        }
        let greedy = !self.eat('?');
        Ok(Node::Repeat {
            node: Box::new(node),
            min,
            max,
            greedy,
        })
    }

    /// Parse `{n}`, `{n,}` or `{n,m}`. Returns `None` (without consuming) when
    /// the braces do not form a quantifier.
    fn try_bounds(&mut self) -> ParseResult<Option<(u32, Option<u32>)>> {
        let save = self.pos;
        self.pos += 1; // `{`
        let min = self.digits();
        let Some(min) = min else {
            self.pos = save;
            return Ok(None);
        };
        let max = if self.eat(',') {
            self.digits()
        } else {
            Some(min)
        };
        if !self.eat('}') {
            self.pos = save;
            return Ok(None);
        }
        Ok(Some((min, max)))
    }

    fn digits(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(c) = self.peek()
            && c.is_ascii_digit()
        {
            value = value
                .saturating_mul(10)
                .saturating_add(c as u32 - '0' as u32);
            self.pos += 1;
        }
        (self.pos > start).then_some(value)
    }

    fn atom(&mut self) -> ParseResult<Node> {
        let Some(c) = self.peek() else {
            return Ok(Node::Empty);
        };
        match c {
            '^' => {
                self.pos += 1;
                Ok(Node::Anchor(Anchor::Start))
            }
            '$' => {
                self.pos += 1;
                Ok(Node::Anchor(Anchor::End))
            }
            '.' => {
                self.pos += 1;
                Ok(Node::AnyChar)
            }
            '(' => self.group(),
            '[' => self.class(),
            '\\' => self.escape(),
            '*' | '+' | '?' => Err("Nothing to repeat".to_string()),
            _ => {
                self.pos += 1;
                Ok(Node::Char(c))
            }
        }
    }

    fn group(&mut self) -> ParseResult<Node> {
        self.pos += 1; // `(`
        // `(?…`: a non-capturing group, a lookaround, or a named capture.
        if self.eat('?') {
            if self.eat(':') {
                let node = self.alternation()?;
                self.expect(')')?;
                return Ok(Node::Group {
                    index: None,
                    node: Box::new(node),
                });
            }
            if self.eat('=') {
                return self.lookaround(true, false);
            }
            if self.eat('!') {
                return self.lookaround(true, true);
            }
            if self.eat('<') {
                if self.eat('=') {
                    return self.lookaround(false, false);
                }
                if self.eat('!') {
                    return self.lookaround(false, true);
                }
                // `(?<name>…)`
                let name = self.group_name()?;
                self.group_count += 1;
                let index = self.group_count;
                if self.names.insert(name.clone(), index).is_some() {
                    return Err(format!("Duplicate capture group name '{}'", name));
                }
                let node = self.alternation()?;
                self.expect(')')?;
                return Ok(Node::Group {
                    index: Some(index),
                    node: Box::new(node),
                });
            }
            return Err("Invalid group".to_string());
        }
        self.group_count += 1;
        let index = self.group_count;
        let node = self.alternation()?;
        self.expect(')')?;
        Ok(Node::Group {
            index: Some(index),
            node: Box::new(node),
        })
    }

    fn lookaround(&mut self, ahead: bool, negative: bool) -> ParseResult<Node> {
        let node = self.alternation()?;
        self.expect(')')?;
        Ok(Node::Look {
            ahead,
            negative,
            node: Box::new(node),
        })
    }

    fn group_name(&mut self) -> ParseResult<String> {
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c == '>' {
                self.pos += 1;
                if name.is_empty() {
                    return Err("Invalid capture group name".to_string());
                }
                return Ok(name);
            }
            name.push(c);
            self.pos += 1;
        }
        Err("Invalid capture group name".to_string())
    }

    fn expect(&mut self, c: char) -> ParseResult<()> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("Expected '{}' in regular expression", c))
        }
    }

    fn class(&mut self) -> ParseResult<Node> {
        self.pos += 1; // `[`
        let negated = self.eat('^');
        let mut items = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err("Unterminated character class".to_string());
            };
            if c == ']' {
                self.pos += 1;
                break;
            }
            let low = self.class_atom()?;
            // A `-` before `]` is a literal, not a range.
            if self.peek() == Some('-') && self.chars.get(self.pos + 1) != Some(&']') {
                self.pos += 1;
                let high = self.class_atom()?;
                match (low, high) {
                    (ClassItem::Char(a), ClassItem::Char(b)) => {
                        if a > b {
                            return Err("Range out of order in character class".to_string());
                        }
                        items.push(ClassItem::Range(a, b));
                    }
                    // `[\d-z]` is not a range; the parts stand alone with a
                    // literal hyphen between them.
                    (low, high) => {
                        items.push(low);
                        items.push(ClassItem::Char('-'));
                        items.push(high);
                    }
                }
            } else {
                items.push(low);
            }
        }
        Ok(Node::Class { negated, items })
    }

    fn class_atom(&mut self) -> ParseResult<ClassItem> {
        let Some(c) = self.peek() else {
            return Err("Unterminated character class".to_string());
        };
        if c != '\\' {
            self.pos += 1;
            return Ok(ClassItem::Char(c));
        }
        self.pos += 1; // `\`
        let Some(e) = self.peek() else {
            return Err("Trailing backslash in regular expression".to_string());
        };
        self.pos += 1;
        if self.unicode && matches!(e, 'p' | 'P') {
            return self.unicode_property(e == 'P');
        }
        Ok(match e {
            'd' => ClassItem::Shorthand {
                kind: Shorthand::Digit,
                negated: false,
            },
            'D' => ClassItem::Shorthand {
                kind: Shorthand::Digit,
                negated: true,
            },
            'w' => ClassItem::Shorthand {
                kind: Shorthand::Word,
                negated: false,
            },
            'W' => ClassItem::Shorthand {
                kind: Shorthand::Word,
                negated: true,
            },
            's' => ClassItem::Shorthand {
                kind: Shorthand::Space,
                negated: false,
            },
            'S' => ClassItem::Shorthand {
                kind: Shorthand::Space,
                negated: true,
            },
            // `\b` inside a class is a backspace, not a word boundary.
            'b' => ClassItem::Char('\u{0008}'),
            other => ClassItem::Char(self.escaped_char(other)?),
        })
    }

    fn escape(&mut self) -> ParseResult<Node> {
        self.pos += 1; // `\`
        let Some(e) = self.peek() else {
            return Err("Trailing backslash in regular expression".to_string());
        };
        self.pos += 1;
        Ok(match e {
            'p' | 'P' if self.unicode => Node::Class {
                negated: false,
                items: vec![self.unicode_property(e == 'P')?],
            },
            'd' | 'D' | 'w' | 'W' | 's' | 'S' => {
                let (kind, negated) = match e {
                    'd' => (Shorthand::Digit, false),
                    'D' => (Shorthand::Digit, true),
                    'w' => (Shorthand::Word, false),
                    'W' => (Shorthand::Word, true),
                    's' => (Shorthand::Space, false),
                    _ => (Shorthand::Space, true),
                };
                Node::Class {
                    negated: false,
                    items: vec![ClassItem::Shorthand { kind, negated }],
                }
            }
            'b' => Node::Anchor(Anchor::WordBoundary),
            'B' => Node::Anchor(Anchor::NotWordBoundary),
            // `\k<name>`: a named backreference.
            'k' => {
                if !self.eat('<') {
                    return Err("Invalid named reference".to_string());
                }
                let name = self.group_name()?;
                let index = *self
                    .names
                    .get(&name)
                    .ok_or_else(|| format!("Invalid named capture referenced: {}", name))?;
                Node::Backref(index)
            }
            c if c.is_ascii_digit() && c != '0' => {
                self.pos -= 1;
                let index = self.digits().unwrap_or(0) as usize;
                Node::Backref(index)
            }
            other => Node::Char(self.escaped_char(other)?),
        })
    }

    /// Parse `\p{Property}` / `\P{Property}` using regex-syntax's Unicode
    /// property tables, then keep only the compact character ranges needed by
    /// the VM's backtracking matcher.
    fn unicode_property(&mut self, negated: bool) -> ParseResult<ClassItem> {
        if !self.eat('{') {
            return Err("Invalid Unicode property escape".to_string());
        }
        let start = self.pos;
        while let Some(c) = self.peek()
            && c != '}'
        {
            self.pos += 1;
        }
        if !self.eat('}') || self.pos - start <= 1 {
            return Err("Invalid Unicode property escape".to_string());
        }
        let property: String = self.chars[start..self.pos - 1].iter().collect();
        let pattern = format!(r"\p{{{property}}}");
        let hir = regex_syntax::Parser::new()
            .parse(&pattern)
            .map_err(|error| format!("Invalid Unicode property escape: {error}"))?;
        let regex_syntax::hir::HirKind::Class(regex_syntax::hir::Class::Unicode(class)) =
            hir.kind()
        else {
            return Err("Invalid Unicode property escape".to_string());
        };
        let ranges = class
            .ranges()
            .iter()
            .map(|range| (range.start(), range.end()))
            .collect();
        Ok(ClassItem::UnicodeProperty { ranges, negated })
    }

    /// Resolve a single-character escape: the control abbreviations, the
    /// numeric forms, and — outside Unicode mode — an escaped literal.
    fn escaped_char(&mut self, e: char) -> ParseResult<char> {
        Ok(match e {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'f' => '\u{000C}',
            'v' => '\u{000B}',
            '0' => '\0',
            'x' => self.code_point(2)?,
            'u' => {
                // `\u{1F600}` in Unicode mode, `￿` otherwise.
                if self.eat('{') {
                    let mut value: u32 = 0;
                    while let Some(c) = self.peek()
                        && c != '}'
                    {
                        value = value
                            .saturating_mul(16)
                            .saturating_add(c.to_digit(16).ok_or("Invalid Unicode escape")?);
                        self.pos += 1;
                    }
                    self.expect('}')?;
                    char::from_u32(value).ok_or("Invalid Unicode escape")?
                } else {
                    self.code_point(4)?
                }
            }
            'c' => {
                // `\cA` … `\cZ`: a control character.
                let Some(letter) = self.peek() else {
                    return Ok('\\');
                };
                self.pos += 1;
                char::from_u32((letter as u32) % 32).ok_or("Invalid control escape")?
            }
            other => {
                if self.unicode && other.is_ascii_alphanumeric() {
                    return Err(format!("Invalid escape '\\{}'", other));
                }
                other
            }
        })
    }

    fn code_point(&mut self, digits: usize) -> ParseResult<char> {
        let mut value: u32 = 0;
        for _ in 0..digits {
            let c = self.peek().ok_or("Invalid escape")?;
            value = value
                .saturating_mul(16)
                .saturating_add(c.to_digit(16).ok_or("Invalid escape")?);
            self.pos += 1;
        }
        char::from_u32(value).ok_or_else(|| "Invalid escape".to_string())
    }
}
