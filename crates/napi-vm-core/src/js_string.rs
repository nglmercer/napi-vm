//! ECMAScript strings are sequences of UTF-16 code units, including unpaired
//! surrogates. UTF-8 rendering is only a host-facing, lossy view.
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

#[derive(Clone)]
struct StringData {
    units: Vec<u16>,
    utf8: OnceLock<String>,
}
#[derive(Clone)]
pub struct JsString(Arc<StringData>);
impl JsString {
    pub fn from_units(units: impl Into<Vec<u16>>) -> Self {
        Self(Arc::new(StringData {
            units: units.into(),
            utf8: OnceLock::new(),
        }))
    }
    pub fn truncate(&mut self, len: usize) {
        let data = Arc::make_mut(&mut self.0);
        data.utf8.take();
        data.units.truncate(len);
    }
    pub fn units(&self) -> &[u16] {
        &self.0.units
    }
    pub fn len(&self) -> usize {
        self.units().len()
    }
    pub fn is_empty(&self) -> bool {
        self.units().is_empty()
    }
    pub fn as_str(&self) -> &str {
        self.0
            .utf8
            .get_or_init(|| String::from_utf16_lossy(self.units()))
    }
    pub fn to_utf8(&self) -> Result<String, std::string::FromUtf16Error> {
        String::from_utf16(self.units())
    }
    pub fn slice(&self, start: usize, end: usize) -> Self {
        Self::from_units(self.units()[start..end].to_vec())
    }
    pub fn concat(&self, other: &Self) -> Self {
        let mut units = Vec::with_capacity(self.len().saturating_add(other.len()));
        units.extend_from_slice(self.units());
        units.extend_from_slice(other.units());
        Self::from_units(units)
    }
    pub fn push_str(&mut self, text: impl Into<Self>) {
        let text = text.into();
        let data = Arc::make_mut(&mut self.0);
        data.utf8.take();
        data.units.extend_from_slice(text.units());
    }
    pub fn push(&mut self, ch: char) {
        let data = Arc::make_mut(&mut self.0);
        data.utf8.take();
        data.units
            .extend(ch.encode_utf16(&mut [0; 2]).iter().copied());
    }

