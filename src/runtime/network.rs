//! HTTP workers own no VM values. Redirects are followed only after a new grant check.
use super::permissions::Permissions;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Deserialize)]
pub(super) struct FetchRequest {
    pub url: String,
    #[serde(default = "get")]
    pub method: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    #[serde(default = "follow")]
    pub redirect: String,
}
fn get() -> String {
    "GET".into()
}
fn follow() -> String {
    "follow".into()
}
#[derive(Serialize)]
struct FetchResponse {
    url: String,
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    bytes_base64: String,
    redirected: bool,
}
pub(super) fn validate_url(input: &str, permissions: &Permissions) -> Result<url::Url, String> {
    let url = url::Url::parse(input).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("TypeError: fetch requires an HTTP(S) URL without credentials".into());
    }
    permissions
        .check_net(
            url.host_str().ok_or("missing HTTP host")?,
            url.port_or_known_default().ok_or("missing HTTP port")?,
        )
        .map_err(|e| e.to_string())?;
    Ok(url)
}
async fn fetch_async(
    mut request: FetchRequest,
    permissions: Permissions,
    max_bytes: usize,
    timeout: Duration,
    cancelled: Arc<AtomicBool>,
) -> Result<serde_json::Value, String> {
    let mut url = validate_url(&request.url, &permissions)?;
    let client = reqwest::Client::builder()
        // Ambient proxy configuration must not reroute sandbox traffic.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now()
        .checked_add(timeout)
        .ok_or("invalid network timeout")?;
    for hop in 0..=10 {
        if cancelled.load(Ordering::Acquire) {
            return Err("AbortError: cancelled".into());
        }
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or("TimeoutError: fetch timeout")?;
        let method =
            reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|e| e.to_string())?;
        if matches!(method.as_str(), "CONNECT" | "TRACE" | "TRACK") {
            return Err("TypeError: forbidden HTTP method".into());
        }
        let mut outgoing = client
            .request(method.clone(), url.clone())
            .timeout(remaining);
        for (name, value) in &request.headers {
            // Do not let a guest override the authority granted by permissions.
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "host"
                    | "connection"
                    | "content-length"
                    | "transfer-encoding"
                    | "proxy-authorization"
                    | "proxy-connection"
            ) {
                continue;
            }
            outgoing = outgoing.header(name, value);
        }
        if let Some(body) = &request.body {
            outgoing = outgoing.body(body.clone());
        }
        let mut response = cancellable(outgoing.send(), &cancelled, deadline).await?;
        let status = response.status();
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
            && let Some(location) = response.headers().get(reqwest::header::LOCATION)
        {
            if request.redirect == "error" {
                return Err("TypeError: redirect disallowed".into());
            }
            if request.redirect != "manual" {
                if hop == 10 {
                    return Err("TypeError: too many redirects".into());
                }
                let target = url
                    .join(location.to_str().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                validate_url(target.as_str(), &permissions)?;
                if target.origin() != url.origin() {
                    request.headers.retain(|(n, _)| {
                        !matches!(n.to_ascii_lowercase().as_str(), "authorization" | "cookie")
                    });
                }
                if status.as_u16() == 303 && method != reqwest::Method::HEAD
                    || matches!(status.as_u16(), 301 | 302) && method == reqwest::Method::POST
                {
                    request.method = "GET".into();
                    request.body = None;
                    request
                        .headers
                        .retain(|(n, _)| !n.to_ascii_lowercase().starts_with("content-"));
                }
                url = target;
                continue;
            }
        }
        if response
            .content_length()
            .is_some_and(|n| n > max_bytes as u64)
        {
            return Err("ResourceLimit: HTTP body size exceeded".into());
        }
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                Ok((
                    name.to_string(),
                    value.to_str().map_err(|e| e.to_string())?.to_string(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = cancellable(response.chunk(), &cancelled, deadline).await? {
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                return Err("ResourceLimit: HTTP body size exceeded".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if cancelled.load(Ordering::Acquire) {
            return Err("AbortError: cancelled".into());
        }
        return serde_json::to_value(FetchResponse {
            url: url.into(),
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("").into(),
            headers,
            bytes_base64: {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(bytes)
            },
            redirected: hop != 0,
        })
        .map_err(|e| e.to_string());
    }
    Err("TypeError: too many redirects".into())
}

async fn cancellable<T>(
    future: impl std::future::Future<Output = Result<T, reqwest::Error>>,
    cancelled: &AtomicBool,
    deadline: std::time::Instant,
) -> Result<T, String> {
    tokio::pin!(future);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("AbortError: cancelled".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err("TimeoutError: HTTP deadline exceeded".into());
        }
        tokio::select! {
            result=&mut future=>return result.map_err(|e|e.to_string()),
            _=tokio::time::sleep(Duration::from_millis(10))=>{}
        }
    }
}
pub(super) fn fetch(
    request: FetchRequest,
    permissions: Permissions,
    max_bytes: usize,
    timeout: Duration,
    cancelled: Arc<AtomicBool>,
) -> Result<serde_json::Value, String> {
    // Synchronous module loading is also usable from an embedding Tokio owner.
    // Only transport data enters this worker, never interpreter state.
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::Builder::new()
            .name("napi-vm-http-loader".into())
            .spawn(move || fetch(request, permissions, max_bytes, timeout, cancelled))
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "HTTP worker panicked".to_string())?;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(fetch_async(
            request,
            permissions,
            max_bytes,
            timeout,
            cancelled,
        ))
}

pub(super) fn response_bytes(
    response: &serde_json::Value,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let encoded = response["bytes_base64"]
        .as_str()
        .ok_or("Missing binary transport payload")?;
    if encoded.len() > max_bytes.div_ceil(3).saturating_mul(4) {
        return Err("ResourceLimit: binary payload exceeded".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max_bytes {
        return Err("ResourceLimit: binary payload exceeded".into());
    }
    Ok(bytes)
}
