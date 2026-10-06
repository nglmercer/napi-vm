//! Bounded, declarative language-service metadata.
//!
//! The runtime and the static LSP manifest use the same JSON representation.
//! This module is deliberately independent of the VM: metadata describes
//! editor-visible shapes only and is never executed.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Maximum nesting depth accepted for a declarative shape.
pub const MAX_SHAPE_DEPTH: usize = 8;
/// Maximum properties on one object shape.
pub const MAX_SHAPE_PROPERTIES: usize = 256;
/// Maximum globals in one runtime snapshot or manifest.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_GLOBALS: usize = 256;
/// Maximum parameters on one function shape.
pub const MAX_PARAMETERS: usize = 64;
/// Maximum length of a global/property/parameter name.
pub const MAX_NAME_LENGTH: usize = 128;
/// Maximum length of a legacy display type string (`returns`, `typeName`).
/// These are free-form descriptive names rather than a bounded vocabulary, so
/// they need their own ceiling before they are stored and rendered in hovers.
pub const MAX_TYPE_NAME_LENGTH: usize = 256;
/// Maximum documentation length in bytes.
pub const MAX_DOCUMENTATION_BYTES: usize = 16 * 1024;
/// Maximum recursive shape nodes in one global declaration.
pub const MAX_SCHEMA_NODES: usize = 4096;
/// Maximum static manifest file size.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Clamp a legacy display type string (`returns`, `typeName`) arriving from a
/// host.
///
/// These are free-form descriptive names (`User`, `Result<User>`) rather than a
/// bounded vocabulary, so there is nothing to validate against — only a ceiling,
/// so an oversized string cannot be stored and then rendered into every hover.
/// An absent, empty, or oversized value degrades to `"unknown"`, matching how
/// the rest of the legacy path treats metadata it cannot use.
#[doc(hidden)]
pub fn clamp_type_name(value: Option<&str>) -> String {
    match value {
        Some(name) if !name.is_empty() && name.len() <= MAX_TYPE_NAME_LENGTH => name.to_string(),
        _ => "unknown".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalInfo {
    pub name: String,
    pub shape: Shape,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyInfo {
    pub shape: Shape,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParameterInfo {
    pub name: String,
    pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Unknown,
    Any,
    Void,
    Undefined,
    Null,
    Boolean,
    Number,
    String,
    Array(Box<Shape>),
    Promise(Box<Shape>),
    Object(BTreeMap<String, PropertyInfo>),
    Function {
        params: Vec<ParameterInfo>,
        returns: Box<Shape>,
        async_fn: bool,
    },
}

impl Shape {
    pub fn property(&self, name: &str) -> Option<&PropertyInfo> {
        match self {
            Self::Object(properties) => properties.get(name),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetadataError(pub String);

impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for MetadataError {}

#[derive(Debug, Deserialize)]
struct RawGlobal {
    name: String,
    shape: RawType,
    #[serde(default)]
    documentation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawType {
    Shape(Box<RawShape>),
    String(String),
}

#[derive(Debug, Deserialize)]
struct RawShape {
    kind: String,
    #[serde(default)]
    properties: BTreeMap<String, RawType>,
    #[serde(default)]
    items: Option<Box<RawType>>,
    #[serde(default)]
    value: Option<Box<RawType>>,
    #[serde(default)]
    params: Vec<RawParameter>,
    #[serde(default)]
    returns: Option<Box<RawType>>,
    #[serde(default, rename = "async")]
    async_fn: bool,
    #[serde(default)]
    documentation: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawParameter {
    name: String,
    #[serde(rename = "type", default)]
    type_shape: Option<RawType>,
    #[serde(default)]
    shape: Option<RawType>,
    #[serde(rename = "typeName", default)]
    type_name: Option<String>,
}

struct ParsedShape {
    shape: Shape,
    documentation: Option<String>,
}

/// Parse and validate a bounded list of global declarations.
///
/// Only the language server consumes this, and `crate::lsp` is not built for
/// wasm32 — so the manifest path is gated off there along with it.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_globals(value: &serde_json::Value) -> Result<Vec<GlobalInfo>, MetadataError> {
    let raw: Vec<RawGlobal> = serde_json::from_value(value.clone())
        .map_err(|error| MetadataError(format!("invalid globals metadata: {error}")))?;
    if raw.len() > MAX_GLOBALS {
        return Err(MetadataError(format!(
            "too many global declarations (maximum {MAX_GLOBALS})"
        )));
    }
    let mut nodes = 0;
    raw.into_iter()
        .map(|raw| parse_raw_global_with_nodes(raw, &mut nodes))
        .collect()
}

/// Parse and validate a shape used by an observed handler payload.
pub fn parse_shape(value: &serde_json::Value) -> Result<Shape, MetadataError> {
    let raw: RawType = serde_json::from_value(value.clone())
        .map_err(|error| MetadataError(format!("invalid shape metadata: {error}")))?;
    Ok(parse_raw_shape(raw, 0, &mut 0)?.shape)
}

fn parse_raw_global(raw: RawGlobal) -> Result<GlobalInfo, MetadataError> {
    parse_raw_global_with_nodes(raw, &mut 0)
}

fn parse_raw_global_with_nodes(
    raw: RawGlobal,
    nodes: &mut usize,
) -> Result<GlobalInfo, MetadataError> {
    validate_name(&raw.name, "global")?;
    let documentation = validate_documentation(raw.documentation, "global")?;
    let parsed = parse_raw_shape(raw.shape, 0, nodes)?;
    Ok(GlobalInfo {
        name: raw.name,
        shape: parsed.shape,
        documentation: documentation.or(parsed.documentation),
    })
}

fn parse_raw_shape(
    raw: RawType,
    depth: usize,
    nodes: &mut usize,
) -> Result<ParsedShape, MetadataError> {
    if depth > MAX_SHAPE_DEPTH {
        return Err(MetadataError(format!(
            "shape exceeds maximum depth of {MAX_SHAPE_DEPTH}"
        )));
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or_else(|| MetadataError("shape node count overflow".into()))?;
    if *nodes > MAX_SCHEMA_NODES {
        return Err(MetadataError(format!(
            "shape exceeds maximum node count of {MAX_SCHEMA_NODES}"
        )));
    }

    match raw {
        RawType::String(value) => Ok(ParsedShape {
            shape: parse_legacy_shape(&value, depth, nodes)?,
            documentation: None,
        }),
        RawType::Shape(raw) => parse_raw_shape_object(*raw, depth, nodes),
    }
}

fn parse_raw_shape_object(
    raw: RawShape,
    depth: usize,
    nodes: &mut usize,
) -> Result<ParsedShape, MetadataError> {
    let documentation = validate_documentation(raw.documentation.clone(), "shape")?;
    let shape = match raw.kind.as_str() {
        "unknown" => Shape::Unknown,
        "any" => Shape::Any,
        "void" => Shape::Void,
        "undefined" => Shape::Undefined,
        "null" => Shape::Null,
        "boolean" => Shape::Boolean,
        "number" => Shape::Number,
        "string" => Shape::String,
        "array" => {
            reject_shape_fields(&raw, false, true, false, "array")?;
            if raw.items.is_some() && raw.value.is_some() {
                return Err(MetadataError(
                    "array shape has duplicate item fields".into(),
                ));
            }
            let items = raw
                .items
                .or(raw.value)
                .map(|items| parse_raw_shape(*items, depth + 1, nodes))
                .transpose()?
                .map(|parsed| parsed.shape)
                .unwrap_or(Shape::Unknown);
            Shape::Array(Box::new(items))
        }
        "promise" => {
            reject_shape_fields(&raw, false, true, false, "promise")?;
            if raw.items.is_some() && raw.value.is_some() {
                return Err(MetadataError(
                    "promise shape has duplicate item fields".into(),
                ));
            }
            let value = raw
                .items
                .or(raw.value)
                .map(|value| parse_raw_shape(*value, depth + 1, nodes))
                .transpose()?
                .map(|parsed| parsed.shape)
                .unwrap_or(Shape::Unknown);
            Shape::Promise(Box::new(value))
        }
        "object" => {
            if raw.properties.len() > MAX_SHAPE_PROPERTIES {
                return Err(MetadataError(format!(
                    "object has too many properties (maximum {MAX_SHAPE_PROPERTIES})"
                )));
            }
            if !raw.params.is_empty()
                || raw.returns.is_some()
                || raw.items.is_some()
                || raw.value.is_some()
            {
                return Err(MetadataError(
                    "object shape contains function/array fields".into(),
                ));
            }
            let mut properties = BTreeMap::new();
            for (name, property) in raw.properties {
                validate_name(&name, "property")?;
                let parsed = parse_raw_shape(property, depth + 1, nodes)?;
                properties.insert(
                    name,
                    PropertyInfo {
                        shape: parsed.shape,
                        documentation: parsed.documentation,
                    },
                );
            }
            Shape::Object(properties)
        }
        "function" => {
            if raw.params.len() > MAX_PARAMETERS {
                return Err(MetadataError(format!(
                    "function has too many parameters (maximum {MAX_PARAMETERS})"
                )));
            }
            if !raw.properties.is_empty() || raw.items.is_some() || raw.value.is_some() {
                return Err(MetadataError(
                    "function shape contains object/array fields".into(),
                ));
            }
            let mut params = Vec::with_capacity(raw.params.len());
            for parameter in raw.params {
                validate_name(&parameter.name, "parameter")?;
                let raw_type = parameter
                    .type_shape
                    .or(parameter.shape)
                    .or_else(|| parameter.type_name.map(RawType::String))
                    .unwrap_or(RawType::Shape(Box::new(RawShape {
                        kind: "unknown".into(),
                        properties: BTreeMap::new(),
                        items: None,
                        value: None,
                        params: Vec::new(),
                        returns: None,
                        async_fn: false,
                        documentation: None,
                    })));
                params.push(ParameterInfo {
                    name: parameter.name,
                    shape: parse_raw_shape(raw_type, depth + 1, nodes)?.shape,
                });
            }
            let returns = raw
                .returns
                .map(|returns| parse_raw_shape(*returns, depth + 1, nodes))
                .transpose()?
                .map(|parsed| parsed.shape)
                .unwrap_or(Shape::Unknown);
            Shape::Function {
                params,
                returns: Box::new(returns),
                async_fn: raw.async_fn,
            }
        }
        other => return Err(MetadataError(format!("unsupported shape kind: {other}"))),
    };
    Ok(ParsedShape {
        shape,
        documentation,
    })
}

fn reject_shape_fields(
    raw: &RawShape,
    _allow_properties: bool,
    allow_items: bool,
    allow_returns: bool,
    kind: &str,
) -> Result<(), MetadataError> {
    if !raw.properties.is_empty()
        || !raw.params.is_empty()
        || (!allow_items && (raw.items.is_some() || raw.value.is_some()))
        || (!allow_returns && raw.returns.is_some())
    {
        return Err(MetadataError(format!(
            "{kind} shape contains invalid fields"
        )));
    }
    Ok(())
}

fn parse_legacy_shape(
    value: &str,
    depth: usize,
    nodes: &mut usize,
) -> Result<Shape, MetadataError> {
    if depth > MAX_SHAPE_DEPTH {
        return Err(MetadataError(format!(
            "shape exceeds maximum depth of {MAX_SHAPE_DEPTH}"
        )));
    }
    let value = value.trim();
    if let Some(inner) = value.strip_suffix("[]") {
        *nodes += 1;
        if *nodes > MAX_SCHEMA_NODES {
            return Err(MetadataError(format!(
                "shape exceeds maximum node count of {MAX_SCHEMA_NODES}"
            )));
        }
        return Ok(Shape::Array(Box::new(parse_legacy_shape(
            inner,
            depth + 1,
            nodes,
        )?)));
    }
    if let Some(inner) = value
        .strip_prefix("Promise<")
        .and_then(|value| value.strip_suffix('>'))
    {
        *nodes += 1;
        if *nodes > MAX_SCHEMA_NODES {
            return Err(MetadataError(format!(
                "shape exceeds maximum node count of {MAX_SCHEMA_NODES}"
            )));
        }
        return Ok(Shape::Promise(Box::new(parse_legacy_shape(
            inner,
            depth + 1,
            nodes,
        )?)));
    }
    Ok(match value {
        "unknown" => Shape::Unknown,
        "any" => Shape::Any,
        "void" => Shape::Void,
        "undefined" => Shape::Undefined,
        "null" => Shape::Null,
        "boolean" => Shape::Boolean,
        "number" => Shape::Number,
        "string" => Shape::String,
        "object" => Shape::Object(BTreeMap::new()),
        "function" => Shape::Function {
            params: Vec::new(),
            returns: Box::new(Shape::Unknown),
            async_fn: false,
        },
        _ => {
            return Err(MetadataError(format!(
                "unsupported legacy shape string: {value}"
            )));
        }
    })
}

fn validate_name(name: &str, kind: &str) -> Result<(), MetadataError> {
    if name.is_empty() || name.len() > MAX_NAME_LENGTH || name.chars().any(char::is_control) {
        return Err(MetadataError(format!("invalid {kind} name")));
    }
    Ok(())
}

fn validate_documentation(
    documentation: Option<String>,
    kind: &str,
) -> Result<Option<String>, MetadataError> {
    if documentation
        .as_ref()
        .is_some_and(|value| value.len() > MAX_DOCUMENTATION_BYTES)
    {
        return Err(MetadataError(format!(
            "{kind} documentation exceeds maximum length"
        )));
    }
    Ok(documentation)
}

impl<'de> serde::Deserialize<'de> for GlobalInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawGlobal::deserialize(deserializer)?;
        parse_raw_global(raw).map_err(serde::de::Error::custom)
    }
}

impl<'de> serde::Deserialize<'de> for Shape {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawType::deserialize(deserializer)?;
        parse_raw_shape(raw, 0, &mut 0)
            .map(|parsed| parsed.shape)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The exact boundary the Node validator in `runtime/session.cjs` must
    /// mirror. When the two disagreed, `registerGlobal()` reported success and
    /// the LSP then rejected the whole globals collection, silently dropping
    /// the runtime snapshot. Keep this in step with the `legacy string shapes
    /// enforce the same depth limit as structured shapes` test there.
    #[test]
    fn legacy_string_shapes_share_the_structured_depth_limit() {
        let legacy = |wrappers: usize| {
            parse_globals(&json!([{
                "name": "g",
                "shape": "string".to_string() + &"[]".repeat(wrappers),
            }]))
        };
        let structured = |wrappers: usize| {
            let mut shape = json!({ "kind": "string" });
            for _ in 0..wrappers {
                shape = json!({ "kind": "array", "items": shape });
            }
            parse_globals(&json!([{ "name": "g", "shape": shape }]))
        };

        for wrappers in 0..=MAX_SHAPE_DEPTH {
            assert!(legacy(wrappers).is_ok(), "legacy depth {wrappers} rejected");
            assert!(
                structured(wrappers).is_ok(),
                "structured depth {wrappers} rejected"
            );
        }
        // The string form is flat in memory, so it can be pushed far past the
        // limit — this is the shape that used to slip through the Node
        // validator entirely.
        for wrappers in [MAX_SHAPE_DEPTH + 1, 64, 5000] {
            assert!(
                legacy(wrappers).is_err(),
                "legacy depth {wrappers} accepted"
            );
        }
        // Structured shapes stay modest here: nesting a `serde_json::Value`
        // thousands deep would overflow the stack building the test input
        // itself, well before this parser sees it.
        for wrappers in [MAX_SHAPE_DEPTH + 1, 64] {
            assert!(
                structured(wrappers).is_err(),
                "structured depth {wrappers} accepted"
            );
        }
    }

    #[test]
    fn parses_recursive_function_metadata() {
        let global = parse_globals(&json!([{
            "name": "api",
            "shape": {
                "kind": "object",
                "properties": {
                    "fetch": {
                        "kind": "function",
                        "params": [{"name": "id", "type": {"kind": "string"}}],
                        "returns": {"kind": "unknown"},
                        "async": true,
                        "documentation": "Fetch one record."
                    }
                }
            }
        }]))
        .unwrap();
        let Shape::Object(properties) = &global[0].shape else {
            panic!("expected object shape");
        };
        let Some(PropertyInfo {
            shape: Shape::Function {
                params, async_fn, ..
            },
            documentation,
        }) = properties.get("fetch")
        else {
            panic!("expected function property");
        };
        assert_eq!(params[0].name, "id");
        assert!(*async_fn);
        assert_eq!(documentation.as_deref(), Some("Fetch one record."));
    }

    #[test]
    fn accepts_legacy_string_shapes_inside_objects() {
        let globals = parse_globals(&json!([{
            "name": "legacy",
            "shape": {
                "kind": "object",
                "properties": {
                    "value": "string",
                    "items": { "kind": "array", "items": "number" }
                }
            }
        }]))
        .unwrap();
        let Shape::Object(properties) = &globals[0].shape else {
            panic!("expected object shape");
        };
        assert_eq!(properties["value"].shape, Shape::String);
        assert_eq!(
            properties["items"].shape,
            Shape::Array(Box::new(Shape::Number))
        );
    }

    #[test]
    fn rejects_malformed_and_oversized_shapes() {
        assert!(parse_shape(&json!({"kind": "banana"})).is_err());
        assert!(
            parse_shape(&json!({
                "kind": "function",
                "params": (0..=MAX_PARAMETERS)
                    .map(|i| json!({"name": format!("p{i}"), "type": {"kind": "unknown"}}))
                    .collect::<Vec<_>>()
            }))
            .is_err()
        );
        assert!(
            parse_shape(&json!({
                "kind": "string",
                "documentation": "x".repeat(MAX_DOCUMENTATION_BYTES + 1)
            }))
            .is_err()
        );
    }

    #[test]
    fn bounds_global_count() {
        let globals = (0..=MAX_GLOBALS)
            .map(|i| json!({"name": format!("g{i}"), "shape": {"kind": "unknown"}}))
            .collect::<Vec<_>>();
        assert!(parse_globals(&json!(globals)).is_err());
    }
}
