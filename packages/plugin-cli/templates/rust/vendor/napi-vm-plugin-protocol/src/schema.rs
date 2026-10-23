use crate::{InterfaceIdentity, PluginResult, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Validated contract schema and canonical interface identity shared across languages.
pub struct Contract {
    pub descriptor: Value,
    pub schemas: Value,
    pub digest: String,
}
impl Contract {
    pub fn from_value(value: Value) -> PluginResult<Self> {
        let c: Self = serde_json::from_value(value)?;
        let mut body = serde_json::json!({"descriptor":c.descriptor,"schemas":c.schemas});
        crate::normalize_value(&mut body, 0, 64)?;
        let mut canonical = serde_json_canonicalizer::to_vec(&body)
            .map_err(|e| invalid(&format!("contract canonicalization failed: {e}")))?;
        canonical.push(b'\n');
        use sha2::{Digest, Sha256};
        let actual = format!("{:x}", Sha256::digest(canonical));
        if actual != c.digest {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "generated contract digest does not match its descriptor/schema bytes",
            ));
        }

        if c.descriptor["descriptorVersion"] != 1
            || !identifier(c.id())
            || !version(c.version())
            || c.digest.len() != 64
            || !c
                .digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "invalid contract identity or descriptor version",
            ));
        }
        let d = c
            .descriptor
            .as_object()
            .ok_or_else(|| invalid("descriptor must be object"))?;
        for k in d.keys() {
            if ![
                "descriptorVersion",
                "id",
                "version",
                "types",
                "methods",
                "errors",
                "events",
                "state",
                "description",
                "title",
            ]
            .contains(&k.as_str())
            {
                return Err(invalid(&format!("unknown descriptor keyword {k}")));
            }
        }
        let methods = c.descriptor["methods"]
            .as_object()
            .ok_or_else(|| invalid("methods must be object"))?;
        for category in ["errors", "events"] {
            let entries = c.descriptor[category]
                .as_object()
                .ok_or_else(|| invalid("errors/events must be objects"))?;
            for (name, item) in entries {
                if !member_name(name) {
                    return Err(invalid("invalid descriptor member name"));
                }
                let keys = if category == "errors" {
                    ["data", "description"]
                } else {
                    ["payload", "description"]
                };
                let o = item
                    .as_object()
                    .ok_or_else(|| invalid("descriptor member must be object"))?;
                if o.keys().any(|k| !keys.contains(&k.as_str()))
                    || o.get("description").is_some_and(|v| !v.is_string())
                {
                    return Err(invalid("invalid descriptor member fields"));
                }
            }
        }
        for (name, m) in methods {
            if !member_name(name) {
                return Err(invalid("invalid method name"));
            }
            let o = m
                .as_object()
                .ok_or_else(|| invalid("method descriptor must be object"))?;
            for key in o.keys() {
                if !["input", "output", "errors", "idempotent", "description"]
                    .contains(&key.as_str())
                {
                    return Err(invalid(&format!("unknown method keyword {key}")));
                }
            }
            if m.get("idempotent").is_some_and(|v| !v.is_boolean())
                || m.get("description").is_some_and(|v| !v.is_string())
            {
                return Err(invalid("invalid method metadata"));
            }
            let errors = m["errors"]
                .as_array()
                .ok_or_else(|| invalid("method requires errors array"))?;
            let mut names = BTreeSet::new();
            for e in errors {
                if !e.as_str().is_some_and(|v| names.insert(v)) {
                    return Err(invalid("invalid or duplicate method errors"));
                }
            }
            for k in ["input", "output"] {
                c.schema(
                    m[k].as_str()
                        .ok_or_else(|| invalid("method input/output must be schema refs"))?,
                )?;
            }
            if let Some(es) = m.get("errors") {
                let a = es
                    .as_array()
                    .ok_or_else(|| invalid("method errors must be array"))?;
                for e in a {
                    if c.descriptor["errors"]
                        .get(
                            e.as_str()
                                .ok_or_else(|| invalid("error name must be string"))?,
                        )
                        .is_none()
                    {
                        return Err(invalid("undeclared method error"));
                    }
                }
            }
        }
        if let Some(errors) = c.descriptor.get("errors") {
            for (_, e) in errors
                .as_object()
                .ok_or_else(|| invalid("errors must be object"))?
            {
                c.schema(
                    e["data"]
                        .as_str()
                        .ok_or_else(|| invalid("error data must be schema ref"))?,
                )?;
            }
        }
        if let Some(events) = c.descriptor.get("events") {
            for (_, e) in events
                .as_object()
                .ok_or_else(|| invalid("events must be object"))?
            {
                c.schema(
                    e["payload"]
                        .as_str()
                        .ok_or_else(|| invalid("event payload must be schema ref"))?,
                )?;
            }
        }
        if let Some(s) = c.descriptor.get("state") {
            if s.as_object().is_none_or(|o| {
                o.len() != 3
                    || o.keys()
                        .any(|k| !["contract", "version", "schema"].contains(&k.as_str()))
            }) {
                return Err(invalid("invalid state descriptor fields"));
            }

            if !identifier(s["contract"].as_str().unwrap_or(""))
                || s["version"]
                    .as_u64()
                    .is_none_or(|n| n == 0 || n > 9007199254740991)
            {
                return Err(invalid("invalid state identity"));
            }
            c.schema(
                s["schema"]
                    .as_str()
                    .ok_or_else(|| invalid("state schema must be ref"))?,
            )?;
        }
        let root = c
            .schemas
            .as_object()
            .ok_or_else(|| invalid("schemas root must be an object"))?;
        if root
            .keys()
            .any(|k| !["$defs", "$schema", "title", "description"].contains(&k.as_str()))
            || c.schemas["$defs"]
                .as_object()
                .is_none_or(|d| d.len() > 1024)
        {
            return Err(invalid(
                "schemas requires bounded $defs and document metadata only",
            ));
        }
        check_schema(&c.schemas, &c.schemas, &mut Vec::new(), 0)?;
        Ok(c)
    }
    pub fn id(&self) -> &str {
        self.descriptor["id"].as_str().unwrap_or("")
    }
    pub fn version(&self) -> &str {
        self.descriptor["version"].as_str().unwrap_or("")
    }
    pub fn identity(&self) -> InterfaceIdentity {
        InterfaceIdentity {
            version: self.version().into(),
            digest: self.digest.clone(),
        }
    }
    pub fn schema(&self, reference: &str) -> PluginResult<&Value> {
        resolve(&self.schemas, reference)
    }
    pub fn validate_ref(&self, reference: &str, value: &Value) -> PluginResult<()> {
        validate_at(self.schema(reference)?, value, &self.schemas, "", 0)
    }
    pub fn method(&self, method: &str) -> PluginResult<&Value> {
        self.descriptor["methods"].get(method).ok_or_else(|| {
            RpcError::new(
                "METHOD_NOT_FOUND",
                format!("{}.{method} is not declared", self.id()),
            )
        })
    }
    pub fn input(&self, method: &str, value: &Value) -> PluginResult<()> {
        let m = self.method(method)?;
        self.validate_ref(m["input"].as_str().unwrap_or(""), value)
    }
    pub fn output(&self, method: &str, value: &Value) -> PluginResult<()> {
        let m = self.method(method)?;
        self.validate_ref(m["output"].as_str().unwrap_or(""), value)
            .map_err(|e| RpcError::new("INVALID_RESULT", e.message))
    }
    pub fn error(&self, method: &str, error: &RpcError) -> PluginResult<()> {
        if error.stable_code() != "APPLICATION_ERROR" {
            return Ok(());
        }
        let code = error.data["domainCode"]
            .as_str()
            .ok_or_else(|| RpcError::new("INVALID_RESULT", "missing domainCode"))?;
        if !self.method(method)?["errors"]
            .as_array()
            .is_some_and(|es| es.iter().any(|e| e == code))
        {
            return Err(RpcError::new("INVALID_RESULT", "undeclared domain error"));
        }
        let r = self.descriptor["errors"][code]["data"]
            .as_str()
            .ok_or_else(|| RpcError::new("INVALID_RESULT", "unknown domain error"))?;
        self.validate_ref(r, &error.data["data"])
            .map_err(|e| RpcError::new("INVALID_RESULT", e.message))
    }
    pub fn event(&self, event: &str, payload: &Value) -> PluginResult<()> {
        let r = self.descriptor["events"][event]["payload"]
            .as_str()
            .ok_or_else(|| RpcError::new("INVALID_ARGUMENT", "undeclared event"))?;
        self.validate_ref(r, payload)
    }
    pub fn snapshot(&self, value: &Value) -> PluginResult<()> {
        if value.is_null() {
            return Ok(());
        }
        let o = value.as_object().ok_or_else(|| {
            RpcError::new("STATE_INCOMPATIBLE", "snapshot must be object or null")
        })?;
        let s = &self.descriptor["state"];
        if o.len() != 3
            || value["stateVersion"] != s["version"]
            || value["contract"] != s["contract"]
            || !o.contains_key("data")
            || s.is_null()
        {
            return Err(RpcError::new(
                "STATE_INCOMPATIBLE",
                "snapshot identity does not match generated contract",
            ));
        }
        self.validate_ref(s["schema"].as_str().unwrap_or(""), &value["data"])
            .map_err(|e| RpcError::new("STATE_INCOMPATIBLE", e.message))
    }
}
pub fn identifier(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 {
        return false;
    }
    let mut iter = s.split(['.', '-']);
    let Some(first) = iter.next() else {
        return false;
    };
    !first.is_empty()
        && first.as_bytes()[0].is_ascii_lowercase()
        && first
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && iter.all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}
pub fn member_name(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 200
}

