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

mod emoji_tables;
mod parse;
mod unicode;

use std::cell::Cell;
use std::collections::HashMap;

pub use parse::{Anchor, ClassItem, Node, Shorthand};

/// Maximum backtracking steps for one match attempt.
///
/// Sized so an ordinary pattern never notices while a catastrophic one fails
/// in well under a second.
const MAX_STEPS: usize = 1_000_000;
const STEPS_PER_INPUT_UNIT: usize = 64;

/// A compiled pattern together with its flags.
#[derive(Debug)]
pub struct Regex {
    root: Node,
    pub group_count: usize,
    pub names: HashMap<String, Vec<usize>>,
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

/// Validate ECMAScript pattern grammar independently of matcher coverage.
/// Literals and constructors use the same validator; parsing a literal does
/// not execute or compile the engine's partial matcher implementation.
pub(crate) fn validate_syntax(source: &crate::JsString, flags: &str) -> Result<String, String> {
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
    // Bound recursive grammar before entering either parser. Brackets in
    // ordinary classes are characters, while v-mode permits nested sets.
    let mut groups = 0usize;
    let mut classes = 0usize;
    let mut escaped = false;
    for character in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        match character {
            '[' if classes == 0 || flags.contains('v') => classes += 1,
            ']' => classes = classes.saturating_sub(1),
            '(' if classes == 0 => groups += 1,
            ')' if classes == 0 => groups = groups.saturating_sub(1),
            _ => {}
        }
        if groups + classes > 128 {
            return Err("Maximum regular expression nesting exceeded".into());
        }
    }
    // Normalize only identifier escapes in capture names. Oxc compares raw
    // escaped spellings for named references; ECMAScript compares StringValue.
    let normalized_names = normalize_group_names(&pattern, unicode)?;
    let allocator = oxc_allocator::Allocator::default();
    oxc_regular_expression::LiteralParser::new(
        &allocator,
        normalized_names.as_deref().unwrap_or(&pattern),
        Some(flags),
        oxc_regular_expression::Options::default(),
    )
    .parse()
    .map_err(|error| error.to_string())?;
    Ok(pattern)
}

