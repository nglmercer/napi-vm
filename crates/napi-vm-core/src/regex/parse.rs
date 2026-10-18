//! Pattern parser: JavaScript regular-expression syntax to a syntax tree.

use std::collections::HashMap;

/// One entry inside a character class.
#[derive(Debug, Clone)]
pub enum ClassItem {
    Char(u32),
    Range(u32, u32),
    /// A shorthand class (`\d`, `\W`, …). `negated` distinguishes `\d` from
    /// `\D` so the pair can share one representation.
    Shorthand {
        kind: Shorthand,
        negated: bool,
    },
    /// A Unicode property escape, compiled to sorted scalar-value ranges.
    UnicodeProperty {
        surrogates: bool,
        ranges: Vec<(char, char)>,
        folded: Vec<(char, char)>,
        folded_complement: Vec<(char, char)>,
        negated: bool,
    },
    /// A nested `v`-mode class: matches when the inner union matches (or
    /// does not, when negated).
    Nested {
        negated: bool,
        items: Vec<ClassItem>,
    },
    /// `v`-mode string alternatives (`\q{…}`, properties of strings) as
    /// UTF-16 units. Matched before scalar items, in source order.
    Strings(StringAlts),
    /// `v`-mode `A&&B&&…`: every operand union must match.
    Conjoin(Vec<Vec<ClassItem>>),
    /// `v`-mode `A--B`: the left union matches and the right one does not.
    Except {
        left: Vec<ClassItem>,
        right: Vec<ClassItem>,
    },
}

/// Ordered string alternatives with a first-unit index so long tables
/// (e.g. `RGI_Emoji` with ~4k entries) only attempt alternatives that
/// can start at the current input unit on the common no-fold path.
#[derive(Debug, Clone, Default)]
pub struct StringAlts {
    alts: Vec<Vec<u16>>,
    /// Single-unit alternatives by their unit.
    by_single: HashMap<u16, Vec<usize>>,
    /// Longer alternatives by their first two units. Astral emoji share a
    /// handful of lead surrogates, so one unit still matches hundreds of
    /// entries; two units narrow RGI candidates to roughly a dozen.
    by_pair: HashMap<(u16, u16), Vec<usize>>,
    has_empty: bool,
}

impl StringAlts {
    pub fn new(alts: Vec<Vec<u16>>) -> Self {
        let mut by_single: HashMap<u16, Vec<usize>> = HashMap::new();
        let mut by_pair: HashMap<(u16, u16), Vec<usize>> = HashMap::new();
        let mut has_empty = false;
        for (index, alternative) in alts.iter().enumerate() {
            match alternative.as_slice() {
                [] => has_empty = true,
                [only] => by_single.entry(*only).or_default().push(index),
                [first, second, ..] => by_pair.entry((*first, *second)).or_default().push(index),
            }
        }
        Self {
            alts,
            by_single,
            by_pair,
            has_empty,
        }
    }

    pub fn all(&self) -> &[Vec<u16>] {
        &self.alts
    }

    pub fn has_empty(&self) -> bool {
        self.has_empty
    }

    /// Whether a single-unit alternative equals `unit`.
    pub fn has_single(&self, unit: u16) -> bool {
        self.by_single.contains_key(&unit)
    }

    /// Single-unit alternative indices starting with `unit`, in source
    /// order. Used to merge singles with longer matches without
    /// disturbing alternative order.
    pub fn singles(&self, unit: u16) -> Option<&[usize]> {
        self.by_single.get(&unit).map(Vec::as_slice)
    }

