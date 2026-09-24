//! Shared file, path, and identifier helpers.

use super::*;

pub(super) fn read_limited(path: &Path, max_file_bytes: u64) -> Result<Vec<u8>, PluginHostError> {
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options
        .open(path)
        .map_err(|error| PluginHostError::Io(format!("cannot open plugin file: {error}")))?;
    let metadata = file
        .metadata()
        .map_err(|error| PluginHostError::Io(format!("cannot inspect plugin file: {error}")))?;
    if !metadata.is_file() {
        return Err(PluginHostError::Load(
            "plugin source is not a regular file".into(),
        ));
    }
    if metadata.len() > max_file_bytes {
        return Err(PluginHostError::Load(format!(
            "plugin file exceeds the {max_file_bytes} byte read limit"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| PluginHostError::Io(format!("cannot read plugin file: {error}")))?;
    if bytes.len() as u64 > max_file_bytes {
        return Err(PluginHostError::Load(format!(
            "plugin file exceeds the {max_file_bytes} byte read limit"
        )));
    }
    Ok(bytes)
}

pub(super) fn valid_plugin_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

pub(super) fn validate_capability_name(name: &str) -> Result<(), PluginHostError> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(PluginHostError::Manifest(format!(
            "invalid capability name \"{name}\""
        )));
    }
    Ok(())
}

pub(super) fn is_js_identifier(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "new",
        "null",
        "return",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "let",
        "enum",
        "implements",
        "interface",
        "package",
        "private",
        "protected",
        "public",
        "static",
    ];
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
        && !RESERVED.contains(&name)
}

pub(super) fn guest_path_is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.starts_with("//") || is_drive_path(path)
}

pub(super) fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

pub(super) fn normalize_guest_path(path: &str, absolute: bool) -> String {
    let mut segments = Vec::<&str>::new();
    let mut prefix = "";
    let mut body = path;
    if is_drive_path(path) {
        prefix = &path[..2];
        body = &path[2..];
    }
    for segment in body.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|part| *part != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..");
                }
            }
            value => segments.push(value),
        }
    }
    let joined = segments.join("/");
    if absolute {
        if prefix.is_empty() {
            format!("/{joined}")
        } else {
            format!("{prefix}/{joined}")
        }
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

pub(super) fn path_pattern_matches(pattern: &str, candidate: &str) -> bool {
    let pattern_segments: Vec<_> = pattern.trim_matches('/').split('/').collect();
    let candidate_segments: Vec<_> = candidate.trim_matches('/').split('/').collect();
    fn match_segments(pattern: &[&str], candidate: &[&str]) -> bool {
        if pattern.is_empty() {
            return candidate.is_empty();
        }
        if pattern[0] == "**" {
            return match_segments(&pattern[1..], candidate)
                || (!candidate.is_empty() && match_segments(pattern, &candidate[1..]));
        }
        !candidate.is_empty()
            && wildcard_match(pattern[0], candidate[0])
            && match_segments(&pattern[1..], &candidate[1..])
    }
    match_segments(&pattern_segments, &candidate_segments)
}

pub(super) fn wildcard_match(pattern: &str, candidate: &str) -> bool {
    let parts: Vec<_> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == candidate;
    }
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            if !candidate[offset..].starts_with(part) {
                return false;
            }
            offset += part.len();
        } else if index + 1 == parts.len() {
            return candidate[offset..].ends_with(part);
        } else if let Some(found) = candidate[offset..].find(part) {
            offset += found + part.len();
        } else {
            return false;
        }
    }
    true
}

#[cfg(unix)]
pub(super) fn set_no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
pub(super) fn set_no_follow(_options: &mut OpenOptions) {}
