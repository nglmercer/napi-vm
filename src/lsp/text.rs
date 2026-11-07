//! Position and URI conversions between LSP and the Rust core.
//!
//! Two encodings meet here. LSP positions are `(line, character)` pairs where
//! `character` counts **UTF-16 code units** (the protocol default, and what
//! Zed sends), while `crate::lang` works with **UTF-8 byte offsets** into the
//! document and reports diagnostics in 1-based **character** (Unicode scalar)
//! columns, because the lexer walks `chars()`. Every conversion between those
//! three worlds lives in this module so the server itself stays transport-only.

use std::path::PathBuf;

use url::Url;

/// Resolve a `file://` URI to a native path.
///
/// Handles percent-encoding (`My%20Project`) and Windows drive letters
/// (`file:///C:/Users/...`), both of which a naive `strip_prefix("file://")`
/// gets wrong. Non-URI inputs are passed through as plain paths so that
/// callers that already hold a path keep working.
pub fn uri_path(uri: &str) -> PathBuf {
    let Ok(url) = Url::parse(uri) else {
        return PathBuf::from(uri);
    };
    if let Ok(path) = url.to_file_path() {
        return path;
    }
    if url.scheme() != "file" {
        return PathBuf::from(uri);
    }
    // `to_file_path` refuses some well-formed `file://` URIs — notably a
    // POSIX-style `file:///home/...` when running on Windows, which has no
    // drive letter. The decoded path is still the best answer available, and
    // it must never leave `%20` in a path we then hash as the workspace root.
    PathBuf::from(percent_decode(url.path()))
}

/// Decode `%XX` escapes. Invalid escapes are left as literal text, and bytes
/// that do not form valid UTF-8 are replaced rather than dropped.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match (bytes[index], bytes.get(index + 1), bytes.get(index + 2)) {
            (b'%', Some(&high), Some(&low)) => match hex_pair(high, low) {
                Some(byte) => {
                    out.push(byte);
                    index += 3;
                }
                None => {
                    out.push(bytes[index]);
                    index += 1;
                }
            },
            _ => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    Some((hex_digit(high)? << 4) | hex_digit(low)?)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Byte offset of the start of `line` (0-based), clamped to `text.len()`.
fn line_start(text: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    let mut seen = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            seen += 1;
            if seen == line {
                return Some(index + 1);
            }
        }
    }
    None
}

/// The text of `line` (0-based), without its trailing `\r\n` / `\n`.
pub fn line_text(text: &str, line: usize) -> Option<&str> {
    let start = line_start(text, line)?;
    Some(text_at_line_start(&text[start..]))
}

/// Slice one line after its start has already been found. Position conversion
/// must not rescan the document prefix just to find the same line again.
fn text_at_line_start(rest: &str) -> &str {
    let end = rest.find('\n').unwrap_or(rest.len());
    rest[..end].strip_suffix('\r').unwrap_or(&rest[..end])
}

/// Convert a UTF-16 code-unit column into a UTF-8 byte offset within `line`.
///
/// Columns that land inside a surrogate pair (an emoji is two UTF-16 units)
/// resolve to the start of that character rather than to a byte offset that
/// would split it.
pub fn utf16_column_to_byte_offset(line: &str, utf16_column: usize) -> usize {
    let mut units = 0;
    for (byte_index, ch) in line.char_indices() {
        let next = units + ch.len_utf16();
        if next > utf16_column {
            return byte_index;
        }
        units = next;
    }
    line.len()
}

/// Convert a 0-based Unicode-scalar column into a UTF-16 code-unit column.
pub fn char_column_to_utf16(line: &str, char_column: usize) -> usize {
    line.chars().take(char_column).map(char::len_utf16).sum()
}

/// Convert an LSP `(line, character)` position into a UTF-8 byte offset.
///
/// Out-of-range lines clamp to the end of the document and out-of-range
/// columns clamp to the end of their line, matching how editors send a
/// position for a cursor sitting past the last character.
pub fn position_to_offset(text: &str, line: usize, utf16_column: usize) -> usize {
    let Some(start) = line_start(text, line) else {
        return text.len();
    };
    let line = text_at_line_start(&text[start..]);
    start + utf16_column_to_byte_offset(line, utf16_column)
}

/// Convert a UTF-16 column into a character column — the inverse of
/// [`char_column_to_utf16`], for mapping an editor position onto a source
/// offset the analysis understands.
pub fn utf16_column_to_char_column(line: &str, utf16_column: usize) -> usize {
    let mut units = 0usize;
    for (chars, ch) in line.chars().enumerate() {
        if units >= utf16_column {
            return chars;
        }
        units += ch.len_utf16();
    }
    line.chars().count()
}

