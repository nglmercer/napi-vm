//! A regular-expression engine with JavaScript semantics.
//!
//! Backtracking rather than automaton-based, because JavaScript's regular
//! expressions are not regular: backreferences and lookaround have no
//! finite-automaton equivalent, and leftmost-first alternation is defined in
//! terms of backtracking order. An engine built on a DFA would have to reject
//! those, and would pick different matches where it did not.
//!
//! Backtracking's cost is that a pathological pattern (`(a+)+b` against a long
//! run of `a`) can take exponential time. In a sandbox that is a denial of
//! service, so every match runs under a step budget and reports a catchable
//! error instead of hanging the host — the same treatment loops, recursion and
//! the job queue get.
//!
//! Positions are indices into a `Vec<char>`, so one index is one Unicode
//! scalar value. JavaScript indexes UTF-16 code units, so a pattern matching
//! text outside the Basic Multilingual Plane reports different offsets here.

mod parse;

use std::cell::Cell;
use std::collections::HashMap;

pub use parse::{Anchor, ClassItem, Node, Shorthand};

/// Maximum backtracking steps for one match attempt.
///
/// Sized so an ordinary pattern never notices while a catastrophic one fails
/// in well under a second.
const MAX_STEPS: usize = 1_000_000;

/// A compiled pattern together with its flags.
#[derive(Debug)]
pub struct Regex {
    root: Node,
    pub group_count: usize,
    pub names: HashMap<String, usize>,
    pub source: crate::JsString,
    pub flags: String,
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub sticky: bool,
    pub unicode: bool,
}

/// Where each capture group matched, as `(start, end)` character offsets.
/// Index 0 is the whole match.
pub type Captures = Vec<Option<(usize, usize)>>;

impl Regex {
    pub fn new(source: impl Into<crate::JsString>, flags: &str) -> Result<Self, String> {
        let source = source.into();
        let mut seen = String::new();
        for flag in flags.chars() {
            if !"dgimsuvy".contains(flag) {
                return Err(format!("Invalid regular expression flags: '{}'", flag));
            }
            if seen.contains(flag) {
                return Err(format!("Duplicate regular expression flag: '{}'", flag));
            }
            seen.push(flag);
        }
        let unicode = flags.contains('u') || flags.contains('v');
        let mut pattern = String::new();
        for cp in source.code_points() {
            let n = cp.code_point_at(0).unwrap();
            if (0xD800..=0xDFFF).contains(&n) {
                if unicode {
                    pattern.push_str(&format!("\\u{{{n:x}}}"));
                } else {
                    pattern.push_str(&format!("\\u{n:04x}"));
                }
            } else if !unicode && n > 0xFFFF {
                for u in cp.units() {
                    pattern.push_str(&format!("\\u{u:04x}"));
                }
            } else {
                pattern.push(char::from_u32(n).unwrap());
            }
        }
        let parsed = parse::Parser::new(&pattern, unicode).parse()?;
        Ok(Self {
            root: parsed.root,
            group_count: parsed.group_count,
            names: parsed.names,
            source: if source.is_empty() {
                crate::JsString::from("(?:)")
            } else {
                source.clone()
            },
            flags: flags.to_string(),
            global: flags.contains('g'),
            ignore_case: flags.contains('i'),
            multiline: flags.contains('m'),
            dot_all: flags.contains('s'),
            sticky: flags.contains('y'),
            unicode,
        })
    }

    /// Find the leftmost match at or after `start`.
    ///
    /// With the sticky flag the match must begin exactly at `start`; otherwise
    /// the start position advances until one is found.
    pub fn find_at(&self, input: &[u16], start: usize) -> Result<Option<Captures>, String> {
        if start > input.len() {
            return Ok(None);
        }
        let matcher = Matcher {
            regex: self,
            input,
            steps: Cell::new(0),
            depth: Cell::new(0),
        };
        // Unicode matching starts at a code-point boundary even when a
        // caller supplies lastIndex between the halves of a valid pair.
        let mut at = if self.unicode
            && start > 0
            && start < input.len()
            && (0xD800..=0xDBFF).contains(&input[start - 1])
            && (0xDC00..=0xDFFF).contains(&input[start])
        {
            start - 1
        } else {
            start
        };
        loop {
            let mut caps: Captures = vec![None; self.group_count + 1];
            let mut end: Option<usize> = None;
            let matched = matcher.node(&self.root, at, &mut caps, &mut |pos, _| {
                end = Some(pos);
                true
            })?;
            if matched {
                caps[0] = Some((at, end.unwrap_or(at)));
                return Ok(Some(caps));
            }
            if self.sticky || at >= input.len() {
                return Ok(None);
            }
            at = matcher.advance(at);
        }
    }
}

