//! Data-only trusted process protocol. This crate provides no sandbox.
#![forbid(unsafe_code)]
mod peer;
mod schema;
pub use peer::*;
pub use schema::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fmt, future::Future, pin::Pin};
pub type PluginResult<T> = Result<T, RpcError>;
pub type PluginFuture<'a, T> = Pin<Box<dyn Future<Output = PluginResult<T>> + Send + 'a>>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub data: Value,
}
impl RpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        let number = match code {
            "PARSE_ERROR" => -32700,
            "INVALID_REQUEST" => -32600,
            "METHOD_NOT_FOUND" => -32601,
            "INVALID_ARGUMENT" => -32602,
            "NOT_READY" => -32001,
            "PROTOCOL_MISMATCH" => -32002,
            "CONTRACT_MISMATCH" => -32003,
            "CANCELLED" => -32004,
            "DEADLINE_EXCEEDED" => -32005,
            "OVERLOADED" => -32006,
            "PLUGIN_EXITED" => -32007,
            "CONNECTION_CLOSED" => -32008,
            "REENTRANT_CALL" => -32009,
            "INVALID_RESULT" => -32010,
            "STATE_INCOMPATIBLE" => -32011,
            "APPLICATION_ERROR" => -32012,
            _ => -32603,
        };
        Self {
            code: number,
            message: message.into().chars().take(2048).collect(),
            data: json!({"code":code}),
        }
    }
    pub(crate) fn validate_wire(&self) -> PluginResult<()> {
        let invalid = || RpcError::new("INVALID_REQUEST", "invalid error metadata");
        let data = self.data.as_object().ok_or_else(invalid)?;
        let code = data
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let known = [
            "PARSE_ERROR",
            "INVALID_REQUEST",
            "METHOD_NOT_FOUND",
            "INVALID_ARGUMENT",
            "INTERNAL_ERROR",
            "NOT_READY",
            "PROTOCOL_MISMATCH",
            "CONTRACT_MISMATCH",
            "CANCELLED",
            "DEADLINE_EXCEEDED",
            "OVERLOADED",
            "PLUGIN_EXITED",
            "CONNECTION_CLOSED",
            "REENTRANT_CALL",
            "INVALID_RESULT",
            "STATE_INCOMPATIBLE",
            "APPLICATION_ERROR",
        ];
        if !known.contains(&code)
            || Self::new(code, "").code != self.code
            || self.message.chars().count() > 2048
            || data
                .keys()
                .any(|key| !["code", "details", "domainCode", "data"].contains(&key.as_str()))
        {
            return Err(invalid());
        }
        if code == "APPLICATION_ERROR" {
            if !data
                .get("domainCode")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
                || !data.contains_key("data")
            {
                return Err(invalid());
            }
        } else if data.contains_key("domainCode") || data.contains_key("data") {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn domain(code: &str, message: impl Into<String>, data: Value) -> Self {
        let mut e = Self::new("APPLICATION_ERROR", message);
        e.data = json!({"code":"APPLICATION_ERROR", "domainCode":code, "data":data});
        e
    }
    pub fn stable_code(&self) -> &str {
        self.data["code"].as_str().unwrap_or("INTERNAL_ERROR")
    }
}
impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stable_code(), self.message)
    }
}
impl std::error::Error for RpcError {}
impl From<std::io::Error> for RpcError {
    fn from(e: std::io::Error) -> Self {
        Self::new("CONNECTION_CLOSED", e.to_string())
    }
}
impl From<serde_json::Error> for RpcError {
    fn from(e: serde_json::Error) -> Self {
        Self::new("INVALID_ARGUMENT", e.to_string())
    }
}

/// Missing and explicit null are independent: use Field<Option<T>> for optional nullable fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Field<T> {
    #[default]
    Missing,
    Value(T),
}
impl<T> Field<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}
impl<T: Serialize> Serialize for Field<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(v) => v.serialize(s),
            Self::Missing => Err(serde::ser::Error::custom(
                "Missing must use skip_serializing_if = Field::is_missing",
            )),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::Value)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceIdentity {
    pub version: String,
    pub digest: String,
}

