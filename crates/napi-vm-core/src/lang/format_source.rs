//! Source formatting: indentation only.
//!
//! A formatter that reconstructs source from the AST would delete every
//! comment, because the parser does not keep them — and a formatter that
//! deletes comments is worse than none. This one works on the text instead,
//! and changes exactly two things: the indentation of each line, and trailing
//! whitespace. It never joins lines, splits them, or reorders anything.
//!
//! That restraint is what makes it safe. Automatic semicolon insertion depends
//! on where the newlines are, so a formatter that moved them could change what
//! a program means; this one cannot. It is also why the output is less tidy
//! than a full pretty-printer's: line breaks are the author's.

/// Where the scan currently is, so indentation is computed only from real
/// code — a brace inside a string or a comment must not shift anything.
#[derive(Clone, Copy, PartialEq)]
enum Scan {
    Code,
    BlockComment,
    /// A quoted string, remembering its delimiter.
    Text(char),
    /// A template literal. Nesting counts `${…}` so a brace inside one is not
    /// mistaken for the end of the template.
    Template,
}

pub struct FormatOptions {
    /// Spaces per indentation level. Zero uses a tab.
    pub indent_width: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self { indent_width: 2 }
    }
}

/// Re-indent `source`, leaving everything else exactly as written.
pub fn format_source(source: &str, options: &FormatOptions) -> String {
    let unit = if options.indent_width == 0 {
        "\t".to_string()
    } else {
        " ".repeat(options.indent_width)
    };

    let mut out = String::with_capacity(source.len());
    let mut depth: usize = 0;
    let mut scan = Scan::Code;
    // `${` nesting inside a template, so its closing brace is not counted.
    let mut template_depth: Vec<usize> = Vec::new();

    let lines: Vec<&str> = source.split('\n').collect();
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);

        // A line continuing a multi-line literal or block comment is content:
        // its leading whitespace belongs to the program, not to the layout.
        let continues_literal = matches!(scan, Scan::BlockComment | Scan::Template | Scan::Text(_));
        if continues_literal {
            out.push_str(line);
        } else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                // A blank line stays blank rather than collecting indentation.
            } else {
                // A line that *starts* by closing a block belongs one level
                // out, so `}` lines up with the statement that opened it.
                let closes = trimmed.starts_with('}')
                    || trimmed.starts_with(']')
                    || trimmed.starts_with(')');
                let level = if closes {
                    depth.saturating_sub(1)
                } else {
                    depth
                };
                out.push_str(&unit.repeat(level));
                out.push_str(trimmed);
            }
        }

        scan = scan_line(line, scan, &mut depth, &mut template_depth);

        if index + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}

/// Advance the scanner over one line, updating the nesting depth.
///
/// Returns the state the next line starts in.
fn scan_line(
    line: &str,
    mut scan: Scan,
    depth: &mut usize,
    template_depth: &mut Vec<usize>,
) -> Scan {
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        match scan {
            Scan::BlockComment => {
                if c == '*' && next == Some('/') {
                    scan = Scan::Code;
                    index += 2;
                    continue;
                }
            }
            Scan::Text(quote) => {
                if c == '\\' {
                    index += 2;
                    continue;
                }
                if c == quote {
                    scan = Scan::Code;
                }
            }
            Scan::Template => {
                if c == '\\' {
                    index += 2;
                    continue;
                }
                if c == '`' {
                    scan = Scan::Code;
                } else if c == '$' && next == Some('{') {
                    // Remember the depth to return to when this `${…}` closes.
                    template_depth.push(*depth);
                    scan = Scan::Code;
                    index += 2;
                    continue;
                }
            }
            Scan::Code => match c {
                // A line comment runs to the newline, so nothing after it on
                // this line affects the depth.
                '/' if next == Some('/') => return Scan::Code,
                '/' if next == Some('*') => {
                    scan = Scan::BlockComment;
                    index += 2;
                    continue;
                }
                '"' | '\'' => scan = Scan::Text(c),
                '`' => scan = Scan::Template,
                '{' | '[' | '(' => *depth += 1,
                '}' | ']' | ')' => {
                    // A `}` that closes a template's `${…}` returns to the
                    // template rather than to code.
                    if c == '}' && template_depth.last().is_some_and(|saved| *saved == *depth) {
                        template_depth.pop();
                        scan = Scan::Template;
                    } else {
                        *depth = depth.saturating_sub(1);
                    }
                }
                _ => {}
            },
        }
        index += 1;
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted(source: &str) -> String {
        format_source(source, &FormatOptions::default())
    }

    #[test]
    fn nested_blocks_are_indented() {
        assert_eq!(
            formatted("function f() {\nreturn 1;\n}"),
            "function f() {\n  return 1;\n}"
        );
    }

    #[test]
    fn over_indentation_is_removed() {
        assert_eq!(
            formatted("const a = 1;\n      const b = 2;"),
            "const a = 1;\nconst b = 2;"
        );
    }

    #[test]
    fn a_closing_line_lines_up_with_its_opener() {
        assert_eq!(
            formatted("if (x) {\nif (y) {\na();\n}\n}"),
            "if (x) {\n  if (y) {\n    a();\n  }\n}"
        );
    }

    #[test]
    fn comments_survive() {
        assert_eq!(
            formatted("function f() {\n// a note\nreturn 1;\n}"),
            "function f() {\n  // a note\n  return 1;\n}"
        );
    }

    #[test]
    fn a_brace_in_a_string_does_not_indent() {
        assert_eq!(
            formatted("const a = '{';\nconst b = 2;"),
            "const a = '{';\nconst b = 2;"
        );
    }

    #[test]
    fn a_block_comment_keeps_its_own_layout() {
        let source = "/*\n   aligned\n*/\nconst a = 1;";
        assert_eq!(formatted(source), source);
    }

    #[test]
    fn a_multi_line_template_is_left_alone() {
        let source = "const t = `\n   raw\n`;\nconst b = 2;";
        assert_eq!(formatted(source), source);
    }

    #[test]
    fn a_template_interpolation_does_not_unbalance_the_scan() {
        assert_eq!(
            formatted("function f() {\nreturn `${a}`;\n}"),
            "function f() {\n  return `${a}`;\n}"
        );
    }

    #[test]
    fn blank_lines_stay_blank() {
        assert_eq!(
            formatted("function f() {\n\nreturn 1;\n}"),
            "function f() {\n\n  return 1;\n}"
        );
    }

    #[test]
    fn trailing_whitespace_is_trimmed() {
        assert_eq!(formatted("const a = 1;   "), "const a = 1;");
    }

    #[test]
    fn newlines_are_never_moved() {
        // Automatic semicolon insertion depends on them, so the line count is
        // invariant.
        let source = "const a = 1\nconst b = 2\n";
        assert_eq!(formatted(source).lines().count(), source.lines().count());
    }

    #[test]
    fn tabs_are_available() {
        assert_eq!(
            format_source("if (x) {\na();\n}", &FormatOptions { indent_width: 0 }),
            "if (x) {\n\ta();\n}"
        );
    }
}