impl Regex {
    pub fn new(source: impl Into<crate::JsString>, flags: &str) -> Result<Self, String> {
        let source = source.into();
        let pattern = validate_syntax(&source, flags)?;
        let unicode = flags.contains('u') || flags.contains('v');
        let parsed = parse::Parser::new(&pattern, unicode, flags.contains('v')).parse()?;
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
            // Linear scans over large inputs are legitimate work, not
            // runaway backtracking: grant a per-unit allowance above the
            // flat budget so only super-linear blowups trip it.
            budget: MAX_STEPS.saturating_add(input.len().saturating_mul(STEPS_PER_INPUT_UNIT)),
            depth: Cell::new(0),
            modifiers: Cell::new(
                u8::from(self.ignore_case)
                    | (u8::from(self.multiline) << 1)
                    | (u8::from(self.dot_all) << 2),
            ),
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
    budget: usize,
    depth: Cell<usize>,
    modifiers: Cell<u8>,
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
        if steps > self.budget {
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
            || ((self.modifiers.get() & 1 != 0)
                && unicode::canonical(a, self.regex.unicode)
                    == unicode::canonical(b, self.regex.unicode))
    }
    fn word_character(&self, c: u32) -> bool {
        matches!(c, 48..=57 | 65..=90 | 97..=122 | 95)
            || (self.regex.unicode && self.modifiers.get() & 1 != 0 && matches!(c, 0x017f | 0x212a))
    }
    fn is_word(&self, index: usize) -> bool {
        self.scalar(index).is_some_and(|c| self.word_character(c))
    }
    fn is_line_terminator(c: u16) -> bool {
        matches!(c, 10 | 13 | 0x2028 | 0x2029)
    }
    fn class_matches(&self, negated: bool, items: &[ClassItem], c: u32) -> bool {
        self.union_matches(items, c) != negated
    }

    /// Possible end positions for a class match at `pos`: string
    /// alternatives in source order, then one scalar advance. Callers
    /// backtrack across every end through their continuation.
    fn class_ends(
        &self,
        negated: bool,
        items: &[ClassItem],
        pos: usize,
    ) -> Result<Vec<usize>, String> {
        if negated {
            // Negated classes never hold strings (rejected at parse).
            return Ok(match self.scalar(pos) {
                Some(c) if !self.union_matches(items, c) => vec![self.advance(pos)],
                _ => vec![],
            });
        }
        let mut ends = Vec::new();
        for end in self.union_ends(items, pos)? {
            if !ends.contains(&end) {
                ends.push(end);
            }
        }
        Ok(ends)
    }

    /// End positions for a union: every string alternative that matches
    /// here, plus one scalar advance when the scalar predicate matches.
    fn union_ends(&self, items: &[ClassItem], pos: usize) -> Result<Vec<usize>, String> {
        let mut ends = Vec::new();
        for item in items {
            match item {
                ClassItem::Strings(strings) => {
                    if self.modifiers.get() & 1 == 0 && pos < self.input.len() {
                        self.string_table_ends(strings, pos, &mut ends);
                        if strings.has_empty() {
                            ends.push(pos);
                        }
                    } else {
                        for alternative in strings.all() {
                            if self.string_at(alternative, pos) {
                                ends.push(pos + alternative.len());
                            }
                        }
                    }
                }
                ClassItem::Nested { negated, items } => {
                    let sub = self.union_ends(items, pos)?;
                    if *negated {
                        if sub.is_empty() && self.scalar(pos).is_some() {
                            ends.push(self.advance(pos));
                        }
                    } else {
                        ends.extend(sub);
                    }
                }
                ClassItem::Conjoin(unions) => {
                    let mut common: Option<Vec<usize>> = None;
                    for union in unions {
                        let union_ends = self.union_ends(union, pos)?;
                        common = Some(match common {
                            None => union_ends,
                            Some(kept) => kept
                                .into_iter()
                                .filter(|end| union_ends.contains(end))
                                .collect(),
                        });
                    }
                    ends.extend(common.unwrap_or_default());
                }
                ClassItem::Except { left, right } => {
                    let excluded = self.union_ends(right, pos)?;
                    ends.extend(
                        self.union_ends(left, pos)?
                            .into_iter()
                            .filter(|end| !excluded.contains(end)),
                    );
                }
                _ => {}
            }
        }
        if let Some(c) = self.scalar(pos)
            && self.union_matches(items, c)
        {
            ends.push(self.advance(pos));
        }
        Ok(ends)
    }

    /// Ends for a string table at `pos` on the no-fold path, in source
    /// alternative order: single-unit matches (which always match their
    /// first unit) merged by alternative index with the verified longer
    /// matches. Callers deduplicate or sort as their order needs.
    fn string_table_ends(&self, strings: &parse::StringAlts, pos: usize, ends: &mut Vec<usize>) {
        let unit = self.input[pos];
        // Verified longer matches as (alternative index, end). Indices
        // ascend because the pair buckets were built in source order.
        let mut longer = Vec::new();
        if pos + 1 < self.input.len()
            && let Some(indices) = strings.candidates(unit, self.input[pos + 1])
        {
            for index in indices {
                let alternative = &strings.all()[*index];
                if self.string_at(alternative, pos) {
                    longer.push((*index, pos + alternative.len()));
                }
            }
        }
        // Singles match unconditionally on the no-fold path. Both lists
        // ascend in alternative index, so a merge join preserves source
        // alternative order exactly.
        let mut singles = strings
            .singles(unit)
            .unwrap_or(&[])
            .iter()
            .map(|index| (*index, pos + 1))
            .peekable();
        let mut longer = longer.into_iter().peekable();
        while longer.peek().is_some() {
            let take_single = match (singles.peek(), longer.peek()) {
                (Some((si, _)), Some((li, _))) => si <= li,
                (Some(_), None) => true,
                _ => false,
            };
            if take_single {
                let (_, end) = singles.next().expect("peeked single");
                ends.push(end);
            } else {
                let (_, end) = longer.next().expect("peeked longer");
                ends.push(end);
            }
        }
        for (_, end) in singles {
            ends.push(end);
        }
    }

    /// Match one input unit slice against a string alternative, folding
    /// each unit under the `i` flag like scalar matching does.
    fn string_at(&self, alternative: &[u16], pos: usize) -> bool {
        if alternative.is_empty() {
            return true;
        }
        if pos + alternative.len() > self.input.len() {
            return false;
        }
        alternative
            .iter()
            .enumerate()
            .all(|(offset, unit)| self.same_char(self.input[pos + offset] as u32, *unit as u32))
    }

    /// Match one scalar against a union of class items, including `v`-mode
    /// nesting, conjunction, and difference. Case folding reuses each
    /// item's own semantics. Single-unit string alternatives match their
    /// scalar here too.
    fn union_matches(&self, items: &[ClassItem], c: u32) -> bool {
        items.iter().any(|item| match item {
            ClassItem::Strings(strings) => {
                if self.modifiers.get() & 1 == 0 && c <= u16::MAX as u32 {
                    strings.has_single(c as u16)
                } else {
                    strings.all().iter().any(|alternative| {
                        alternative.len() == 1 && self.same_char(c, alternative[0] as u32)
                    })
                }
            }
            ClassItem::Nested { negated, items } => self.union_matches(items, c) != *negated,
            ClassItem::Conjoin(unions) => unions.iter().all(|union| self.union_matches(union, c)),
            ClassItem::Except { left, right } => {
                self.union_matches(left, c) && !self.union_matches(right, c)
            }
            ClassItem::Char(want) => self.same_char(c, *want),
            ClassItem::Range(low, high) => {
                (*low..=*high).contains(&c)
                    || (self.modifiers.get() & 1 != 0
                        && unicode::range_matches(c, *low, *high, self.regex.unicode))
            }
            ClassItem::Shorthand { kind, negated } => {
                let base = match kind {
                    Shorthand::Digit => (48..=57).contains(&c),
                    Shorthand::Word => self.word_character(c),
                    Shorthand::Space => {
                        matches!(c, 0x0009..=0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
                    }
                };
                base != *negated
            }
            ClassItem::UnicodeProperty {
                surrogates,
                ranges,
                folded,
                folded_complement,
                negated,
            } => {
                let folding = self.modifiers.get() & 1 != 0;
                // Unicode mode folds the complement of a negative property;
                // UnicodeSets mode complements the already folded property.
                let complement_first = folding && *negated && !self.regex.flags.contains('v');
                let ranges = if complement_first {
                    folded_complement
                } else if folding {
                    folded
                } else {
                    ranges
                };
                let index = ranges.partition_point(|(_, high)| (*high as u32) < c);
                let hit = if (0xd800..=0xdfff).contains(&c) {
                    if complement_first {
                        !surrogates
                    } else {
                        *surrogates
                    }
                } else {
                    ranges
                        .get(index)
                        .is_some_and(|(low, high)| (*low as u32..=*high as u32).contains(&c))
                };
                if complement_first {
                    hit
                } else {
                    hit != *negated
                }
            }
        })
    }

    fn node(
        &self,
        node: &Node,
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            self.node_inner(node, pos, caps, k)
        })
    }

