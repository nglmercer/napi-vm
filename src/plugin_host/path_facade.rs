//! Host implementation of the `node:path` facade operations.

use super::*;

pub(super) fn path_operation(operation: usize, parts: &[String]) -> Value {
    let sep = std::path::MAIN_SEPARATOR;
    let first = parts.first().map(String::as_str).unwrap_or("");
    match operation {
        0 => {
            let joined = parts
                .iter()
                .filter(|part| !part.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(&sep.to_string());
            Value::String(normalize_host_path(&joined))
        }
        1 => Value::String(normalize_host_path(first)),
        2 => Value::String(host_dirname(first)),
        3 => Value::String(host_basename(first, parts.get(1).map(String::as_str))),
        4 => Value::String(host_extname(first)),
        5 => Value::String(host_resolve(parts)),
        6 => Value::String(host_relative(
            first,
            parts.get(1).map(String::as_str).unwrap_or(""),
        )),
        7 => Value::Bool(Path::new(first).is_absolute()),
        _ => Value::Undefined,
    }
}

#[cfg(not(windows))]
pub(super) fn normalize_host_path(path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let converted = path.to_owned();
    let absolute = converted.starts_with(sep);
    let trailing = converted.ends_with(sep);
    let mut segments = Vec::<String>::new();
    for part in converted.split(sep) {
        match part {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|value| value != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..".into());
                }
            }
            value => segments.push(value.into()),
        }
    }
    let mut result = segments.join(&sep.to_string());
    if absolute {
        result.insert(0, sep);
    }
    if result.is_empty() {
        result = if absolute {
            sep.to_string()
        } else {
            ".".into()
        };
    }
    if trailing && result != sep.to_string() && !result.ends_with(sep) {
        result.push(sep);
    }
    result
}

#[cfg(windows)]
pub(super) fn normalize_host_path(path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let converted = path.replace('/', "\\");
    let trailing = converted.ends_with(sep);
    let mut prefix = String::new();
    let body_storage;
    let mut body = converted.as_str();
    let absolute;
    if let Some(rest) = body.strip_prefix("\\\\") {
        let mut pieces = rest.split('\\');
        let server = pieces.next().unwrap_or_default();
        let share = pieces.next().unwrap_or_default();
        if !server.is_empty() && !share.is_empty() {
            prefix = format!("\\\\{server}\\{share}");
            body_storage = pieces.collect::<Vec<_>>().join("\\");
            body = &body_storage;
        } else {
            body = rest;
        }
        absolute = true;
    } else if body.len() >= 2
        && body.as_bytes()[0].is_ascii_alphabetic()
        && body.as_bytes()[1] == b':'
    {
        prefix = body[..2].to_owned();
        body = &body[2..];
        absolute = body.starts_with(sep);
        body = body.trim_start_matches(sep);
    } else {
        absolute = body.starts_with(sep);
        body = body.trim_start_matches(sep);
    }
    let mut segments = Vec::<String>::new();
    for part in body.split(sep) {
        match part {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|value| value != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..".into());
                }
            }
            value => segments.push(value.into()),
        }
    }
    let joined = segments.join(&sep.to_string());
    let mut result = if absolute {
        if prefix.is_empty() {
            format!("{sep}{joined}")
        } else if joined.is_empty() {
            format!("{prefix}{sep}")
        } else {
            format!("{prefix}{sep}{joined}")
        }
    } else {
        format!("{prefix}{joined}")
    };
    if result.is_empty() {
        result = if absolute {
            sep.to_string()
        } else {
            ".".into()
        };
    }
    if trailing && result != sep.to_string() && !result.ends_with(sep) {
        result.push(sep);
    }
    result
}

pub(super) fn host_dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    #[cfg(windows)]
    {
        let normalized = path.replace('/', "\\");
        let trimmed = normalized.trim_end_matches('\\');
        if trimmed.is_empty() {
            return "\\".into();
        }
        if trimmed.len() == 2
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.as_bytes()[1] == b':'
        {
            return format!("{trimmed}\\");
        }
        return Path::new(trimmed)
            .parent()
            .map(|parent| {
                let value = parent.to_string_lossy().into_owned();
                if value.is_empty() { ".".into() } else { value }
            })
            .unwrap_or_else(|| "\\".into());
    }
    #[cfg(not(windows))]
    {
        if path.chars().all(|ch| ch == std::path::MAIN_SEPARATOR) {
            return std::path::MAIN_SEPARATOR.to_string();
        }
        let normalized = path.trim_end_matches(std::path::MAIN_SEPARATOR);
        let Some(index) = normalized.rfind(std::path::MAIN_SEPARATOR) else {
            return ".".into();
        };
        if index == 0 {
            std::path::MAIN_SEPARATOR.to_string()
        } else {
            normalized[..index].to_owned()
        }
    }
}

pub(super) fn host_basename(path: &str, suffix: Option<&str>) -> String {
    #[cfg(windows)]
    let path = path.replace('/', "\\");
    #[cfg(windows)]
    let path = path.as_str();
    let trimmed = path.trim_end_matches(std::path::MAIN_SEPARATOR);
    let mut base = trimmed
        .rsplit(std::path::MAIN_SEPARATOR)
        .next()
        .unwrap_or("");
    if let Some(suffix) = suffix.filter(|suffix| !suffix.is_empty())
        && base.len() > suffix.len()
        && base.ends_with(suffix)
    {
        base = &base[..base.len() - suffix.len()];
    }
    base.to_owned()
}

pub(super) fn host_extname(path: &str) -> String {
    let base = host_basename(path, None);
    if matches!(base.as_str(), "." | "..") {
        return String::new();
    }
    let Some(index) = base.rfind('.') else {
        return String::new();
    };
    if index == 0 {
        String::new()
    } else {
        base[index..].to_owned()
    }
}

pub(super) fn host_resolve(parts: &[String]) -> String {
    let mut resolved = PathBuf::new();
    let mut found_absolute = false;
    for part in parts.iter().rev() {
        if part.is_empty() {
            continue;
        }
        let path = Path::new(part);
        if path.is_absolute() {
            resolved = path.to_path_buf();
            found_absolute = true;
            break;
        }
        resolved = path.join(resolved);
    }
    if !found_absolute {
        resolved = std::env::current_dir().unwrap_or_default().join(resolved);
    }
    normalize_host_path(&resolved.to_string_lossy())
}

pub(super) fn host_relative(from: &str, to: &str) -> String {
    let from = host_resolve(&[from.to_owned()]);
    let to = host_resolve(&[to.to_owned()]);
    let from_parts: Vec<_> = Path::new(&from).components().collect();
    let to_parts: Vec<_> = Path::new(&to).components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = vec!["..".to_owned(); from_parts.len() - common];
    relative.extend(
        to_parts[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    relative.join(std::path::MAIN_SEPARATOR_STR)
}