/// Converts safe integral binary64 values to integral JSON storage before typed deserialization.
/// Duplicate keys are already last-key-wins after serde_json::Value parsing.
pub fn normalize_value(value: &mut Value, depth: usize, limit: usize) -> PluginResult<()> {
    if depth > limit {
        return Err(RpcError::new(
            "INVALID_ARGUMENT",
            "JSON nesting limit exceeded",
        ));
    }
    match value {
        Value::Number(n) => {
            let f = n
                .as_f64()
                .ok_or_else(|| RpcError::new("INVALID_ARGUMENT", "non-finite number"))?;
            if !f.is_finite() {
                return Err(RpcError::new("INVALID_ARGUMENT", "non-finite number"));
            }
            if f.fract() == 0.0 && f.abs() <= 9007199254740991.0 {
                *value = Value::from(f as i64);
            } else {
                *value = Value::Number(
                    serde_json::Number::from_f64(f)
                        .ok_or_else(|| RpcError::new("INVALID_ARGUMENT", "non-finite number"))?,
                );
            }
        }
        Value::Array(a) => {
            for v in a {
                normalize_value(v, depth + 1, limit)?;
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                normalize_value(v, depth + 1, limit)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn from_wire_value<T: serde::de::DeserializeOwned>(mut v: Value) -> PluginResult<T> {
    normalize_value(&mut v, 0, 64)?;
    Ok(serde_json::from_value(v)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct S {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        x: Field<Option<String>>,
    }
    #[test]
    fn error_messages_obey_the_shared_unicode_limit() {
        let error = RpcError::new("INTERNAL_ERROR", "🙂".repeat(3000));
        assert_eq!(error.message.chars().count(), 2048);
        assert!(serde_json::to_string(&error).is_ok());
    }
    #[test]
    fn wire_errors_reject_malformed_metadata() {
        for (code, data) in [
            (-32603, json!({"code":"UNKNOWN"})),
            (-32012, json!({"code":"INTERNAL_ERROR"})),
            (-32603, json!({"code":"INTERNAL_ERROR", "extra":true})),
            (
                -32603,
                json!({"code":"INTERNAL_ERROR", "domainCode":"ERR", "data":null}),
            ),
            (
                -32012,
                json!({"code":"APPLICATION_ERROR", "domainCode":12, "data":null}),
            ),
            (
                -32012,
                json!({"code":"APPLICATION_ERROR", "domainCode":"ERR"}),
            ),
        ] {
            let error = RpcError {
                code,
                message: "bad".into(),
                data,
            };
            assert_eq!(
                error.validate_wire().unwrap_err().stable_code(),
                "INVALID_REQUEST"
            );
        }
        RpcError::new("INTERNAL_ERROR", "🙂".repeat(2048))
            .validate_wire()
            .unwrap();
        RpcError::domain("ERR", "failure", Value::Null)
            .validate_wire()
            .unwrap();
        let mut error = RpcError::new("INTERNAL_ERROR", "failure");
        error.message = "x".repeat(2049);
        assert!(error.validate_wire().is_err());
    }
    #[test]
    fn missing_is_not_null() {
        assert_eq!(serde_json::from_str::<S>("{}").unwrap().x, Field::Missing);
        assert_eq!(
            serde_json::from_str::<S>(r#"{"x":null}"#).unwrap().x,
            Field::Value(None)
        );
        assert_eq!(
            serde_json::to_value(S { x: Field::Missing }).unwrap(),
            json!({})
        );
    }
    #[test]
    fn numeric_forms_and_duplicate_keys() {
        for s in ["{\"x\":0,\"x\":1}", "{\"x\":1.0}", "{\"x\":1e0}"] {
            let mut v: Value = serde_json::from_str(s).unwrap();
            normalize_value(&mut v, 0, 64).unwrap();
            assert_eq!(v, json!({"x":1}));
        }
    }
}

/// A finite binary64 business number. Construction rejects NaN/infinity before
/// serde_json can coerce them to null, including inside nullable containers.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct FiniteF64(f64);
impl FiniteF64 {
    pub fn get(self) -> f64 {
        self.0
    }
}
impl TryFrom<f64> for FiniteF64 {
    type Error = RpcError;
    fn try_from(value: f64) -> PluginResult<Self> {
        if !value.is_finite() {
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "business numbers must be finite binary64",
            ));
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }
}
impl From<FiniteF64> for f64 {
    fn from(v: FiniteF64) -> Self {
        v.get()
    }
}
impl Serialize for FiniteF64 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if !self.0.is_finite() {
            return Err(serde::ser::Error::custom("non-finite business number"));
        }
        s.serialize_f64(self.0)
    }
}
impl<'de> Deserialize<'de> for FiniteF64 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = f64::deserialize(d)?;
        Self::try_from(v).map_err(serde::de::Error::custom)
    }
}
#[cfg(test)]
mod finite_tests {
    use super::*;
    #[test]
    fn finite_before_optional_serialization() {
        for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(FiniteF64::try_from(n).is_err());
        }
        let v = FiniteF64::try_from(-0.0).unwrap();
        assert!(!v.get().is_sign_negative());
        assert_eq!(serde_json::to_value(Some(v)).unwrap().as_f64(), Some(0.0));
        assert!(serde_json::from_str::<FiniteF64>("1e400").is_err());
    }
}
