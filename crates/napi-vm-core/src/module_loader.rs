//! Host-selected module source loading. The engine has no ambient loader.
use crate::VmErr;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ModuleSource {
    pub id: String,
    pub source: String,
}

/// Raw bytes returned by a host without text decoding.
#[derive(Debug, Clone)]
pub struct ModuleBytes {
    pub id: String,
    pub bytes: Vec<u8>,
}

/// Resolve identities separately from loading text. Implementations are
/// owner-local and must enforce their capabilities before accessing resources.
pub trait ModuleLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr>;
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr>;

    /// Load a resource for `type: "bytes"`, without UTF-8 decoding. Binary
    /// support is an explicit host capability; text-only loaders may reject it.
    fn load_bytes(
        &self,
        _id: &str,
        _attributes: &[(String, crate::JsString)],
    ) -> Result<ModuleBytes, VmErr> {
        Err(VmErr::Msg(
            "TypeError: Module loader does not support byte modules".into(),
        ))
    }

    /// Load a static or dynamic module request with its import attributes.
    /// Hosts that do not assign meaning to attributes can keep using `load`.
    fn load_with_attributes(
        &self,
        id: &str,
        _attributes: &[(String, crate::JsString)],
    ) -> Result<ModuleSource, VmErr> {
        self.load(id)
    }
}

/// In-memory modules for embedders; no runtime feature or OS access required.
#[derive(Default)]
pub struct VirtualLoader {
    sources: RefCell<HashMap<String, String>>,
    bytes: RefCell<HashMap<String, Vec<u8>>>,
}
impl VirtualLoader {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&self, id: impl Into<String>, source: impl Into<String>) {
        let id = id.into();
        self.bytes.borrow_mut().remove(&id);
        self.sources.borrow_mut().insert(id, source.into());
    }
    pub fn insert_bytes(&self, id: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        let id = id.into();
        self.sources.borrow_mut().remove(&id);
        self.bytes.borrow_mut().insert(id, bytes.into());
    }
}
impl ModuleLoader for VirtualLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr> {
        let id = if specifier.starts_with('.') {
            if let Some(referrer) = referrer {
                let mut parts: Vec<_> = referrer
                    .rsplit_once('/')
                    .map(|(base, _)| base.split('/').collect())
                    .unwrap_or_default();
                for part in specifier.split('/') {
                    match part {
                        "" | "." => {}
                        ".." => {
                            parts.pop();
                        }
                        value => parts.push(value),
                    }
                }
                parts.join("/")
            } else {
                specifier.into()
            }
        } else {
            specifier.into()
        };
        if self.sources.borrow().contains_key(&id) || self.bytes.borrow().contains_key(&id) {
            Ok(id)
        } else {
            Err(VmErr::Msg(format!("Module not found: {id}")))
        }
    }
    fn load_bytes(
        &self,
        id: &str,
        _attributes: &[(String, crate::JsString)],
    ) -> Result<ModuleBytes, VmErr> {
        let bytes = self
            .bytes
            .borrow()
            .get(id)
            .cloned()
            .or_else(|| {
                self.sources
                    .borrow()
                    .get(id)
                    .map(|source| source.as_bytes().to_vec())
            })
            .ok_or_else(|| VmErr::Msg(format!("Module not found: {id}")))?;
        Ok(ModuleBytes {
            id: id.into(),
            bytes,
        })
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        if let Some(bytes) = self.bytes.borrow().get(id) {
            return Ok(ModuleSource {
                id: id.into(),
                source: String::from_utf8(bytes.clone()).map_err(|_| {
                    VmErr::Msg("TypeError: Module source is not valid UTF-8".into())
                })?,
            });
        }
        let source = self
            .sources
            .borrow()
            .get(id)
            .cloned()
            .ok_or_else(|| VmErr::Msg(format!("Module not found: {id}")))?;
        Ok(ModuleSource {
            id: id.into(),
            source,
        })
    }
}