    fn node_inner(
        &self,
        node: &Node,
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        let _depth = self.enter()?;
        self.step()?;
        match node {
            Node::Modifiers { add, remove, node } => {
                let outer = self.modifiers.get();
                let inner = (outer | add) & !remove;
                self.modifiers.set(inner);
                let result = self.node(node, pos, caps, &mut |pos, caps| {
                    self.modifiers.set(outer);
                    let accepted = k(pos, caps);
                    self.modifiers.set(inner);
                    accepted
                });
                self.modifiers.set(outer);
                result
            }
            Node::Empty => Ok(k(pos, caps)),
            Node::Char(want) => match self.scalar(pos) {
                Some(c) if self.same_char(c, *want) => Ok(k(self.advance(pos), caps)),
                _ => Ok(false),
            },
            Node::AnyChar => match self.input.get(pos) {
                Some(c) if (self.modifiers.get() & 4 != 0) || !Self::is_line_terminator(*c) => {
                    Ok(k(self.advance(pos), caps))
                }
                _ => Ok(false),
            },
            Node::Class { negated, items } => {
                for end in self.class_ends(*negated, items, pos)? {
                    if k(end, caps) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
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
            Node::Backref(index) => self.backreference(*index, pos, caps, k),
            Node::NamedBackref(name) => {
                let index = self
                    .regex
                    .names
                    .get(name)
                    .and_then(|indices| {
                        indices
                            .iter()
                            .find(|index| caps.get(**index).is_some_and(Option::is_some))
                    })
                    .copied();
                match index {
                    Some(index) => self.backreference(index, pos, caps, k),
                    None => Ok(k(pos, caps)),
                }
            }
            Node::Anchor(anchor) => {
                let ok = match anchor {
                    Anchor::Start => {
                        pos == 0
                            || ((self.modifiers.get() & 2 != 0)
                                && Self::is_line_terminator(self.input[pos - 1]))
                    }
                    Anchor::End => {
                        pos == self.input.len()
                            || ((self.modifiers.get() & 2 != 0)
                                && Self::is_line_terminator(self.input[pos]))
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

    fn backreference(
        &self,
        index: usize,
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        let Some(Some((start, end))) = caps.get(index).copied() else {
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

    fn sequence(
        &self,
        nodes: &[Node],
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
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
        let mut atom = node;
        while let Node::Group { index: None, node } = atom {
            atom = node;
        }
        // Classes iterate end-position sets without nesting one frame per
        // repetition, so long runs over string tables cannot exhaust the
        // nesting budget. Scalar-only classes keep the exact historical
        // order (counts, then the single advance).
        if count == 0 {
            if let Node::Class { negated, items } = atom {
                return self.repeat_class(*negated, items, min, max, greedy, pos, caps, k);
            }
            if matches!(atom, Node::Char(_) | Node::AnyChar) {
                return self.repeat_atom(atom, min, max, greedy, pos, caps, k);
            }
        }
        self.step()?;
        let mut error = None;
        // Below the minimum there is no choice: the body must match again.
        if count < min {
            clear_captures(node, caps);
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
            clear_captures(node, caps);
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
    /// Repeat a class without nesting one frame per iteration. Each
    /// layer holds the end positions reachable in exactly that many
    /// iterations; expansion stops at `max` or when no new end appears
    /// (ends range over a finite slice of the input, so this terminates).
    /// Backtracking then tries (count, end) pairs highest-count-first for
    /// greedy quantifiers and lowest-first for lazy ones; the continuation
    /// never observes the count, so each distinct end is tried once.
    /// Scalar-only classes produce one end per layer, exactly the order
    /// the historical positions loop used.
    #[allow(clippy::too_many_arguments)]
    fn repeat_class(
        &self,
        negated: bool,
        items: &[ClassItem],
        min: u32,
        max: Option<u32>,
        greedy: bool,
        pos: usize,
        caps: &mut Captures,
        k: Cont<'_>,
    ) -> Result<bool, String> {
        let strings = parse::Parser::set_contains_strings(items);
        // Scalar-only classes advance deterministically (one scalar per
        // iteration), so the layers would be singletons. Walk the maximal
        // run directly instead: same greedy/lazy end order, one position
        // vector, no per-layer or per-count allocation. This keeps
        // million-unit `+` runs (unicode property tests) in budget.
        let scalar_only = !strings
            && items.iter().all(|item| {
                matches!(
                    item,
                    parse::ClassItem::Char(_)
                        | parse::ClassItem::Range(_, _)
                        | parse::ClassItem::Shorthand { .. }
                        | parse::ClassItem::UnicodeProperty { .. }
                )
            });
        if scalar_only {
            let mut ends = vec![pos];
            let mut current = pos;
            loop {
                if max.is_some_and(|limit| ends.len() > limit as usize) {
                    break;
                }
                let Some(c) = self.scalar(current) else {
                    break;
                };
                if !self.class_matches(negated, items, c) {
                    break;
                }
                current = self.advance(current);
                ends.push(current);
            }
            let top = ends.len() - 1;
            if greedy {
                for end in ends.iter().take(top + 1).skip(min as usize).rev() {
                    if k(*end, caps) {
                        return Ok(true);
                    }
                }
            } else {
                for end in ends.iter().take(top + 1).skip(min as usize) {
                    if k(*end, caps) {
                        return Ok(true);
                    }
                }
            }
            return Ok(false);
        }
        let mut layers: Vec<Vec<usize>> = vec![vec![pos]];
        // Every end ever produced, so the fixed-point check below stays
        // linear instead of rescanning all previous layers per end. Layers
        // keep their full end sets (including ends seen at lower counts)
        // because greedy/lazy backtracking order depends on which counts
        // can reach each end.
        let mut seen = vec![false; self.input.len() + 1];
        seen[pos] = true;
        loop {
            let count = layers.len() - 1;
            if max.is_some_and(|m| count as u64 >= m as u64) {
                break;
            }
            if strings {
                self.step()?;
            }
            let mut next = Vec::new();
            // Pure string tables dominate long runs (RGI has ~4k entries):
            // expand inline without one allocation per end, then sort and
            // deduplicate. The end multiset matches `class_ends` exactly.
            let table_only = !negated
                && self.modifiers.get() & 1 == 0
                && matches!(items, [parse::ClassItem::Strings(_)]);
            if table_only {
                let [parse::ClassItem::Strings(strings)] = items else {
                    unreachable!("table-only match checked above")
                };
                for &start in layers.last().expect("layer") {
                    if start < self.input.len() {
                        self.string_table_ends(strings, start, &mut next);
                    } else if strings.has_empty() {
                        next.push(start);
                    }
                }
                next.sort_unstable();
                next.dedup();
            } else {
                for &start in layers.last().expect("layer") {
                    for end in self.class_ends(negated, items, start)? {
                        if !next.contains(&end) {
                            next.push(end);
                        }
                    }
                }
                next.sort_unstable();
            }
            if next.iter().all(|end| {
                let already = seen[*end];
                seen[*end] = true;
                already
            }) {
                layers.push(next);
                break;
            }
            layers.push(next);
        }
        // Counts beyond the frozen layers repeat the same ends; extend to
        // `min` so empty-matching iterations can still satisfy it.
        while ((layers.len() - 1) as u64) < min as u64 {
            layers.push(layers.last().expect("layer").clone());
        }
        let top = layers.len() - 1;
        let mut counts: Vec<usize> = (min as usize..=top).collect();
        if greedy {
            counts.reverse();
        }
        let mut tried = vec![false; self.input.len() + 1];
        for count in counts {
            let mut ends = layers[count].clone();
            if greedy {
                ends.sort_unstable_by(|a, b| b.cmp(a));
            } else {
                ends.sort_unstable();
            }
            for end in ends {
                if tried[end] {
                    continue;
                }
                tried[end] = true;
                if k(end, caps) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

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
                Node::AnyChar => self.input.get(pos).is_some_and(|c| {
                    (self.modifiers.get() & 4 != 0) || !Self::is_line_terminator(*c)
                }),
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

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod modifier_tests {
    use super::Regex;

    fn matches(pattern: &str, flags: &str, input: &str) -> bool {
        Regex::new(pattern, flags)
            .unwrap()
            .find_at(&input.encode_utf16().collect::<Vec<_>>(), 0)
            .unwrap()
            .is_some()
    }

    #[test]
    fn unicode_17_properties_and_simple_case_equivalence() {
        for flags in ["u", "v"] {
            assert!(matches(r"^\p{Alphabetic}$", flags, "\u{88f}"));
            assert!(matches(r"^\p{Assigned}$", flags, "\u{1fac8}"));
            assert!(matches(r"^\p{Script=Sidetic}$", flags, "\u{10940}"));
            assert!(matches(
                r"^\p{Script_Extensions=Sidetic}$",
                flags,
                "\u{10940}"
            ));
            assert!(matches(
                r"^\p{General_Category=Currency_Symbol}$",
                flags,
                "\u{20c1}"
            ));
            assert!(Regex::new(r"\p{alphabetic}", flags).is_err());
            assert!(Regex::new(r"\p{Script=latin}", flags).is_err());
            assert!(Regex::new(r"\p{Block=ASCII}", flags).is_err());
        }
        assert!(matches("^\u{390}$", "iu", "\u{1fd3}"));
        assert!(matches("^[\u{390}-\u{390}]$", "iu", "\u{1fd3}"));
        assert!(matches("^s$", "iu", "ſ"));
        assert!(!matches("^s$", "i", "ſ"));
        assert!(!matches("^[s-s]$", "i", "ſ"));
        assert!(!matches("^k$", "i", "K"));
        assert!(matches("^k$", "iu", "K"));
        assert!(!matches("^[I-I]$", "iu", "ı"));
        assert!(!matches("^[S-S]$", "iu", "ß"));
        assert!(matches("^꟎$", "iu", "꟏"));
        assert!(matches(r"^\p{Uppercase}$", "iu", "꟏"));
    }

    #[test]
    fn whitespace_classes_use_ecmascript_whitespace() {
        for flags in ["", "u", "v"] {
            for c in ["\u{85}", "\u{180e}", "\u{200b}"] {
                assert!(!matches(r"\s", flags, c));
                assert!(matches(r"\S", flags, c));
            }
            for c in ["\t", "\n", "\u{a0}", "\u{2028}", "\u{feff}"] {
                assert!(matches(r"\s", flags, c));
                assert!(!matches(r"\S", flags, c));
            }
        }
    }

    #[test]
    fn unicode_properties_include_lone_surrogates_where_required() {
        for flags in ["u", "v", "iu", "iv"] {
            for property in [
                "Any",
                "Assigned",
                "Cs",
                "Surrogate",
                "gc=Cs",
                "General_Category=Surrogate",
                "C",
                "Other",
            ] {
                for unit in [0xd800, 0xdbff, 0xdc00, 0xdfff] {
                    let positive = Regex::new(format!(r"^\p{{{property}}}+$"), flags).unwrap();
                    assert!(
                        positive.find_at(&[unit], 0).unwrap().is_some(),
                        "{property}/{flags}"
                    );
                    let negative = Regex::new(format!(r"^\P{{{property}}}+$"), flags).unwrap();
                    assert!(
                        negative.find_at(&[unit], 0).unwrap().is_none(),
                        "{property}/{flags}"
                    );
                }
            }
            assert!(
                Regex::new(r"^\p{Cs}$", flags)
                    .unwrap()
                    .find_at(&[0xd83d, 0xde00], 0)
                    .unwrap()
                    .is_none()
            );
            assert!(
                Regex::new(r"^\P{Cs}$", flags)
                    .unwrap()
                    .find_at(&[0xd83d, 0xde00], 0)
                    .unwrap()
                    .is_some()
            );
            assert!(
                Regex::new(r"^\p{ASCII}$", flags)
                    .unwrap()
                    .find_at(&[0xd800], 0)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn scalar_class_repeat_preserves_bounds_backtracking_and_unicode() {
        assert!(matches(r"^[a-z]+[a-z]$", "", "abc"));
        assert!(matches(r"^[a-z]+?[a-z]$", "", "abc"));
        assert!(!matches(r"^[a-z]{4,}$", "", "abc"));
        assert!(matches(r"^[a-z]{0}$", "", ""));
        assert!(matches(r"^\p{ASCII}{2,3}b$", "u", "aab"));
        assert!(!matches(r"^\p{ASCII}{2,3}b$", "u", "ab"));
        assert!(matches(r"^[^a]{2}$", "u", "😀😀"));
        assert!(matches(r"^(?i:[a-z]+)Z$", "", "AbZ"));
        assert!(!matches(r"^(?i:[a-z]+)Z$", "", "Abz"));
        assert!(matches(r"^\p{ASCII}+$", "u", &"a".repeat(1_000_000)));
    }

    #[test]
    fn vsets_nesting_intersection_and_difference() {
        assert!(matches("[[0-9]]", "v", "5"));
        assert!(!matches("[[0-9]]", "v", "a"));
        assert!(matches("[^[^0-9]]", "v", "5"));
        assert!(!matches("[^[^0-9]]", "v", "a"));
        assert!(matches("[\\d&&[0-9]]", "v", "0"));
        assert!(!matches("[\\d&&[0-9]]", "v", "a"));
        assert!(matches("[[0-9]--[0-5]]", "v", "7"));
        assert!(!matches("[[0-9]--[0-5]]", "v", "3"));
        assert!(!matches("[_--_]", "v", "_"));
        assert!(!matches("[a&&b]", "v", "a"));
        assert!(matches("[a&&a]", "v", "a"));
    }

    #[test]
    fn vsets_string_literals_and_keycap_sequences() {
        assert!(matches("[\\q{ab}]", "v", "ab"));
        assert!(!matches("[\\q{ab}]", "v", "a"));
        assert!(matches(
            "[\\q{0|2|4|9\u{FE0F}\u{20E3}}]",
            "v",
            "9\u{FE0F}\u{20E3}"
        ));
        assert!(matches("[\\q{0|2|4|9\u{FE0F}\u{20E3}}]", "v", "2"));
        assert!(!matches("[\\q{0|2|4|9\u{FE0F}\u{20E3}}]", "v", "9"));
        assert!(matches(
            "[\\p{Emoji_Keycap_Sequence}]",
            "v",
            "1\u{FE0F}\u{20E3}"
        ));
        assert!(matches(
            "[\\p{Emoji_Keycap_Sequence}]",
            "v",
            "#\u{FE0F}\u{20E3}"
        ));
        assert!(!matches("[\\p{Emoji_Keycap_Sequence}]", "v", "1\u{20E3}"));
        assert!(!matches("[\\p{Emoji_Keycap_Sequence}]", "v", "1"));
        assert!(matches("^[\\q{ab|a}]+$", "v", "aba"));
        assert!(matches("[\\q{ab}--b]", "v", "ab"));
        assert!(!matches("[\\q{a}&&b]", "v", "a"));
        assert!(Regex::new("[^\\q{ab}]", "v").is_err());
        assert!(Regex::new("[\\p{Emoji_Keycap_Sequence}]", "u").is_err());
    }

    #[test]
    fn vsets_indexed_string_repeat_matches_long_runs() {
        // Shared-prefix alternatives exercise the first-unit index on
        // both match and mismatch paths.
        let run = "ab".repeat(2000);
        assert!(matches("^[\\q{a|ab|abc}]+$", "v", &run));
        assert!(!matches("^[\\q{a|ab|abc}]+$", "v", &(run.clone() + "x")));
        assert!(!matches("^[\\q{a|ab|abc}]+$", "v", "c"));
        // A full-table run over thousands of emoji must stay linear: the
        // repetition layer loop tracks seen ends in one set instead of
        // rescanning every previous layer per end (cubic). The 30s bound
        // is ~500x the observed ~50ms, so only the old blowup can trip it.
        let emoji = "\u{1F600}".repeat(4000);
        let started = std::time::Instant::now();
        assert!(matches("^\\p{RGI_Emoji}+$", "v", &emoji));
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
    }

    #[test]
    fn vsets_rejects_mixed_operators_and_ranges() {
        for pattern in [
            "[ab&&b]",
            "[a&&bc]",
            "[a&&b c]",
            "[a--b c]",
            "[a&&b--c]",
            "[a-b&&c]",
            "[a-b--c]",
            "[a&&]",
            "[a--]",
            "[\\d-z]",
            "[a-]",
            "[--a]",
            "[(]",
        ] {
            assert!(Regex::new(pattern, "v").is_err(), "{pattern}");
        }
    }

    #[test]
    fn scoped_flags_restore_on_exit_and_backtracking() {
        assert!(matches("^(?i:a)b$", "", "Ab"));
        assert!(!matches("^(?i:a)b$", "", "AB"));
        assert!(matches("^(?i:a|ab)c$", "", "ABc"));
        assert!(!matches("^(?i:a|ab)c$", "", "ABC"));
        assert!(matches("^(?i:a(?-i:b)c)d$", "", "AbCd"));
        assert!(!matches("^(?i:a(?-i:b)c)d$", "", "ABCd"));
        assert!(matches("^(?-i:a)b$", "i", "aB"));
        assert!(!matches("^(?-i:a)b$", "i", "AB"));
        assert!(matches("(?s-:.)", "", "\n"));
        assert!(matches("(?m:^a$)", "", "b\na\nc"));
        assert!(matches(r"(?i:\w)", "u", "ſ"));
        assert!(matches(r"(?i:\b)", "u", "K"));
        assert!(matches(r"(?i:\p{Lu})", "u", "a"));
        assert!(matches(r"(?i:\P{Lu})", "u", "A"));
        assert!(!matches(r"(?i:\P{Lu})", "v", "A"));
    }
}

fn clear_captures(node: &Node, caps: &mut Captures) {
    match node {
        Node::Group { index, node } => {
            if let Some(index) = index {
                caps[*index] = None;
            }
            clear_captures(node, caps);
        }
        Node::Concat(nodes) | Node::Alt(nodes) => {
            for node in nodes {
                clear_captures(node, caps);
            }
        }
        Node::Repeat { node, .. } | Node::Look { node, .. } | Node::Modifiers { node, .. } => {
            clear_captures(node, caps)
        }
        _ => {}
    }
}

fn normalize_group_names(pattern: &str, unicode: bool) -> Result<Option<String>, String> {
    if !pattern.contains('\\') || !(pattern.contains("(?<") || pattern.contains("\\k<")) {
        return Ok(None);
    }
    let parser = parse::Parser::new(pattern, unicode, false);
    let references = unicode || parser.has_named_captures;
    let chars: Vec<char> = pattern.chars().collect();
    let mut output = String::new();
    let mut at = 0;
    let mut class = false;
    while at < chars.len() {
        let capture = !class
            && chars[at..].starts_with(&['(', '?', '<'])
            && !matches!(chars.get(at + 3), Some('=' | '!'));
        let reference = !class && references && chars[at..].starts_with(&['\\', 'k', '<']);
        if capture || reference {
            output.extend(chars[at..at + 3].iter());
            at += 3;
            let end = chars[at..]
                .iter()
                .position(|c| *c == '>')
                .map(|n| at + n)
                .ok_or("Invalid capture group name")?;
            let raw: String = chars[at..=end].iter().collect();
            let name = parse::Parser::new(&raw, true, false).group_name()?;
            let mut identifier = name.chars();
            if !identifier
                .next()
                .is_some_and(crate::lexer::is_identifier_start)
                || !identifier.all(crate::lexer::is_identifier_continue)
            {
                return Err("Invalid capture group name".into());
            }
            output.push_str(&name);
            output.push('>');
            at = end + 1;
        } else {
            let c = chars[at];
            output.push(c);
            at += 1;
            if c == '\\' && at < chars.len() {
                output.push(chars[at]);
                at += 1;
            } else if c == '[' {
                class = true;
            } else if c == ']' {
                class = false;
            }
        }
    }
    Ok(Some(output))
}