pub fn version(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 {
        return false;
    }
    let (base, build) = s.split_once('+').map_or((s, None), |(a, b)| (a, Some(b)));
    if build.is_some_and(|b| {
        b.split('.')
            .any(|x| x.is_empty() || !x.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    }) {
        return false;
    }
    let (core, pre) = base
        .split_once('-')
        .map_or((base, None), |(a, b)| (a, Some(b)));
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|x| {
            x.is_empty()
                || !x.bytes().all(|b| b.is_ascii_digit())
                || (x.len() > 1 && x.starts_with('0'))
        })
    {
        return false;
    }
    !pre.is_some_and(|p| {
        p.split('.').any(|x| {
            x.is_empty()
                || !x.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || (x.len() > 1 && x.starts_with('0') && x.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}
fn invalid(msg: &str) -> RpcError {
    RpcError::new("INVALID_ARGUMENT", msg)
}
fn resolve<'a>(root: &'a Value, r: &str) -> PluginResult<&'a Value> {
    if !r.starts_with("#/$defs/") {
        return Err(invalid(
            "only bundled local #/$defs/... references are supported",
        ));
    }
    root.pointer(&r[1..])
        .ok_or_else(|| invalid(&format!("unresolved schema reference {r}")))
}
const KEYS: &[&str] = &[
    "$schema",
    "$defs",
    "$ref",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "maxItems",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "enum",
    "const",
    "oneOf",
    "title",
    "description",
    "x-wire-type",
];
fn check_schema(
    s: &Value,
    root: &Value,
    stack: &mut Vec<String>,
    depth: usize,
) -> PluginResult<()> {
    if depth > 64 {
        return Err(invalid("schema reference/depth limit exceeded"));
    }
    let o = s
        .as_object()
        .ok_or_else(|| invalid("schema must be an object"))?;
    if o.get("$schema")
        .is_some_and(|v| v != "https://json-schema.org/draft/2020-12/schema")
    {
        return Err(invalid("schema draft must be 2020-12"));
    }

    for k in ["title", "description"] {
        if o.get(k).is_some_and(|v| !v.is_string()) {
            return Err(invalid("schema metadata must be strings"));
        }
    }
    for k in o.keys() {
        if !KEYS.contains(&k.as_str()) {
            return Err(invalid(&format!("unsupported schema keyword {k}")));
        }
    }
    if let Some(r) = o.get("$ref") {
        let r = r.as_str().ok_or_else(|| invalid("$ref must be string"))?;
        if o.keys()
            .any(|k| !["$ref", "title", "description"].contains(&k.as_str()))
        {
            return Err(invalid("$ref cannot have semantic siblings"));
        }
        if stack.len() >= 32 {
            return Err(invalid("schema reference chain exceeds 32"));
        }
        if stack.iter().any(|v| v == r) {
            return Err(invalid(&format!("recursive schema reference {r}")));
        }
        stack.push(r.into());
        check_schema(resolve(root, r)?, root, stack, depth + 1)?;
        stack.pop();
        return Ok(());
    }
    if let Some(types) = s["type"].as_array() {
        if s.get("enum").is_some() || s.get("const").is_some() {
            return Err(invalid("nullable type arrays cannot combine enum/const"));
        }
        if types.len() != 2
            || types.iter().filter(|v| **v == "null").count() != 1
            || types.iter().any(|t| !t.is_string())
        {
            return Err(invalid(
                "nullable type array requires exactly one type and null",
            ));
        }
        let other = types.iter().find(|t| **t != "null").unwrap();
        let mut nonnull = s.clone();
        nonnull["type"] = other.clone();
        check_schema(&nonnull, root, stack, depth + 1)?;
        return Ok(());
    }
    for key in ["minItems", "maxItems", "minLength", "maxLength"] {
        if let Some(v) = o.get(key)
            && v.as_u64().is_none_or(|n| n > 9007199254740991)
        {
            return Err(invalid("length bounds must be nonnegative integers"));
        }
    }
    for key in ["minimum", "maximum"] {
        if o.get(key)
            .is_some_and(|v| v.as_f64().is_none_or(|v| !v.is_finite()))
        {
            return Err(invalid("numeric bounds must be finite numbers"));
        }
    }
    for (lo, hi) in [
        ("minItems", "maxItems"),
        ("minLength", "maxLength"),
        ("minimum", "maximum"),
    ] {
        if let (Some(a), Some(b)) = (o.get(lo), o.get(hi))
            && a.as_f64().zip(b.as_f64()).is_none_or(|(x, y)| x > y)
        {
            return Err(invalid("conflicting schema bounds"));
        }
    }
    if let Some(defs) = o.get("$defs") {
        for v in defs
            .as_object()
            .ok_or_else(|| invalid("$defs must be object"))?
            .values()
        {
            check_schema(v, root, stack, depth + 1)?;
        }
    }
    if let Some(v) = o.get("oneOf") {
        if o.keys()
            .any(|k| !["oneOf", "title", "description"].contains(&k.as_str()))
        {
            return Err(invalid("oneOf cannot have semantic siblings"));
        }

        let a = v.as_array().ok_or_else(|| invalid("oneOf must be array"))?;
        if a.len() < 2 || a.len() > 32 {
            return Err(invalid("tagged union must contain 2..32 variants"));
        }
        let mut choices = Vec::new();
        for s in a {
            if s.get("$ref").is_some() {
                return Err(invalid("oneOf variants must be inline tagged objects"));
            }
            check_schema(s, root, stack, depth + 1)?;
            let s = if let Some(r) = s["$ref"].as_str() {
                resolve(root, r)?
            } else {
                s
            };
            if s["type"] != "object" {
                return Err(invalid("oneOf requires tagged object variants"));
            }
            choices.push(s);
        }
        let props = choices[0]["properties"]
            .as_object()
            .ok_or_else(|| invalid("tagged union properties missing"))?;
        let valid = props.keys().any(|k| {
            let mut tags = BTreeSet::new();
            choices.iter().all(|s| {
                s["required"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == k))
                    && s["properties"][k]["const"]
                        .as_str()
                        .is_some_and(|v| tags.insert(v.to_string()))
            })
        });
        if !valid {
            return Err(invalid(
                "oneOf needs a shared required string-const discriminator with unique tags",
            ));
        }
        return Ok(());
    }
    if let Some(t) = o.get("type") {
        let t = t
            .as_str()
            .ok_or_else(|| invalid("nullable syntax is type:[nonNullType, null]"))?;
        if ![
            "object", "array", "string", "integer", "number", "boolean", "null",
        ]
        .contains(&t)
        {
            return Err(invalid("unsupported type"));
        }
        if t == "object" {
            if s["additionalProperties"] != false {
                return Err(invalid("objects require additionalProperties:false"));
            }
            let p = s["properties"]
                .as_object()
                .ok_or_else(|| invalid("objects require properties"))?;
            for v in p.values() {
                check_schema(v, root, stack, depth + 1)?;
            }
            if s.get("required").is_none() {
                return Err(invalid("objects require explicit required array"));
            }
            let mut seen = BTreeSet::new();
            if let Some(r) = s.get("required") {
                for k in r
                    .as_array()
                    .ok_or_else(|| invalid("required must be array"))?
                {
                    let k = k
                        .as_str()
                        .ok_or_else(|| invalid("required item must be string"))?;
                    if !p.contains_key(k) || !seen.insert(k) {
                        return Err(invalid("invalid or duplicate required property"));
                    }
                }
            }
        }
        if t == "array" {
            check_schema(&s["items"], root, stack, depth + 1)?;
        }
        for (key, allowed) in [
            ("properties", t == "object"),
            ("required", t == "object"),
            ("additionalProperties", t == "object"),
            ("items", t == "array"),
            ("minItems", t == "array"),
            ("maxItems", t == "array"),
            ("minLength", t == "string"),
            ("maxLength", t == "string"),
            ("minimum", t == "integer" || t == "number"),
            ("maximum", t == "integer" || t == "number"),
        ] {
            if o.contains_key(key) && !allowed {
                return Err(invalid(&format!("{key} conflicts with type {t}")));
            }
        }
    } else if !o.contains_key("$defs") && !o.contains_key("const") && !o.contains_key("enum") {
        return Err(invalid("schema requires explicit type"));
    }
    if s.get("enum").is_some() && s["type"] != "string" {
        return Err(invalid("enum is supported only for strings"));
    }
    if let Some(e) = s.get("enum") {
        let a = e.as_array().ok_or_else(|| invalid("enum must be array"))?;
        let mut seen = BTreeSet::new();
        if a.is_empty() || a.iter().any(|v| v.as_str().is_none_or(|x| !seen.insert(x))) {
            return Err(invalid("only unique nonempty string enums supported"));
        }
    }
    if let Some(w) = s.get("x-wire-type")
        && (s["type"] != "string"
            || !["i64-decimal", "u64-decimal", "bytes-base64", "utc-datetime"]
                .contains(&w.as_str().unwrap_or("")))
    {
        return Err(invalid("unknown or non-string x-wire-type"));
    }
    if let Some(c) = s.get("const") {
        if c.is_object() || c.is_array() {
            return Err(invalid("only primitive literal constants are supported"));
        }
        let mut plain = s.clone();
        plain.as_object_mut().unwrap().remove("const");
        validate_at(&plain, c, root, "const", 0)?;
    }
    if let Some(es) = s["enum"].as_array() {
        let mut plain = s.clone();
        plain.as_object_mut().unwrap().remove("enum");
        for e in es {
            validate_at(&plain, e, root, "enum", 0)?;
        }
    }
    Ok(())
}
pub fn validate(schema: &Value, value: &Value) -> PluginResult<()> {
    check_schema(schema, schema, &mut Vec::new(), 0)?;
    validate_at(schema, value, schema, "", 0)
}
fn validate_at(s: &Value, v: &Value, root: &Value, path: &str, depth: usize) -> PluginResult<()> {
    let fail = |m: &str| invalid(&format!("{path}: {m}"));
    if depth > 64 {
        return Err(fail("maximum nesting exceeded"));
    }
    if let Some(r) = s["$ref"].as_str() {
        return validate_at(resolve(root, r)?, v, root, path, depth + 1);
    }
    if let Some(a) = s["oneOf"].as_array() {
        let n = a
            .iter()
            .filter(|s| validate_at(s, v, root, path, depth + 1).is_ok())
            .count();
        return if n == 1 {
            Ok(())
        } else {
            Err(fail("union variant mismatch"))
        };
    }
    if let Some(types) = s["type"].as_array() {
        if v.is_null() {
            return Ok(());
        }
        let mut nonnull = s.clone();
        nonnull["type"] = types
            .iter()
            .find(|t| **t != "null")
            .cloned()
            .ok_or_else(|| fail("invalid nullable type"))?;
        return validate_at(&nonnull, v, root, path, depth + 1);
    }
    if let Some(c) = s.get("const")
        && c != v
        && !(c.is_number() && v.is_number() && c.as_f64() == v.as_f64())
    {
        return Err(fail("const mismatch"));
    }
    if let Some(e) = s["enum"].as_array()
        && !e.contains(v)
    {
        return Err(fail("enum mismatch"));
    }
    match s["type"].as_str() {
        Some("object") => {
            let o = v.as_object().ok_or_else(|| fail("expected object"))?;
            let p = s["properties"]
                .as_object()
                .ok_or_else(|| fail("invalid object schema"))?;
            if let Some(a) = s["required"].as_array() {
                for k in a {
                    if !o.contains_key(k.as_str().unwrap_or("")) {
                        return Err(fail(&format!("missing required {k}")));
                    }
                }
            }
            for (k, v) in o {
                let schema = p
                    .get(k)
                    .ok_or_else(|| fail(&format!("unknown property {k}")))?;
                validate_at(
                    schema,
                    v,
                    root,
                    &format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                    depth + 1,
                )?;
            }
        }
        Some("array") => {
            let a = v.as_array().ok_or_else(|| fail("expected array"))?;
            length_bounds(s, a.len(), "minItems", "maxItems").map_err(fail)?;
            for (i, v) in a.iter().enumerate() {
                validate_at(&s["items"], v, root, &format!("{path}/{i}"), depth + 1)?;
            }
        }
        Some("string") => {
            let x = v.as_str().ok_or_else(|| fail("expected string"))?;
            length_bounds(s, x.chars().count(), "minLength", "maxLength").map_err(fail)?;
            if let Some(w) = s["x-wire-type"].as_str() {
                let ok = match w {
                    "i64-decimal" => canonical_decimal(x, true) && x.parse::<i64>().is_ok(),
                    "u64-decimal" => canonical_decimal(x, false) && x.parse::<u64>().is_ok(),
                    "bytes-base64" => canonical_base64(x),
                    "utc-datetime" => utc_datetime(x),
                    _ => false,
                };
                if !ok {
                    return Err(fail(&format!("invalid {w}")));
                }
            }
        }
        Some("integer") | Some("number") => {
            let n = v
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| fail("expected finite number"))?;
            if s["type"] == "integer" && (n.fract() != 0.0 || n.abs() > 9007199254740991.0) {
                return Err(fail("integer outside safe range"));
            }
            if s["minimum"].as_f64().is_some_and(|m| n < m)
                || s["maximum"].as_f64().is_some_and(|m| n > m)
            {
                return Err(fail("numeric bound exceeded"));
            }
        }
        Some("boolean") => {
            if !v.is_boolean() {
                return Err(fail("expected boolean"));
            }
        }
        Some("null") => {
            if !v.is_null() {
                return Err(fail("expected null"));
            }
        }
        None => {}
        _ => return Err(fail("unsupported schema type")),
    }
    Ok(())
}
fn length_bounds(s: &Value, n: usize, lo: &str, hi: &str) -> Result<(), &'static str> {
    if s[lo].as_u64().is_some_and(|m| (n as u64) < m)
        || s[hi].as_u64().is_some_and(|m| n as u64 > m)
    {
        Err("length bound exceeded")
    } else {
        Ok(())
    }
}
pub fn canonical_decimal(s: &str, signed: bool) -> bool {
    let d = if let Some(x) = s.strip_prefix('-') {
        if !signed || x == "0" {
            return false;
        }
        x
    } else {
        s
    };
    !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) && (d.len() == 1 || !d.starts_with('0'))
}
pub fn canonical_base64(s: &str) -> bool {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return false;
    }
    if b.is_empty() {
        return true;
    }
    let padding = b.iter().rev().take_while(|&&b| b == b'=').count();
    if padding > 2 {
        return false;
    }
    let val = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    if b[..b.len() - padding].iter().any(|&b| val(b).is_none()) {
        return false;
    }
    match padding {
        1 => val(b[b.len() - 2]).is_some_and(|v| v & 3 == 0),
        2 => val(b[b.len() - 3]).is_some_and(|v| v & 15 == 0),
        _ => true,
    }
}
pub fn encode_bytes(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::new();
    for c in bytes.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | ((c.get(1).copied().unwrap_or(0) as u32) << 8)
            | (c.get(2).copied().unwrap_or(0) as u32);
        o.push(A[((n >> 18) & 63) as usize] as char);
        o.push(A[((n >> 12) & 63) as usize] as char);
        o.push(if c.len() > 1 {
            A[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        o.push(if c.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    o
}
pub fn decode_bytes(s: &str) -> PluginResult<Vec<u8>> {
    if !canonical_base64(s) {
        return Err(invalid("noncanonical base64"));
    }
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    for c in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for b in c {
            n = (n << 6) | A.iter().position(|x| x == b).unwrap_or(0) as u32;
        }
        out.push((n >> 16) as u8);
        if c[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if c[3] != b'=' {
            out.push(n as u8);
        }
    }
    Ok(out)
}
pub fn utc_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 24
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'.'
        || b[23] != b'Z'
    {
        return false;
    }
    let n = |a: usize, z: usize| -> Option<u32> {
        let p = &b[a..z];
        if !p.iter().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(p.iter().fold(0, |n, b| n * 10 + (*b - b'0') as u32))
    };
    let (Some(y), Some(m), Some(d), Some(h), Some(min), Some(sec), Some(_)) = (
        n(0, 4),
        n(5, 7),
        n(8, 10),
        n(11, 13),
        n(14, 16),
        n(17, 19),
        n(20, 23),
    ) else {
        return false;
    };
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    y >= 1 && d >= 1 && d <= days && h < 24 && min < 60 && sec < 60
}
pub fn identities(
    contracts: impl IntoIterator<Item = Contract>,
) -> BTreeMap<String, InterfaceIdentity> {
    contracts
        .into_iter()
        .map(|c| (c.id().to_string(), c.identity()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn semantic_validation() {
        assert!(validate(&json!({"type":"string","maxLength":1}), &json!("😀")).is_ok());
        assert!(validate(&json!({"type":"integer"}), &json!(9007199254740992u64)).is_err());
        assert!(
            validate(
                &json!({"type":"object","properties":{},"additionalProperties":false}),
                &json!({"__proto__":1})
            )
            .is_err()
        );
    }
    #[test]
    fn canonical_adapters() {
        for s in ["", "AA==", "YWJj", "YWJjZA=="] {
            assert!(canonical_base64(s));
            assert_eq!(encode_bytes(&decode_bytes(s).unwrap()), s);
        }
        for s in ["AB==", "YQ", "YQ=", "====", "a b="] {
            assert!(!canonical_base64(s));
        }
        assert!(utc_datetime("2024-02-29T23:59:59.999Z"));
        for s in [
            "2023-02-29T00:00:00.000Z",
            "0000-01-01T00:00:00.000Z",
            "2024-02-29T00:00:60.000Z",
        ] {
            assert!(!utc_datetime(s));
        }
    }
    #[test]
    fn reject_unsupported() {
        for s in [
            json!({"type":"string","pattern":"a"}),
            json!({"anyOf":[{"type":"string"},{"type":"number"}]}),
            json!({"type":"object","properties":{}}),
        ] {
            assert!(validate(&s, &Value::Null).is_err());
        }
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;
    #[test]
    fn authoritative_js_canonical_bytes_and_tamper() {
        let v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/canonical.contract.json"))
                .unwrap();
        Contract::from_value(v.clone()).unwrap();
        let mut wrong = v;
        wrong["descriptor"]["version"] = Value::String("1.0.1".into());
        assert_eq!(
            Contract::from_value(wrong).unwrap_err().stable_code(),
            "CONTRACT_MISMATCH"
        );
    }
    #[test]
    fn canonical_profiles_reject_unsupported_union_and_nullable_literals() {
        assert!(
            validate(
                &serde_json::json!({"anyOf":[{"type":"string"},{"type":"null"}]}),
                &Value::Null
            )
            .is_err()
        );
        assert!(
            validate(
                &serde_json::json!({"type":["string","null"],"const":"x"}),
                &Value::Null
            )
            .is_err()
        );
    }
}