/// JavaScript and binary data URLs, with no host IO or ambient permissions.
pub struct DataUrlLoader {
    max_bytes: usize,
}
impl DataUrlLoader {
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
}
impl DataUrlLoader {
    fn decode(&self, id: &str, javascript: bool) -> Result<Vec<u8>, VmErr> {
        use base64::Engine;
        let (metadata, payload) = id
            .strip_prefix("data:")
            .and_then(|v| v.split_once(','))
            .ok_or_else(|| VmErr::Msg("Invalid data URL".into()))?;
        if payload.len() > self.max_bytes.saturating_mul(3).saturating_add(4) {
            return Err(VmErr::Msg("ResourceLimit: module size exceeded".into()));
        }
        let mut parameters = metadata.split(';');
        let media = parameters.next().unwrap_or("");
        if javascript
            && !matches!(
                media.to_ascii_lowercase().as_str(),
                "text/javascript"
                    | "application/javascript"
                    | "application/ecmascript"
                    | "text/ecmascript"
            )
        {
            return Err(VmErr::Msg("Unsupported data module media type".into()));
        }
        let mut encoded = false;
        for parameter in parameters {
            if parameter.eq_ignore_ascii_case("base64") {
                if encoded {
                    return Err(VmErr::Msg("Unsupported data module encoding".into()));
                }
                encoded = true;
            } else if javascript && !parameter.eq_ignore_ascii_case("charset=utf-8") {
                return Err(VmErr::Msg("Unsupported data module encoding".into()));
            }
        }
        let mut bytes = Vec::new();
        let payload = payload.split('#').next().unwrap_or("").as_bytes();
        let mut index = 0;
        while index < payload.len() {
            if payload[index] == b'%' {
                let hex = payload
                    .get(index + 1..index + 3)
                    .ok_or_else(|| VmErr::Msg("Invalid percent escape".into()))?;
                let hi = (hex[0] as char)
                    .to_digit(16)
                    .ok_or_else(|| VmErr::Msg("Invalid percent escape".into()))?;
                let lo = (hex[1] as char)
                    .to_digit(16)
                    .ok_or_else(|| VmErr::Msg("Invalid percent escape".into()))?;
                bytes.push((hi * 16 + lo) as u8);
                index += 3;
            } else {
                bytes.push(payload[index]);
                index += 1;
            }
        }
        if encoded {
            bytes = base64::engine::general_purpose::STANDARD
                .decode(bytes)
                .map_err(|e| VmErr::Msg(e.to_string()))?;
        }
        if bytes.len() > self.max_bytes {
            return Err(VmErr::Msg("ResourceLimit: module size exceeded".into()));
        }
        Ok(bytes)
    }
}
impl ModuleLoader for DataUrlLoader {
    fn resolve(&self, specifier: &str, _referrer: Option<&str>) -> Result<String, VmErr> {
        self.decode(specifier, false)?;
        Ok(specifier.into())
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        Ok(ModuleSource {
            id: id.into(),
            source: String::from_utf8(self.decode(id, true)?)
                .map_err(|e| VmErr::Msg(e.to_string()))?,
        })
    }
    fn load_bytes(&self, id: &str, _: &[(String, crate::JsString)]) -> Result<ModuleBytes, VmErr> {
        Ok(ModuleBytes {
            id: id.into(),
            bytes: self.decode(id, false)?,
        })
    }
}

/// Dispatch absolute module schemes while delegating file and bare requests.
/// A scheme's loader is never selected implicitly.
pub struct CompositeLoader {
    fallback: std::rc::Rc<dyn ModuleLoader>,
    schemes: HashMap<String, std::rc::Rc<dyn ModuleLoader>>,
}
impl CompositeLoader {
    pub fn new(fallback: std::rc::Rc<dyn ModuleLoader>) -> Self {
        Self {
            fallback,
            schemes: HashMap::new(),
        }
    }
    pub fn with_scheme(mut self, scheme: &str, loader: std::rc::Rc<dyn ModuleLoader>) -> Self {
        self.schemes.insert(scheme.into(), loader);
        self
    }
    fn loader(&self, specifier: &str, referrer: Option<&str>) -> &dyn ModuleLoader {
        let scheme = specifier.split_once(':').map(|(s, _)| s).or_else(|| {
            if specifier.starts_with('.') || specifier.starts_with('/') {
                referrer.and_then(|r| r.split_once(':').map(|(s, _)| s))
            } else {
                None
            }
        });
        scheme
            .and_then(|s| self.schemes.get(s))
            .map(|v| v.as_ref())
            .unwrap_or(self.fallback.as_ref())
    }
}
impl ModuleLoader for CompositeLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr> {
        self.loader(specifier, referrer)
            .resolve(specifier, referrer)
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        self.loader(id, None).load(id)
    }
    fn load_with_attributes(
        &self,
        id: &str,
        attributes: &[(String, crate::JsString)],
    ) -> Result<ModuleSource, VmErr> {
        self.loader(id, None).load_with_attributes(id, attributes)
    }
    fn load_bytes(
        &self,
        id: &str,
        attributes: &[(String, crate::JsString)],
    ) -> Result<ModuleBytes, VmErr> {
        self.loader(id, None).load_bytes(id, attributes)
    }
}
