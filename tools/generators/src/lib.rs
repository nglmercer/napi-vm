//! Shared helpers for the offline table generators: reading `.tar.gz`
//! archives, Python-compatible JSON string literals, and writing outputs.

use flate2::read::GzDecoder;
use std::io::Read;
use std::path::Path;

/// A regular file inside an archive.
pub struct Member {
    pub name: String,
    pub data: Vec<u8>,
}

/// Read every regular file of a gzip-compressed tar archive, in archive order.
pub fn read_tgz(path: &Path) -> Result<Vec<Member>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut tar = Vec::new();
    GzDecoder::new(&bytes[..])
        .read_to_end(&mut tar)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    parse_tar(&tar)
}

fn field_text(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn field_number(field: &[u8]) -> Result<usize, String> {
    let text = field_text(field);
    let text = text.trim_matches(|c: char| c == ' ' || c == '\0');
    if text.is_empty() {
        return Ok(0);
    }
    usize::from_str_radix(text, 8).map_err(|error| format!("bad tar size {text:?}: {error}"))
}

fn pax_path(body: &[u8]) -> Option<String> {
    // Records are "<length> <key>=<value>\n".
    let text = String::from_utf8_lossy(body);
    text.lines().find_map(|record| {
        record
            .split_once(' ')?
            .1
            .strip_prefix("path=")
            .map(str::to_string)
    })
}

/// Parse a ustar stream, including GNU long names and pax path records.
pub fn parse_tar(data: &[u8]) -> Result<Vec<Member>, String> {
    let mut members = Vec::new();
    let mut offset = 0;
    let mut long_name = None;
    let mut next_path = None;
    while offset + 512 <= data.len() {
        let header = &data[offset..offset + 512];
        if header.iter().all(|&byte| byte == 0) {
            break;
        }
        let size = field_number(&header[124..136])?;
        let kind = header[156];
        let ustar = &header[257..262] == b"ustar";
        let prefix = if ustar {
            field_text(&header[345..500])
        } else {
            String::new()
        };
        let plain = field_text(&header[0..100]);
        let joined = if prefix.is_empty() {
            plain
        } else {
            format!("{prefix}/{plain}")
        };
        offset += 512;
        let body = data
            .get(offset..offset + size)
            .ok_or("truncated tar member")?;
        offset += size.div_ceil(512) * 512;
        match kind {
            b'L' => long_name = Some(field_text(body)),
            b'x' => next_path = pax_path(body),
            b'0' | 0 => {
                let name = next_path
                    .take()
                    .or_else(|| long_name.take())
                    .unwrap_or(joined);
                members.push(Member {
                    name,
                    data: body.to_vec(),
                });
            }
            _ => {
                long_name = None;
                next_path = None;
            }
        }
    }
    Ok(members)
}

/// The final path component, as Python's `name.split("/")[-1]`.
pub fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// A string literal exactly as Python's `json.dumps` writes it (ASCII only).
pub fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(ch),
            _ => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// A string literal as Python's `json.dumps(text, ensure_ascii=False)` writes
/// it: non-ASCII characters are kept, control characters are escaped.
pub fn json_string_utf8(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// `gzip.compress(data, mtime=0)` from Python: level 9, zero timestamp.
pub fn gzip_compress(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::new(9));
    encoder.write_all(data).expect("gzip into memory");
    encoder.finish().expect("gzip into memory")
}

/// Lowercase hexadecimal SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Write `text` to `directory/name`, creating the directory first.
pub fn write_output(directory: &Path, name: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let path = directory.join(name);
    std::fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))
}

/// The response body of `url`, fetched with curl.
pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error"])
        .arg(url)
        .output()
        .map_err(|error| format!("curl: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "download failed: {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Fetch `url` into `path` with curl, unless the file already exists.
pub fn download_once(url: &str, path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    eprintln!("downloading {url}");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let status = std::process::Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(path)
        .arg(url)
        .status()
        .map_err(|error| format!("curl: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("download failed: {url}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_match_python() {
        assert_eq!(json_string("Europe/Prague"), "\"Europe/Prague\"");
        assert_eq!(json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json_string("é"), "\"\\u00e9\"");
        assert_eq!(json_string("😀"), "\"\\ud83d\\ude00\"");
    }

    #[test]
    fn tar_reads_names_and_skips_directories() {
        let mut archive = Vec::new();
        let header = |name: &str, size: usize, kind: u8| {
            let mut block = [0u8; 512];
            block[..name.len()].copy_from_slice(name.as_bytes());
            let octal = format!("{size:011o}");
            block[124..135].copy_from_slice(octal.as_bytes());
            block[156] = kind;
            block[257..262].copy_from_slice(b"ustar");
            block
        };
        archive.extend_from_slice(&header("dir/", 0, b'5'));
        archive.extend_from_slice(&header("pkg/zone.tab", 5, b'0'));
        let mut body = [0u8; 512];
        body[..5].copy_from_slice(b"hello");
        archive.extend_from_slice(&body);
        archive.extend_from_slice(&[0u8; 1024]);
        let members = parse_tar(&archive).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "pkg/zone.tab");
        assert_eq!(members[0].data, b"hello");
    }
}