struct Matcher<'a> {
    regex: &'a Regex,
    input: &'a [u16],
    steps: Cell<usize>,
    depth: Cell<usize>,
}

struct MatchDepth<'a>(&'a Cell<usize>);
impl Drop for MatchDepth<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

/// The continuation a node calls once it has matched: "the rest of the pattern
/// starts here". Returning `true` accepts; returning `false` asks the node to
/// try its next alternative, which is how backtracking is expressed.
type Cont<'k> = &'k mut dyn FnMut(usize, &mut Captures) -> bool;

impl Matcher<'_> {
    fn enter(&self) -> Result<MatchDepth<'_>, String> {
        if self.depth.get() >= 128 {
            return Err("RangeError: Regular expression exceeded the nesting budget".into());
        }
        self.depth.set(self.depth.get() + 1);
        Ok(MatchDepth(&self.depth))
    }
    fn step(&self) -> Result<(), String> {
        let steps = self.steps.get() + 1;
        self.steps.set(steps);
        if steps > MAX_STEPS {
            return Err(
                "RangeError: Regular expression exceeded the backtracking budget".to_string(),
            );
        }
        Ok(())
    }

    fn scalar(&self, pos: usize) -> Option<u32> {
        let hi = *self.input.get(pos)?;
        if self.regex.unicode
            && (0xD800..=0xDBFF).contains(&hi)
            && let Some(&lo) = self.input.get(pos + 1)
            && (0xDC00..=0xDFFF).contains(&lo)
        {
            Some(0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00))
        } else {
            Some(hi as u32)
        }
    }
    fn advance(&self, pos: usize) -> usize {
        pos + if self.scalar(pos).is_some_and(|cp| cp > 0xFFFF) {
            2
        } else {
            1
        }
    }
    fn same_char(&self, a: u32, b: u32) -> bool {
        a == b
            || (self.regex.ignore_case
                && match (char::from_u32(a), char::from_u32(b)) {
                    (Some(a), Some(b)) => a.to_lowercase().eq(b.to_lowercase()),
                    _ => false,
                })
    }
    fn is_word(&self, index: usize) -> bool {
        self.input
            .get(index)
            .is_some_and(|c| matches!(*c,48..=57|65..=90|97..=122|95))
    }
    fn is_line_terminator(c: u16) -> bool {
        matches!(c, 10 | 13 | 0x2028 | 0x2029)
    }
    fn class_matches(&self, negated: bool, items: &[ClassItem], c: u32) -> bool {
        let hit = items.iter().any(|item| match item {
            ClassItem::Char(want) => self.same_char(c, *want),
            ClassItem::Range(low, high) => {
                (*low..=*high).contains(&c)
                    || (self.regex.ignore_case
                        && char::from_u32(c).is_some_and(|ch| {
                            ch.to_lowercase()
                                .any(|v| (*low..=*high).contains(&(v as u32)))
                                || ch
                                    .to_uppercase()
                                    .any(|v| (*low..=*high).contains(&(v as u32)))
                        }))
            }
            ClassItem::Shorthand { kind, negated } => {
                let base = match kind {
                    Shorthand::Digit => (48..=57).contains(&c),
                    Shorthand::Word => matches!(c,48..=57|65..=90|97..=122|95),
                    Shorthand::Space => {
                        char::from_u32(c).is_some_and(char::is_whitespace) || c == 0xFEFF
                    }
                };
                base != *negated
            }
            ClassItem::UnicodeProperty { ranges, negated } => {
                let hit = ranges
                    .iter()
                    .any(|(a, b)| (*a as u32..=*b as u32).contains(&c));
                hit != *negated
            }
        });
        hit != negated
    }

    fn node(
        &self,
        node: &Node,
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        let _depth = self.enter()?;
        self.step()?;
        match node {
            Node::Empty => Ok(k(pos, caps)),
            Node::Char(want) => match self.scalar(pos) {
                Some(c) if self.same_char(c, *want) => Ok(k(self.advance(pos), caps)),
                _ => Ok(false),
            },
            Node::AnyChar => match self.input.get(pos) {
                Some(c) if self.regex.dot_all || !Self::is_line_terminator(*c) => {
                    Ok(k(self.advance(pos), caps))
                }
                _ => Ok(false),
            },
            Node::Class { negated, items } => match self.scalar(pos) {
                Some(c) if self.class_matches(*negated, items, c) => Ok(k(self.advance(pos), caps)),
                _ => Ok(false),
            },
            Node::Concat(items) => self.sequence(items, pos, caps, k),
            Node::Alt(branches) => {
                // Leftmost-first: the earlier alternative wins even when a
                // later one would match more.
                for branch in branches {
                    let saved = caps.clone();
                    if self.node(branch, pos, caps, k)? {
                        return Ok(true);
                    }
                    *caps = saved;
                }
                Ok(false)
            }
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.repeat(node, *min, *max, *greedy, 0, pos, caps, k),
            Node::Group { index, node } => match index {
                None => self.node(node, pos, caps, k),
                Some(index) => {
                    let index = *index;
                    let start = pos;
                    let previous = caps.get(index).copied().flatten();
                    let matched = self.node(node, pos, caps, &mut |end, caps| {
                        let before = caps[index];
                        caps[index] = Some((start, end));
                        if k(end, caps) {
                            return true;
                        }
                        caps[index] = before;
                        false
                    })?;
                    if !matched && index < caps.len() {
                        caps[index] = previous;
                    }
                    Ok(matched)
                }
            },
            // An unset group matches the empty string, which is why
            // `/(a)?\1/` matches `""`.
            Node::Backref(index) => {
                let Some(Some((start, end))) = caps.get(*index).copied() else {
                    return Ok(k(pos, caps));
                };
                let len = end - start;
                if pos + len > self.input.len() {
                    return Ok(false);
                }
                for offset in 0..len {
                    if !self.same_char(
                        self.input[pos + offset] as u32,
                        self.input[start + offset] as u32,
                    ) {
                        return Ok(false);
                    }
                }
                Ok(k(pos + len, caps))
            }
            Node::Anchor(anchor) => {
                let ok = match anchor {
                    Anchor::Start => {
                        pos == 0
                            || (self.regex.multiline
                                && Self::is_line_terminator(self.input[pos - 1]))
                    }
                    Anchor::End => {
                        pos == self.input.len()
                            || (self.regex.multiline && Self::is_line_terminator(self.input[pos]))
                    }
                    Anchor::WordBoundary => {
                        let before = pos > 0 && self.is_word(pos - 1);
                        before != self.is_word(pos)
                    }
                    Anchor::NotWordBoundary => {
                        let before = pos > 0 && self.is_word(pos - 1);
                        before == self.is_word(pos)
                    }
                };
                if ok { Ok(k(pos, caps)) } else { Ok(false) }
            }
            Node::Look {
                ahead,
                negative,
                node,
            } => {
                let saved = caps.clone();
                let hit = if *ahead {
                    self.node(node, pos, caps, &mut |_, _| true)?
                } else {
                    // Lookbehind: try every start position that could end here.
                    // Bounded by `pos`, so it is linear in the offset rather
                    // than in the whole input.
                    let mut found = false;
                    for start in (0..=pos).rev() {
                        if self.node(node, start, caps, &mut |end, _| end == pos)? {
                            found = true;
                            break;
                        }
                    }
                    found
                };
                if hit == *negative {
                    *caps = saved;
                    return Ok(false);
                }
                // A negative lookaround contributes no captures.
                if *negative {
                    *caps = saved;
                }
                Ok(k(pos, caps))
            }
        }
    }

    fn sequence(
        &self,
        nodes: &[Node],
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        let _depth = self.enter()?;
        match nodes.split_first() {
            None => Ok(k(pos, caps)),
            Some((head, rest)) => {
                let mut error = None;
                let matched = self.node(head, pos, caps, &mut |pos, caps| {
                    match self.sequence(rest, pos, caps, k) {
                        Ok(ok) => ok,
                        Err(e) => {
                            error = Some(e);
                            // Stop unwinding: report the budget error rather
                            // than continue backtracking under it.
                            true
                        }
                    }
                })?;
                match error {
                    Some(e) => Err(e),
                    None => Ok(matched),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn repeat(
        &self,
        node: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        count: u32,
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        let _depth = self.enter()?;
        let mut atom = node;
        while let Node::Group { index: None, node } = atom {
            atom = node;
        }
        if count == 0 && matches!(atom, Node::Char(_) | Node::AnyChar | Node::Class { .. }) {
            return self.repeat_atom(atom, min, max, greedy, pos, caps, k);
        }
        self.step()?;
        let mut error = None;
        // Below the minimum there is no choice: the body must match again.
        if count < min {
            let matched = self.node(node, pos, caps, &mut |next, caps| match self.repeat(
                node,
                min,
                max,
                greedy,
                count + 1,
                next,
                caps,
                k,
            ) {
                Ok(ok) => ok,
                Err(e) => {
                    error = Some(e);
                    true
                }
            })?;
            return match error {
                Some(e) => Err(e),
                None => Ok(matched),
            };
        }

        let may_repeat = max.is_none_or(|max| count < max);

        // A lazy quantifier stops as early as it can, so the continuation is
        // tried before another iteration.
        if !greedy && k(pos, caps) {
            return Ok(true);
        }

        if may_repeat {
            let saved = caps.clone();
            // A body that matched the empty string would repeat forever, so
            // the iteration that consumed nothing is the last one.
            let matched = self.node(node, pos, caps, &mut |next, caps| {
                if next == pos {
                    return false;
                }
                match self.repeat(node, min, max, greedy, count + 1, next, caps, k) {
                    Ok(ok) => ok,
                    Err(e) => {
                        error = Some(e);
                        true
                    }
                }
            })?;
            if let Some(e) = error {
                return Err(e);
            }
            if matched {
                return Ok(true);
            }
            *caps = saved;
        }

        // Greedy: having exhausted the repetitions, hand over to the rest of
        // the pattern. Lazy: the continuation was already tried above.
        Ok(greedy && k(pos, caps))
    }

    // Character repeats need no capture stack: retain code-point boundaries
    // for greedy backtracking and consume lazy repeats only when needed.
    #[allow(clippy::too_many_arguments)]
    fn repeat_atom(
        &self,
        atom: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        mut pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        let saved = caps.clone();
        let mut positions = vec![pos];
        let mut count = 0;
        loop {
            self.step()?;
            if !greedy && count >= min {
                if k(pos, caps) {
                    return Ok(true);
                }
                caps.copy_from_slice(&saved);
            }
            if max.is_some_and(|max| count >= max) {
                break;
            }
            let matches = match atom {
                Node::Char(want) => self.scalar(pos).is_some_and(|c| self.same_char(c, *want)),
                Node::AnyChar => self
                    .input
                    .get(pos)
                    .is_some_and(|c| self.regex.dot_all || !Self::is_line_terminator(*c)),
                Node::Class { negated, items } => self
                    .scalar(pos)
                    .is_some_and(|c| self.class_matches(*negated, items, c)),
                _ => unreachable!("character atom"),
            };
            if !matches {
                break;
            }
            pos = self.advance(pos);
            count += 1;
            if greedy {
                positions.push(pos);
            }
        }
        if greedy && count >= min {
            for pos in positions.into_iter().skip(min as usize).rev() {
                self.step()?;
                if k(pos, caps) {
                    return Ok(true);
                }
                caps.copy_from_slice(&saved);
            }
        }
        Ok(false)
    }
}
