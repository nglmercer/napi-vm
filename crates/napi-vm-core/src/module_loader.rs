//! Host-selected module source loading. The engine has no ambient loader.
use crate::VmErr;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ModuleSource {
    pub id: String,
    pub source: String,
}

/// Resolve identities separately from loading text. Implementations are
/// owner-local and must enforce their capabilities before accessing resources.
pub trait ModuleLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr>;
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr>;
}

/// In-memory modules for embedders; no runtime feature or OS access required.
#[derive(Default)]
pub struct VirtualLoader {
    sources: RefCell<HashMap<String, String>>,
}
impl VirtualLoader {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&self, id: impl Into<String>, source: impl Into<String>) {
        self.sources.borrow_mut().insert(id.into(), source.into());
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
        if self.sources.borrow().contains_key(&id) {
            Ok(id)
        } else {
            Err(VmErr::Msg(format!("Module not found: {id}")))
        }
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
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

/// JavaScript data URLs, with no host IO or ambient permissions.
pub struct DataUrlLoader {
    max_bytes: usize,
}
impl DataUrlLoader {
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
}
impl ModuleLoader for DataUrlLoader {
    fn resolve(&self, specifier: &str, _referrer: Option<&str>) -> Result<String, VmErr> {
        if !specifier.starts_with("data:") {
            return Err(VmErr::Msg("Unsupported data module specifier".into()));
        }
        self.load(specifier)?;
        Ok(specifier.into())
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
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
        if !matches!(
            media.to_ascii_lowercase().as_str(),
            "text/javascript"
                | "application/javascript"
                | "application/ecmascript"
                | "text/ecmascript"
        ) {
            return Err(VmErr::Msg("Unsupported data module media type".into()));
        }
        let mut encoded = false;
        for parameter in parameters {
            if parameter.eq_ignore_ascii_case("base64") && !encoded {
                encoded = true;
            } else if !parameter.eq_ignore_ascii_case("charset=utf-8") {
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
        Ok(ModuleSource {
            id: id.into(),
            source: String::from_utf8(bytes).map_err(|e| VmErr::Msg(e.to_string()))?,
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
}
