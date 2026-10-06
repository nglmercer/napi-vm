use std::fmt;

/// A source location span: line and column are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }

    pub fn unknown() -> Self {
        Self { line: 0, col: 0 }
    }

    pub fn is_unknown(&self) -> bool {
        self.line == 0
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
