use std::fmt;

/// A source location span: line and column are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
    /// End line distinguishes token-internal newlines from intervening trivia.
    pub end_line: usize,
    /// Half-open UTF-16 offsets into the original source, when available.
    pub source_range: Option<(usize, usize)>,
}

impl Span {
    pub fn new(line: usize, col: usize) -> Self {
        Self {
            line,
            col,
            end_line: line,
            source_range: None,
        }
    }

    pub fn unknown() -> Self {
        Self {
            line: 0,
            col: 0,
            end_line: 0,
            source_range: None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        self.line == 0
    }

    pub(crate) fn with_source_range(mut self, start: usize, end: usize) -> Self {
        self.source_range = Some((start, end));
        self
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_unknown() {
            write!(f, "<unknown>")
        } else {
            write!(f, "line {} col {}", self.line, self.col)
        }
    }
}

/// A token paired with its source location.
pub type SpannedToken = (Token, Span);

/// Re-export Token here for convenience so callers can import from `span`.
pub use crate::lexer::Token;
