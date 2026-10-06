//! Permission-checked file modules. No implicit network or package resolution.
use super::permissions::Permissions;
use crate::{ModuleLoader, ModuleSource, VmErr};
#[cfg(feature = "runtime-fs")]
use std::path::{Path, PathBuf};

#[cfg(feature = "runtime-fs")]
pub struct FileLoader {
    permissions: Permissions,
    base: PathBuf,
    max_bytes: usize,
}
#[cfg(feature = "runtime-fs")]
impl FileLoader {
    pub fn new(
        permissions: Permissions,
        base: impl AsRef<Path>,
        max_bytes: usize,
    ) -> std::io::Result<Self> {
        let base = if base.as_ref().is_absolute() {
            base.as_ref().to_path_buf()
        } else {
            std::env::current_dir()?.join(base)
        };
        Ok(Self {
            permissions,
            base,
            max_bytes,
        })
    }
    fn path(id: &str) -> Result<PathBuf, VmErr> {
        url::Url::parse(id)
            .map_err(|e| VmErr::Msg(e.to_string()))?
            .to_file_path()
            .map_err(|_| VmErr::Msg("Unsupported module URL".into()))
    }
}
#[cfg(feature = "runtime-fs")]
impl ModuleLoader for FileLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr> {
        if url::Url::parse(specifier).is_ok_and(|u| u.scheme() != "file") {
            return Err(VmErr::Msg("Unsupported module URL".into()));
        }
        let resolved = if specifier.starts_with("file:") {
            url::Url::parse(specifier).map_err(|e| VmErr::Msg(e.to_string()))?
        } else if let Some(referrer) = referrer {
            if !specifier.starts_with('.') && !specifier.starts_with('/') {
                return Err(VmErr::Msg(format!("Unsupported bare module: {specifier}")));
            }
            url::Url::parse(referrer)
                .map_err(|e| VmErr::Msg(e.to_string()))?
                .join(specifier)
                .map_err(|e| VmErr::Msg(e.to_string()))?
        } else {
            let path = self.base.join(specifier);
            url::Url::from_file_path(path)
                .map_err(|_| VmErr::Msg("invalid file module path".into()))?
        };
        if resolved.scheme() != "file"
            || resolved.query().is_some()
            || resolved.fragment().is_some()
        {
            return Err(VmErr::Msg("Unsupported module URL".into()));
        }
        Ok(resolved.into())
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        let source = self
            .permissions
            .read_text(Self::path(id)?, self.max_bytes)?;
        Ok(ModuleSource {
            id: id.into(),
            source,
        })
    }
}

/// Opt-in HTTP modules. Fetch and every redirect enforce the supplied policy.
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
pub struct HttpLoader {
    permissions: Permissions,
    max_bytes: usize,
    timeout: std::time::Duration,
    redirects: std::cell::RefCell<std::collections::HashMap<String, String>>,
}
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
impl HttpLoader {
    pub fn new(permissions: Permissions, max_bytes: usize, timeout: std::time::Duration) -> Self {
        Self {
            permissions,
            max_bytes,
            timeout,
            redirects: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }
}
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
impl ModuleLoader for HttpLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr> {
        let url = if let Ok(url) = url::Url::parse(specifier) {
            url
        } else {
            let base = referrer
                .ok_or_else(|| VmErr::Msg("HTTP module requires an absolute URL".into()))?;
            let redirected = self.redirects.borrow().get(base).cloned();
            let base = redirected.as_deref().unwrap_or(base);
            if !specifier.starts_with('.') && !specifier.starts_with('/') {
                return Err(VmErr::Msg("Unsupported bare HTTP import".into()));
            }
            url::Url::parse(base)
                .and_then(|base| base.join(specifier))
                .map_err(|e| VmErr::Msg(e.to_string()))?
        };
        super::network::validate_url(url.as_str(), &self.permissions).map_err(VmErr::Msg)?;
        Ok(url.into())
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        let request = super::network::FetchRequest {
            url: id.into(),
            method: "GET".into(),
            headers: Vec::new(),
            body: None,
            redirect: "follow".into(),
        };
        let response = super::network::fetch(
            request,
            self.permissions.clone(),
            self.max_bytes,
            self.timeout,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .map_err(VmErr::Msg)?;
        let status = response["status"].as_u64().unwrap_or(0);
        if !(200..300).contains(&status) {
            return Err(VmErr::Msg(format!("HTTP module returned {status}")));
        }
        let bytes =
            super::network::response_bytes(&response, self.max_bytes).map_err(VmErr::Msg)?;
        self.redirects
            .borrow_mut()
            .insert(id.into(), response["url"].as_str().unwrap_or(id).into());
        Ok(ModuleSource {
            id: id.into(),
            source: String::from_utf8(bytes).map_err(|e| VmErr::Msg(e.to_string()))?,
        })
    }
}