    /// Candidate longer alternatives whose first two units equal
    /// (`first`, `second`), or `None` when none can start with them.
    pub fn candidates(&self, first: u16, second: u16) -> Option<&[usize]> {
        self.by_pair.get(&(first, second)).map(Vec::as_slice)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shorthand {
    Digit,
    Word,
    Space,
}

/// `v`-mode operators. A chain uses one kind; mixing `&&` and `--`
/// is an error, as V8 enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetOperator {
    Intersect,
    Difference,
}

/// One `v`-mode operand: a single character, escape, or nested class,
/// or a `X-Y` range. Ranges cannot neighbor `&&` or `--`.
#[derive(Debug, Clone)]
enum SetOperand {
    Unit(ClassItem),
    Range(u32, u32),
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
    Char(u32),
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
    Modifiers {
        add: u8,
        remove: u8,
        node: Box<Node>,
    },
    Backref(usize),
    NamedBackref(String),
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
    pub names: HashMap<String, Vec<usize>>,
}

pub struct Parser {
    chars: Vec<char>,
    pos: usize,
    group_count: usize,
    names: HashMap<String, Vec<usize>>,
    /// `u` flag: makes otherwise-tolerated escapes an error, as in a real
    /// engine's Unicode mode.
    unicode: bool,
    /// `v` flag: class-set expressions with nesting, `&&` and `--`.
    vsets: bool,
    pub(super) has_named_captures: bool,
}

pub type ParseResult<T> = Result<T, String>;

impl Parser {
    pub fn new(pattern: &str, unicode: bool, vsets: bool) -> Self {
        let chars: Vec<char> = pattern.chars().collect();
        let mut class = false;
        let mut at = 0;
        let mut has_named_captures = false;
        while at < chars.len() {
            match chars[at] {
                '\\' => {
                    at += 2;
                    continue;
                }
                '[' => class = true,
                ']' => class = false,
                '(' if !class
                    && chars.get(at + 1) == Some(&'?')
                    && chars.get(at + 2) == Some(&'<')
                    && !matches!(chars.get(at + 3), Some('=' | '!')) =>
                {
                    has_named_captures = true
                }
                _ => {}
            }
            at += 1;
        }
        Self {
            chars,
            pos: 0,
            group_count: 0,
            names: HashMap::new(),
            unicode,
            vsets,
            has_named_captures,
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
                Ok(Node::Char(c as u32))
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
                // The standards syntax validator has already rejected names
                // that could participate together in one match.
                self.names.entry(name).or_default().push(index);
                let node = self.alternation()?;
                self.expect(')')?;
                return Ok(Node::Group {
                    index: Some(index),
                    node: Box::new(node),
                });
            }
            if matches!(self.peek(), Some('i' | 'm' | 's' | '-')) {
                let mut add = 0;
                let mut remove = 0;
                let mut subtract = false;
                while let Some(c) = self.peek() {
                    if c == '-' && !subtract {
                        subtract = true;
                        self.pos += 1;
                        continue;
                    }
                    let bit = match c {
                        'i' => 1,
                        'm' => 2,
                        's' => 4,
                        _ => break,
                    };
                    if (add | remove) & bit != 0 {
                        return Err("Duplicate regular expression modifier".into());
                    }
                    if subtract {
                        remove |= bit;
                    } else {
                        add |= bit;
                    }
                    self.pos += 1;
                }
                if add | remove == 0 {
                    return Err("Empty regular expression modifier".into());
                }
                self.expect(':')?;
                let node = self.alternation()?;
                self.expect(')')?;
                return Ok(Node::Modifiers {
                    add,
                    remove,
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

    pub(super) fn group_name(&mut self) -> ParseResult<String> {
        let mut units = Vec::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            if c == '>' {
                if units.is_empty() {
                    return Err("Invalid capture group name".into());
                }
                return String::from_utf16(&units).map_err(|_| "Invalid capture group name".into());
            }
            if c == '\\' {
                self.expect('u')?;
                let code = self.escaped_char('u')?;
                if code > 0xffff {
                    units.push(0xd800 + ((code - 0x10000) >> 10) as u16);
                    units.push(0xdc00 + ((code - 0x10000) & 0x3ff) as u16);
                } else {
                    units.push(code as u16);
                }
            } else {
                let mut pair = [0; 2];
                units.extend_from_slice(c.encode_utf16(&mut pair));
            }
        }
        Err("Invalid capture group name".into())
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
        if self.vsets {
            let items = self.class_set_expression()?;
            // A negated class cannot hold strings, at any depth.
            if negated && Self::set_contains_strings(&items) {
                return Err("Cannot negate strings in character class".to_string());
            }
            return Ok(Node::Class { negated, items });
        }
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
                        items.push(ClassItem::Char('-' as u32));
                        items.push(high);
                    }
                }
            } else {
                items.push(low);
            }
        }
        Ok(Node::Class { negated, items })
    }