    pub fn repeat(&self, count: usize) -> Self {
        Self::from_units(self.units().repeat(count))
    }
    pub fn find_from(&self, needle: &Self, start: usize) -> Option<usize> {
        if start > self.len() {
            return None;
        }
        if needle.is_empty() {
            return Some(start);
        }
        self.units()[start..]
            .windows(needle.len())
            .position(|w| w == needle.units())
            .map(|i| start + i)
    }
    pub fn trim_units(&self, start: bool, end: bool) -> Self {
        let whitespace = |unit: u16| matches!(unit,0x0009..=0x000D|0x0020|0x00A0|0x1680|0x2000..=0x200A|0x2028|0x2029|0x202F|0x205F|0x3000|0xFEFF);
        let mut a = 0;
        let mut b = self.len();
        if start {
            while a < b && whitespace(self.units()[a]) {
                a += 1;
            }
        }
        if end {
            while b > a && whitespace(self.units()[b - 1]) {
                b -= 1;
            }
        }
        self.slice(a, b)
    }
    pub fn map_case(&self, upper: bool) -> Self {
        let mut result = Vec::new();
        for ch in char::decode_utf16(self.units().iter().copied()) {
            match ch {
                Err(e) => result.push(e.unpaired_surrogate()),
                Ok(ch) => {
                    let mapped = if upper {
                        ch.to_uppercase().collect::<String>()
                    } else {
                        ch.to_lowercase().collect::<String>()
                    };
                    result.extend(mapped.encode_utf16());
                }
            }
        }
        Self::from_units(result)
    }
    pub fn code_point_at(&self, index: usize) -> Option<u32> {
        let hi = *self.units().get(index)?;
        if (0xD800..=0xDBFF).contains(&hi)
            && let Some(&lo) = self.units().get(index + 1)
            && (0xDC00..=0xDFFF).contains(&lo)
        {
            return Some(0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00));
        }
        Some(hi as u32)
    }
    /// Scalar iteration combines surrogate pairs and retains unpaired units.
    pub fn code_points(&self) -> impl Iterator<Item = Self> + '_ {
        let mut offset = 0;
        std::iter::from_fn(move || {
            let first = *self.units().get(offset)?;
            let start = offset;
            offset += 1;
            if (0xD800..=0xDBFF).contains(&first)
                && self
                    .units()
                    .get(offset)
                    .is_some_and(|u| (0xDC00..=0xDFFF).contains(u))
            {
                offset += 1;
            }
            Some(self.slice(start, offset))
        })
    }
    /// Slot names preserve lone surrogates without colliding with ordinary
    /// strings, including strings containing the reserved encoding marker.
    pub fn to_key(&self) -> String {
        let mut out = String::new();
        for scalar in char::decode_utf16(self.units().iter().copied()) {
            match scalar {
                Ok('\u{FDD0}') => out.push_str("\u{FDD0}\u{FDD0}"),
                Ok(ch) => out.push(ch),
                Err(error) => {
                    out.push('\u{FDD0}');
                    out.push('s');
                    out.push_str(&format!("{:04X}", error.unpaired_surrogate()));
                }
            }
        }
        out
    }
    pub fn from_key(key: &str) -> Self {
        let mut units = Vec::new();
        let mut chars = key.chars();
        while let Some(ch) = chars.next() {
            if ch == '\u{FDD0}' {
                match chars.next() {
                    Some('\u{FDD0}') => units.push(0xFDD0),
                    Some('s') => {
                        let hex = chars.by_ref().take(4).collect::<String>();
                        if let Ok(unit) = u16::from_str_radix(&hex, 16) {
                            units.push(unit);
                        } else {
                            units.extend("\u{FDD0}s".encode_utf16());
                            units.extend(hex.encode_utf16());
                        }
                    }
                    Some(other) => {
                        units.push(0xFDD0);
                        units.extend(other.encode_utf16(&mut [0; 2]).iter().copied());
                    }
                    None => units.push(0xFDD0),
                }
            } else {
                units.extend(ch.encode_utf16(&mut [0; 2]).iter().copied());
            }
        }
        Self::from_units(units)
    }
}
impl Default for JsString {
    fn default() -> Self {
        Self::from("")
    }
}
impl From<String> for JsString {
    fn from(value: String) -> Self {
        let units = value.encode_utf16().collect::<Vec<_>>();
        let utf8 = OnceLock::new();
        let _ = utf8.set(value);
        Self(Arc::new(StringData { units, utf8 }))
    }
}
impl From<&str> for JsString {
    fn from(value: &str) -> Self {
        Self::from(value.to_string())
    }
}
impl From<&String> for JsString {
    fn from(value: &String) -> Self {
        Self::from(value.as_str())
    }
}
impl From<&JsString> for JsString {
    fn from(value: &JsString) -> Self {
        value.clone()
    }
}
impl From<JsString> for String {
    fn from(value: JsString) -> Self {
        value.as_str().to_string()
    }
}
impl Deref for JsString {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl AsRef<str> for JsString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl std::fmt::Display for JsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}
impl std::fmt::Debug for JsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.to_utf8() {
            Ok(text) => std::fmt::Debug::fmt(&text, f),
            Err(_) => f.debug_tuple("JsString").field(&self.units()).finish(),
        }
    }
}
impl PartialEq for JsString {
    fn eq(&self, other: &Self) -> bool {
        self.units() == other.units()
    }
}
impl Eq for JsString {}
impl PartialOrd for JsString {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for JsString {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.units().cmp(other.units())
    }
}
impl std::hash::Hash for JsString {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.units().hash(state)
    }
}
impl PartialEq<str> for JsString {
    fn eq(&self, other: &str) -> bool {
        self.units().iter().copied().eq(other.encode_utf16())
    }
}
impl PartialEq<&str> for JsString {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}
impl PartialEq<String> for JsString {
    fn eq(&self, other: &String) -> bool {
        self == other.as_str()
    }
}
impl PartialEq<JsString> for str {
    fn eq(&self, other: &JsString) -> bool {
        other == self
    }
}
impl PartialEq<JsString> for &str {
    fn eq(&self, other: &JsString) -> bool {
        other == *self
    }
}
impl PartialEq<JsString> for String {
    fn eq(&self, other: &JsString) -> bool {
        other == self
    }
}
impl std::ops::Add<&str> for JsString {
    type Output = Self;
    fn add(self, other: &str) -> Self {
        self.concat(&other.into())
    }
}
impl serde::Serialize for JsString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.to_utf8() {
            Ok(text) => serializer.serialize_str(&text),
            Err(_) => serde::Serialize::serialize(self.units(), serializer),
        }
    }
}
impl<'de> serde::Deserialize<'de> for JsString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Representation {
            Text(String),
            Units(Vec<u16>),
        }
        match Representation::deserialize(deserializer)? {
            Representation::Text(text) => Ok(text.into()),
            Representation::Units(units) => Ok(Self::from_units(units)),
        }
    }
}
