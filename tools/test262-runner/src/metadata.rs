//! The subset of Test262 YAML front matter the runner needs: `flags`,
//! `includes`, `features`, and `negative`. Test262 front matter is a flat
//! mapping; lists are inline (`[a, b]`) or block (`- a`), and `negative` is a
//! nested mapping. Other keys (descriptions, `info: |` blocks) are skipped.

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Negative {
    pub phase: String,
    pub error_type: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub flags: Vec<String>,
    pub includes: Vec<String>,
    pub features: Vec<String>,
    pub negative: Option<Negative>,
}

impl Metadata {
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|candidate| candidate == flag)
    }
}

/// One way to run a test: `script`, `strict`, or `module`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Script,
    Strict,
    Module,
}

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Variant::Script => "script",
            Variant::Strict => "strict",
            Variant::Module => "module",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "script" => Some(Variant::Script),
            "strict" => Some(Variant::Strict),
            "module" => Some(Variant::Module),
            _ => None,
        }
    }
}

/// The variants Test262's flags select, in reporting order.
pub fn variants(metadata: &Metadata) -> Vec<Variant> {
    if metadata.has_flag("module") {
        vec![Variant::Module]
    } else if metadata.has_flag("raw") || metadata.has_flag("noStrict") {
        vec![Variant::Script]
    } else if metadata.has_flag("onlyStrict") {
        vec![Variant::Strict]
    } else {
        vec![Variant::Script, Variant::Strict]
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    let stripped = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        });
    stripped.unwrap_or(value).to_string()
}

fn strip_comment(line: &str) -> &str {
    // Front matter values never contain `#` in the keys this parser reads.
    match line.find(" #") {
        Some(index) => &line[..index],
        None => line,
    }
}

fn inline_list(value: &str) -> Result<Vec<String>, String> {
    let inner = value
        .trim()
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| format!("malformed inline list: {value}"))?;
    Ok(inner
        .split(',')
        .map(unquote)
        .filter(|item| !item.is_empty())
        .collect())
}

/// Parse the `/*--- … ---*/` block of a test source.
pub fn parse(source: &str) -> Result<Metadata, String> {
    let start = source
        .find("/*---")
        .ok_or_else(|| "missing Test262 metadata".to_string())?;
    let body_start = start + "/*---".len();
    let end = source[body_start..]
        .find("---*/")
        .ok_or_else(|| "unterminated Test262 metadata".to_string())?;
    let body = &source[body_start..body_start + end];

    let mut metadata = Metadata::default();
    let mut current: Option<String> = None;
    // YAML accepts CR, LF, and CRLF line breaks; some fixtures use bare CR.
    for raw in body.split(['\r', '\n']) {
        let line = strip_comment(raw);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            let Some((key, value)) = line.split_once(':') else {
                // A continuation of a plain multi-line scalar.
                continue;
            };
            let key = key.trim().to_string();
            let value = value.trim();
            current = Some(key.clone());
            let list = match key.as_str() {
                "flags" => Some(&mut metadata.flags),
                "includes" => Some(&mut metadata.includes),
                "features" => Some(&mut metadata.features),
                _ => None,
            };
            if let Some(list) = list {
                if value.starts_with('[') {
                    *list = inline_list(value)?;
                } else if !value.is_empty() {
                    return Err(format!("expected a list for {key}"));
                }
            } else if key == "negative" {
                metadata.negative = Some(Negative::default());
            }
            continue;
        }
        let trimmed = line.trim();
        match current.as_deref() {
            Some(key @ ("flags" | "includes" | "features")) => {
                if let Some(item) = trimmed.strip_prefix('-') {
                    let item = unquote(item);
                    let list = match key {
                        "flags" => &mut metadata.flags,
                        "includes" => &mut metadata.includes,
                        _ => &mut metadata.features,
                    };
                    list.push(item);
                }
            }
            Some("negative") => {
                if let Some((field, value)) = trimmed.split_once(':') {
                    let negative = metadata.negative.get_or_insert_with(Negative::default);
                    match field.trim() {
                        "phase" => negative.phase = unquote(value),
                        "type" => negative.error_type = unquote(value),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(negative) = &metadata.negative
        && (negative.phase.is_empty() || negative.error_type.is_empty())
    {
        return Err("negative metadata requires phase and type".into());
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_and_block_lists() {
        let source = "/*---\ndescription: x\nflags: [onlyStrict, async]\nincludes:\n  - compareArray.js\n  - 'propertyHelper.js'\nfeatures: [Symbol]\n---*/";
        let metadata = parse(source).unwrap();
        assert_eq!(metadata.flags, ["onlyStrict", "async"]);
        assert_eq!(metadata.includes, ["compareArray.js", "propertyHelper.js"]);
        assert_eq!(metadata.features, ["Symbol"]);
        assert_eq!(variants(&metadata), [Variant::Strict]);
    }

    #[test]
    fn negative_mapping_and_multiline_info() {
        let source = "/*---\ninfo: |\n    flags: [raw]\n    not: a key\nnegative:\n  phase: parse\n  type: SyntaxError\n---*/";
        let metadata = parse(source).unwrap();
        assert!(metadata.flags.is_empty());
        assert_eq!(
            metadata.negative,
            Some(Negative {
                phase: "parse".into(),
                error_type: "SyntaxError".into()
            })
        );
        assert_eq!(variants(&metadata), [Variant::Script, Variant::Strict]);
    }

    #[test]
    fn bare_carriage_return_line_breaks() {
        let metadata =
            parse("/*---\rincludes: [nativeFunctionMatcher.js]\rflags: [raw]\r---*/").unwrap();
        assert_eq!(metadata.includes, ["nativeFunctionMatcher.js"]);
        assert_eq!(metadata.flags, ["raw"]);
        let metadata = parse("/*---\r\nincludes:\r\n  - a.js\r\n---*/").unwrap();
        assert_eq!(metadata.includes, ["a.js"]);
    }

    #[test]
    fn block_flags_and_missing_metadata() {
        let metadata = parse("/*---\nflags:\n  - noStrict\n---*/").unwrap();
        assert_eq!(variants(&metadata), [Variant::Script]);
        assert!(parse("// no front matter").is_err());
        assert!(parse("/*---\nnegative:\n  phase: parse\n---*/").is_err());
    }
}