    /// Parse a `v`-mode class body up to and including `]`, lowering set
    /// expressions to union items. `A&&B` becomes a conjunction,
    /// `A--B` a difference; juxtaposed unions and ranges may not neighbor
    /// an operator.
    fn class_set_expression(&mut self) -> ParseResult<Vec<ClassItem>> {
        let mut operands: Vec<SetOperand> = Vec::new();
        let mut operators: Vec<SetOperator> = Vec::new();
        let mut juxtaposed = false;
        let mut after_operator = false;
        loop {
            match self.peek() {
                None => return Err("Unterminated character class".to_string()),
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            // Two operands with no operator between them is juxtaposition,
            // which may not neighbor `&&` or `--`.
            if !operands.is_empty() && !after_operator {
                juxtaposed = true;
            }
            after_operator = false;
            operands.push(self.class_set_operand()?);
            if self.peek() == Some('&') && self.chars.get(self.pos + 1) == Some(&'&') {
                self.pos += 2;
                operators.push(SetOperator::Intersect);
                after_operator = true;
            } else if self.peek() == Some('-') && self.chars.get(self.pos + 1) == Some(&'-') {
                self.pos += 2;
                operators.push(SetOperator::Difference);
                after_operator = true;
            } else if self.peek() == Some('-') {
                self.class_set_range(&mut operands)?;
            }
        }
        if operators.is_empty() {
            return Ok(operands
                .into_iter()
                .map(|operand| match operand {
                    SetOperand::Unit(item) => item,
                    SetOperand::Range(low, high) => ClassItem::Range(low, high),
                })
                .collect());
        }
        if juxtaposed
            || operands.len() != operators.len() + 1
            || operands
                .iter()
                .any(|operand| matches!(operand, SetOperand::Range(..)))
            || operators.iter().any(|op| *op != operators[0])
        {
            return Err("Invalid class set operation".to_string());
        }
        let mut units = operands.into_iter().map(|operand| match operand {
            SetOperand::Unit(item) => vec![item],
            SetOperand::Range(..) => unreachable!("rejected above"),
        });
        if operators[0] == SetOperator::Intersect {
            return Ok(vec![ClassItem::Conjoin(units.collect())]);
        }
        let first = units.next().expect("operator has a left operand");
        Ok(vec![ClassItem::Except {
            left: first,
            right: units.flatten().collect(),
        }])
    }

    /// Parse one `v`-mode operand: a nested class, an escape, or a single
    /// character. Reserved punctuators must be escaped; a bare `-`, `&`
    /// starting `&&`, or `]` never starts an operand.
    fn class_set_operand(&mut self) -> ParseResult<SetOperand> {
        match self.peek() {
            Some('[') => {
                self.pos += 1;
                let negated = self.eat('^');
                let items = self.class_set_expression()?;
                if negated && Self::set_contains_strings(&items) {
                    return Err("Cannot negate strings in character class".to_string());
                }
                Ok(SetOperand::Unit(ClassItem::Nested { negated, items }))
            }
            Some('\\') => Ok(SetOperand::Unit(self.class_set_escape()?)),
            Some(c) if "(){}-/|".contains(c) => {
                Err("Invalid character in character class".to_string())
            }
            Some('-') | Some('&') => Err("Invalid class set operation".to_string()),
            Some(c) => {
                self.pos += 1;
                Ok(SetOperand::Unit(ClassItem::Char(c as u32)))
            }
            None => Err("Unterminated character class".to_string()),
        }
    }

    /// Parse the `-Y` half of a `v`-mode `X-Y` range after `X`. The end
    /// must be a single character with a value not below `X`.
    fn class_set_range(&mut self, operands: &mut Vec<SetOperand>) -> ParseResult<()> {
        let Some(SetOperand::Unit(ClassItem::Char(low))) = operands.pop() else {
            return Err("Invalid character in character class".to_string());
        };
        self.pos += 1; // `-`
        let end = match self.peek() {
            Some('\\') => self.class_set_escape()?,
            Some(c) if !"(){}-/|]^".contains(c) => {
                self.pos += 1;
                ClassItem::Char(c as u32)
            }
            _ => return Err("Invalid character in character class".to_string()),
        };
        let ClassItem::Char(high) = end else {
            return Err("Invalid character in character class".to_string());
        };
        if high < low {
            return Err("Range out of order in character class".to_string());
        }
        operands.push(SetOperand::Range(low, high));
        Ok(())
    }

    /// Parse a `v`-mode class escape: string literals, shorthands,
    /// Unicode properties, and character escapes.
    fn class_set_escape(&mut self) -> ParseResult<ClassItem> {
        self.pos += 1; // `\`
        let Some(e) = self.peek() else {
            return Err("Trailing backslash in regular expression".to_string());
        };
        if e == 'q' {
            self.pos += 1;
            return Ok(ClassItem::Strings(StringAlts::new(self.q_string()?)));
        }
        self.pos += 1;
        self.class_atom_tail(e)
    }

    /// Parse `\q{alt|alt|…}` string alternatives as UTF-16 units. Each
    /// alternative holds characters and character escapes; shorthands and
    /// nested sets are rejected, and empty alternatives match empty.
    fn q_string(&mut self) -> ParseResult<Vec<Vec<u16>>> {
        if !self.eat('{') {
            return Err("Invalid escape '\\q'".to_string());
        }
        let mut alternatives = Vec::new();
        let mut current = Vec::new();
        loop {
            match self.peek() {
                None => return Err("Unterminated character class".to_string()),
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                Some('|') => {
                    self.pos += 1;
                    alternatives.push(std::mem::take(&mut current));
                }
                Some('\\') => {
                    self.pos += 1;
                    let Some(e) = self.peek() else {
                        return Err("Trailing backslash in regular expression".to_string());
                    };
                    self.pos += 1;
                    match self.class_atom_tail(e)? {
                        ClassItem::Char(unit) if unit <= 0xFFFF => current.push(unit as u16),
                        _ => return Err("Invalid escape in string literal".to_string()),
                    }
                }
                Some(c) => {
                    self.pos += 1;
                    let mut pair = [0u16; 2];
                    current.extend_from_slice(c.encode_utf16(&mut pair));
                }
            }
        }
        alternatives.push(current);
        Ok(alternatives)
    }

    /// Whether a lowered union holds string alternatives, directly or
    /// through nesting, conjunction, or difference. `\q` always counts,
    /// even for single-unit alternatives.
    pub(super) fn set_contains_strings(items: &[ClassItem]) -> bool {
        items.iter().any(|item| match item {
            ClassItem::Strings(_) => true,
            ClassItem::Nested { items, .. } => Self::set_contains_strings(items),
            ClassItem::Conjoin(unions) => {
                unions.iter().any(|union| Self::set_contains_strings(union))
            }
            ClassItem::Except { left, right } => {
                Self::set_contains_strings(left) || Self::set_contains_strings(right)
            }
            _ => false,
        })
    }

    /// RGI Emoji_Keycap_Sequence: `[0-9#*]` with a required U+FE0F
    /// before U+20E3. Fully specified by UTS#51, no data tables needed.
    fn keycap_sequences() -> Vec<Vec<u16>> {
        let mut sequences = Vec::with_capacity(12);
        for base in "0123456789#*".chars() {
            sequences.push(vec![base as u16, 0xFE0F, 0x20E3]);
        }
        sequences
    }

    /// Resolve a `v`-mode property-of-strings name (loosely matched) to
    /// its alternatives. RGI_Emoji composes the six sequence sets plus the
    /// fully-qualified singles; Basic_Emoji the singles plus the
    /// presentation-selector pairs.
    fn string_property(name: &str) -> Option<Vec<Vec<u16>>> {
        use super::emoji_tables as tables;
        let take = |table: &[&[u16]]| table.iter().map(|units| units.to_vec()).collect::<Vec<_>>();
        match name {
            "emojikeycapsequence" => Some(Self::keycap_sequences()),
            "rgiemojiflagsequence" => Some(take(tables::RGI_EMOJI_FLAG_SEQUENCE)),
            "rgiemojitagsequence" => Some(take(tables::RGI_EMOJI_TAG_SEQUENCE)),
            "rgiemojimodifiersequence" => Some(take(tables::RGI_EMOJI_MODIFIER_SEQUENCE)),
            "rgiemojizwjsequence" => Some(take(tables::RGI_EMOJI_ZWJ_SEQUENCE)),
            "basicemoji" => {
                let mut strings: Vec<Vec<u16>> = tables::RGI_EMOJI_SINGLES
                    .iter()
                    .map(|cp| {
                        char::from_u32(*cp).map(|c| {
                            let mut pair = [0u16; 2];
                            c.encode_utf16(&mut pair).to_vec()
                        })
                    })
                    .collect::<Option<_>>()?;
                strings.extend(take(tables::BASIC_EMOJI_PAIRS));
                Some(strings)
            }
            "rgiemoji" => {
                let mut strings: Vec<Vec<u16>> = tables::RGI_EMOJI_SINGLES
                    .iter()
                    .map(|cp| {
                        char::from_u32(*cp).map(|c| {
                            let mut pair = [0u16; 2];
                            c.encode_utf16(&mut pair).to_vec()
                        })
                    })
                    .collect::<Option<_>>()?;
                strings.extend(Self::keycap_sequences());
                for table in [
                    tables::RGI_EMOJI_FLAG_SEQUENCE,
                    tables::RGI_EMOJI_TAG_SEQUENCE,
                    tables::RGI_EMOJI_MODIFIER_SEQUENCE,
                    tables::RGI_EMOJI_ZWJ_SEQUENCE,
                    tables::BASIC_EMOJI_PAIRS,
                ] {
                    strings.extend(take(table));
                }
                Some(strings)
            }
            _ => None,
        }
    }

    /// Shared class-escape shapes for both class flavors. The escape
    /// character is already consumed.
    fn class_atom_tail(&mut self, e: char) -> ParseResult<ClassItem> {
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
            'b' => ClassItem::Char(8),
            other => ClassItem::Char(self.escaped_char(other)?),
        })
    }

    fn class_atom(&mut self) -> ParseResult<ClassItem> {
        let Some(c) = self.peek() else {
            return Err("Unterminated character class".to_string());
        };
        if c != '\\' {
            self.pos += 1;
            return Ok(ClassItem::Char(c as u32));
        }
        self.pos += 1; // `\`
        let Some(e) = self.peek() else {
            return Err("Trailing backslash in regular expression".to_string());
        };
        self.pos += 1;
        self.class_atom_tail(e)
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
            'k' if !self.unicode && !self.has_named_captures => Node::Char('k' as u32),
            'k' => {
                if !self.eat('<') {
                    return Err("Invalid named reference".to_string());
                }
                let name = self.group_name()?;
                Node::NamedBackref(name)
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
        // Properties of strings are outside regex-syntax's scalar tables.
        // Emoji_Keycap_Sequence is fully specified by UTS#51, so it is
        // enumerated directly; anything else stays an error, as does a
        // negated string property.
        let loose: String = property
            .chars()
            .filter(|c| !matches!(c, '_' | '-' | ' '))
            .flat_map(|c| c.to_lowercase())
            .collect();
        // String properties exist only in `v` mode; without it the name
        // falls through to the scalar tables, which reject it. Negated
        // string properties stay an error, matching V8.
        if self.vsets
            && let Some(strings) = Self::string_property(&loose)
        {
            if negated {
                return Err("Cannot negate strings in character class".to_string());
            }
            return Ok(ClassItem::Strings(StringAlts::new(strings)));
        }
        let (class, surrogates) = super::unicode::property(&property)
            .ok_or_else(|| "Invalid Unicode property escape".to_string())?;
        let ranges = class
            .ranges()
            .iter()
            .map(|range| (range.start(), range.end()))
            .collect();
        let to_ranges = |class: &regex_syntax::hir::ClassUnicode| {
            class
                .ranges()
                .iter()
                .map(|r| (r.start(), r.end()))
                .collect()
        };
        let folded = super::unicode::case_closure(&class);
        let mut complement = class.clone();
        complement.negate();
        let complement = super::unicode::case_closure(&complement);
        Ok(ClassItem::UnicodeProperty {
            surrogates,
            ranges,
            folded: to_ranges(&folded),
            folded_complement: to_ranges(&complement),
            negated,
        })
    }

    /// Resolve a single-character escape: the control abbreviations, the
    /// numeric forms, and — outside Unicode mode — an escaped literal.
    fn escaped_char(&mut self, e: char) -> ParseResult<u32> {
        Ok(match e {
            'n' => '\n' as u32,
            't' => '\t' as u32,
            'r' => '\r' as u32,
            'f' => '\u{000C}' as u32,
            'v' => '\u{000B}' as u32,
            '0' => '\0' as u32,
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
                    if value > 0x10FFFF {
                        return Err("Invalid Unicode escape".into());
                    }
                    value
                } else {
                    let high = self.code_point(4)?;
                    if self.unicode
                        && (0xD800..=0xDBFF).contains(&high)
                        && self.chars.get(self.pos..self.pos + 2) == Some(&['\\', 'u'])
                    {
                        let saved = self.pos;
                        self.pos += 2;
                        match self.code_point(4) {
                            Ok(low) if (0xDC00..=0xDFFF).contains(&low) => {
                                0x10000 + ((high - 0xD800) << 10) + low - 0xDC00
                            }
                            _ => {
                                self.pos = saved;
                                high
                            }
                        }
                    } else {
                        high
                    }
                }
            }
            'c' => {
                // `\cA` … `\cZ`: a control character.
                let Some(letter) = self.peek() else {
                    return Ok('\\' as u32);
                };
                self.pos += 1;
                (letter as u32) % 32
            }
            other => {
                if self.unicode && other.is_ascii_alphanumeric() {
                    return Err(format!("Invalid escape '\\{}'", other));
                }
                other as u32
            }
        })
    }

    fn code_point(&mut self, digits: usize) -> ParseResult<u32> {
        let mut value: u32 = 0;
        for _ in 0..digits {
            let c = self.peek().ok_or("Invalid escape")?;
            value = value
                .saturating_mul(16)
                .saturating_add(c.to_digit(16).ok_or("Invalid escape")?);
            self.pos += 1;
        }
        Ok(value)
    }
}