/// Convert a `crate::lang` diagnostic location (1-based line, 1-based
/// character column) into an LSP 0-based `(line, utf16_character)` position.
pub fn diagnostic_position(text: &str, line: usize, col: usize) -> (usize, usize) {
    let line = line.saturating_sub(1);
    let char_column = col.saturating_sub(1);
    let character = char_column_to_utf16(line_text(text, line).unwrap_or(""), char_column);
    (line, character)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_escapes_decode_and_survive_invalid_input() {
        assert_eq!(percent_decode("/My%20Project"), "/My Project");
        assert_eq!(percent_decode("/%C3%A1ccents"), "/áccents");
        // Truncated or non-hex escapes stay literal instead of eating bytes.
        assert_eq!(percent_decode("/100%"), "/100%");
        assert_eq!(percent_decode("/100%zz"), "/100%zz");
        assert_eq!(percent_decode("/a%2"), "/a%2");
    }

    // The POSIX-style URIs below take `Url::to_file_path` on unix and the
    // percent-decoding fallback on Windows, where they have no drive letter.
    #[test]
    fn plain_uri_round_trips() {
        assert_eq!(
            uri_path("file:///home/user/project"),
            PathBuf::from("/home/user/project")
        );
    }

    #[test]
    fn percent_encoded_uri_is_decoded() {
        assert_eq!(
            uri_path("file:///home/user/My%20Project"),
            PathBuf::from("/home/user/My Project")
        );
        assert_eq!(
            uri_path("file:///home/user/%C3%A1ccents"),
            PathBuf::from("/home/user/áccents")
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_uri_becomes_native_path() {
        assert_eq!(
            uri_path("file:///C:/Users/user/My%20Project"),
            PathBuf::from(r"C:\Users\user\My Project")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn windows_uri_parses_without_panicking() {
        // `to_file_path` cannot produce a native Windows path on unix; the
        // point of the test is that parsing still succeeds and decodes.
        assert!(
            uri_path("file:///C:/Users/user/My%20Project")
                .to_string_lossy()
                .contains("My Project")
        );
    }

    #[test]
    fn no_platform_leaves_escapes_in_a_file_path() {
        // Whichever branch a platform takes, an escape must never reach the
        // filesystem: the workspace root is hashed, so `%20` would silently
        // break runtime discovery.
        for uri in [
            "file:///home/user/My%20Project",
            "file:///C:/Users/user/My%20Project",
            "file://localhost/home/user/My%20Project",
            "file:///home/user/%C3%A1ccents",
        ] {
            let path = uri_path(uri);
            assert!(
                !path.to_string_lossy().contains('%'),
                "{uri} resolved to {path:?}"
            );
        }
    }

    #[test]
    fn non_uri_input_passes_through() {
        assert_eq!(uri_path("/plain/path"), PathBuf::from("/plain/path"));
    }

    #[test]
    fn ascii_positions_match_byte_offsets() {
        let text = "const a = 1;\nevent.data.";
        assert_eq!(position_to_offset(text, 0, 0), 0);
        assert_eq!(position_to_offset(text, 1, 0), 13);
        assert_eq!(position_to_offset(text, 1, 11), text.len());
    }

    #[test]
    fn utf16_columns_account_for_multibyte_characters() {
        // `é` and `ñ` are 2 UTF-8 bytes but 1 UTF-16 unit each.
        let text = "const s = \"héllo ñ\";\nevent.data.";
        // 13 UTF-16 units == 13 characters == 14 UTF-8 bytes (`é` is 2 bytes).
        let offset = position_to_offset(text, 0, 13);
        assert_eq!(&text[..offset], "const s = \"hé");
        assert_eq!(offset, 14);
        // The second line is unaffected by the first line's byte inflation.
        assert_eq!(position_to_offset(text, 1, 11), text.len());
    }

    #[test]
    fn utf16_columns_account_for_surrogate_pairs() {
        // `🔥` is 4 UTF-8 bytes and 2 UTF-16 units.
        let line = "const icon = \"🔥\";";
        let text = format!("{line}\nevent.data.");
        // Column 16 sits right after the emoji's two UTF-16 units.
        let offset = position_to_offset(&text, 0, 16);
        assert_eq!(&text[..offset], "const icon = \"🔥");
        // A column inside the surrogate pair clamps to the emoji's start.
        let inside = position_to_offset(&text, 0, 15);
        assert_eq!(&text[..inside], "const icon = \"");
        assert_eq!(position_to_offset(&text, 1, 11), text.len());
    }

    #[test]
    fn cjk_columns_are_single_units() {
        let text = "const jp = \"日本語\";\nevent.data.";
        let offset = position_to_offset(text, 0, 15);
        assert_eq!(&text[..offset], "const jp = \"日本語");
    }

    #[test]
    fn out_of_range_positions_clamp() {
        let text = "a\nb";
        assert_eq!(position_to_offset(text, 9, 0), text.len());
        assert_eq!(position_to_offset(text, 0, 99), 1);
    }

    #[test]
    fn crlf_lines_exclude_the_carriage_return() {
        let text = "const a = 1;\r\nevent.data.";
        assert_eq!(position_to_offset(text, 0, 12), 12);
        assert_eq!(position_to_offset(text, 1, 0), 14);
    }

    #[test]
    fn positions_preserve_line_and_unicode_boundaries() {
        let text = "prefix\r\n\r\na🔥é\r\n";
        let start = "prefix\r\n\r\n".len();
        assert_eq!(position_to_offset(text, 1, 9), "prefix\r\n".len());
        assert_eq!(position_to_offset(text, 2, 0), start);
        assert_eq!(position_to_offset(text, 2, 1), start + 1);
        assert_eq!(position_to_offset(text, 2, 2), start + 1);
        assert_eq!(position_to_offset(text, 2, 3), start + "a🔥".len());
        assert_eq!(
            position_to_offset(text, 2, usize::MAX),
            start + "a🔥é".len()
        );
        assert_eq!(position_to_offset(text, 3, 0), text.len());
        assert_eq!(position_to_offset(text, 4, 0), text.len());
        assert_eq!(position_to_offset("", 0, 0), 0);
        assert_eq!(position_to_offset("a\r", 0, 99), 1);
    }

    #[test]
    fn diagnostic_columns_convert_to_utf16() {
        let text = "const icon = \"🔥\"; (";
        // The lexer counts characters, so the `(` is character column 19.
        let (line, character) = diagnostic_position(text, 1, 19);
        assert_eq!(line, 0);
        // …which is UTF-16 column 19 as well, because the emoji adds one unit
        // to the 18 preceding characters.
        assert_eq!(character, 19);
        assert_eq!(position_to_offset(text, line, character), text.len() - 1);
    }
}
