//! One worker owns each connection; commands and results contain only plain data.
use super::{optional::ExternalEventSender, permissions::Permissions};
use base64::Engine;
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;
use tungstenite::{Message, client::IntoClientRequest};

#[derive(Deserialize)]
pub(super) struct OpenRequest {
    pub kind: String,
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub protocols: Vec<String>,
}
pub(super) enum Command {
    Next(u64),
    Send(u64, Payload),
    Close(u16, String),
}
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Payload {
    Text(String),
    Bytes(Vec<u8>),
    Http { metadata: String, bytes: Vec<u8> },
}
pub(super) fn authority(
    request: &OpenRequest,
    permissions: &Permissions,
) -> Result<(String, u16), String> {
    let (host, port) = if matches!(request.kind.as_str(), "tcp" | "http-server") {
        (
            request.host.clone().ok_or("TCP host required")?,
            request.port.ok_or("TCP port required")?,
        )
    } else if request.kind == "websocket" {
        let url = url::Url::parse(request.url.as_deref().ok_or("WebSocket URL required")?)
            .map_err(|e| e.to_string())?;
        if !matches!(url.scheme(), "ws" | "wss")
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Invalid WebSocket URL".into());
        }
        (
            url.host_str().ok_or("WebSocket host required")?.into(),
            url.port_or_known_default()
                .ok_or("WebSocket port required")?,
        )
    } else {
        return Err("Unsupported socket kind".into());
    };
    permissions
        .check_net(&host, port)
        .map_err(|e| e.to_string())?;
    Ok((
        host.trim_start_matches('[').trim_end_matches(']').into(),
        port,
    ))
}
enum Socket {
    Tcp(TcpStream),
    Ws(Box<tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>>),
    Http {
        listener: TcpListener,
        client: Option<TcpStream>,
        origin: String,
    },
}
fn connect(
    request: &OpenRequest,
    permissions: &Permissions,
    max_bytes: usize,
    timeout: Duration,
) -> Result<(Socket, String), String> {
    let (host, port) = authority(request, permissions)?;
    if request.kind == "http-server" {
        let listener = TcpListener::bind((host.as_str(), port)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        return Ok((
            Socket::Http {
                listener,
                client: None,
                origin: format!("http://{address}"),
            },
            String::new(),
        ));
    }
    let deadline = std::time::Instant::now()
        .checked_add(timeout)
        .ok_or("Invalid socket timeout")?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?;
    let mut connected = None;
    let mut error = "Socket address not found".to_string();
    for address in addresses.take(8) {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or("Socket connect timeout")?;
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(socket) => {
                connected = Some(socket);
                break;
            }
            Err(e) => error = e.to_string(),
        }
    }
    let stream = connected.ok_or(error)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    if request.kind == "tcp" {
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .map_err(|e| e.to_string())?;
        return Ok((Socket::Tcp(stream), String::new()));
    }
    let mut handshake = request
        .url
        .as_ref()
        .ok_or("WebSocket URL required")?
        .as_str()
        .into_client_request()
        .map_err(|e| e.to_string())?;
    if !request.protocols.is_empty() {
        let protocols = request.protocols.join(", ");
        handshake.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            protocols
                .parse()
                .map_err(|e: tungstenite::http::header::InvalidHeaderValue| e.to_string())?,
        );
    }
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(max_bytes))
        .max_frame_size(Some(max_bytes));
    let (mut socket, response) =
        tungstenite::client_tls_with_config(handshake, stream, Some(config), None)
            .map_err(|e| e.to_string())?;
    let protocol = response
        .headers()
        .get("Sec-WebSocket-Protocol")
        .map(|h| h.to_str().map(str::to_string))
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if !protocol.is_empty() && !request.protocols.contains(&protocol) {
        return Err("Server selected an unrequested WebSocket protocol".into());
    }
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => {
            s.set_read_timeout(Some(Duration::from_millis(50)))
        }
        tungstenite::stream::MaybeTlsStream::Rustls(s) => {
            s.sock.set_read_timeout(Some(Duration::from_millis(50)))
        }
        _ => return Err("Unsupported socket TLS backend".into()),
    }
    .map_err(|e| e.to_string())?;
    Ok((Socket::Ws(Box::new(socket)), protocol))
}
pub(super) struct WorkerConfig {
    pub permissions: Permissions,
    pub max_bytes: usize,
    pub timeout: Duration,
}
pub(super) fn worker(
    request: OpenRequest,
    config: WorkerConfig,
    open_id: u64,
    commands: mpsc::Receiver<Command>,
    events: ExternalEventSender,
    closed: Arc<AtomicBool>,
) {
    let WorkerConfig {
        permissions,
        max_bytes,
        timeout,
    } = config;
    let (mut socket, protocol) = match connect(&request, &permissions, max_bytes, timeout) {
        Ok(socket) => socket,
        Err(error) => {
            events.complete_wait(open_id, Err(error));
            return;
        }
    };
    let port = match &socket {
        Socket::Http { listener, .. } => listener.local_addr().ok().map(|a| a.port()),
        _ => None,
    };
    events.complete_wait(
        open_id,
        Ok(serde_json::json!({"id":open_id,"protocol":protocol,"port":port})),
    );
    let mut pending = None;
    let mut closing = false;
    let mut close_started = None;
    let mut close_result =
        serde_json::json!({"type":"close","code":1006,"reason":"","wasClean":false});
    loop {
        if closed.load(Ordering::Acquire) {
            break;
        }
        let waiting_response = matches!(
            &socket,
            Socket::Http {
                client: Some(_),
                ..
            }
        );
        let command = if pending.is_some() && !waiting_response {
            commands.try_recv().ok()
        } else {
            match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(_) => None,
            }
        };
        if let Some(command) = command {
            match command {
                Command::Next(id) => {
                    if pending.is_some() {
                        events.complete_wait(
                            id,
                            Err("Concurrent socket reads are unsupported".into()),
                        );
                    } else {
                        pending = Some(id);
                    }
                }
                Command::Send(id, payload) => {
                    let result = match &mut socket {
                        Socket::Tcp(s) => match payload {
                            Payload::Text(t) => s.write_all(t.as_bytes()),
                            Payload::Bytes(b) => s.write_all(&b),
                            Payload::Http { .. } => Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "HTTP payload on TCP socket",
                            )),
                        }
                        .map_err(|e| e.to_string()),
                        Socket::Ws(s) => s
                            .send(match payload {
                                Payload::Text(t) => Message::Text(t.into()),
                                Payload::Bytes(b) => Message::Binary(b.into()),
                                Payload::Http { .. } => {
                                    events
                                        .complete_wait(id, Err("HTTP payload on WebSocket".into()));
                                    continue;
                                }
                            })
                            .map_err(|e| e.to_string()),
                        Socket::Http { client, .. } => match client.take() {
                            Some(mut stream) => http_response(&mut stream, payload, max_bytes),
                            None => Err("No HTTP request awaits a response".into()),
                        },
                    };
                    let failed = result.is_err();
                    events.complete_wait(id, result.map(|_| serde_json::Value::Null));
                    if failed {
                        break;
                    }
                }
                Command::Close(code, reason) => {
                    closing = true;
                    close_started = Some(std::time::Instant::now());
                    match &mut socket {
                        Socket::Tcp(s) => {
                            let _ = s.shutdown(std::net::Shutdown::Both);
                            close_result = serde_json::json!({"type":"close","code":1000,"reason":"","wasClean":true});
                            break;
                        }
                        Socket::Ws(s) => {
                            let _ = s.close(Some(tungstenite::protocol::CloseFrame {
                                code: code.into(),
                                reason: reason.into(),
                            }));
                        }
                        Socket::Http { client, .. } => {
                            close_result = serde_json::json!({"type":"close","code":1000,"reason":"","wasClean":true});
                            if client.is_none() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        if closing && close_started.is_some_and(|start| start.elapsed() > Duration::from_secs(1)) {
            break;
        }
        if closing && matches!(&socket, Socket::Http { client: None, .. }) {
            break;
        }
        if pending.is_none() {
            continue;
        }
        let result = match &mut socket {
            Socket::Http {
                listener,
                client,
                origin,
            } => {
                if client.is_some() {
                    continue;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(timeout));
                        let _ = stream.set_write_timeout(Some(timeout));
                        match http_request(&mut stream, origin, max_bytes, timeout) {
                            Ok(request) => {
                                *client = Some(stream);
                                Some(Ok(request))
                            }
                            Err(_) => {
                                let _=stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                None
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        None
                    }
                    Err(e) => Some(Err(e.to_string())),
                }
            }
            Socket::Tcp(s) => {
                let mut bytes = vec![0u8; max_bytes.min(65536)];
                match s.read(&mut bytes) {
                    Ok(0) => {
                        close_result = serde_json::json!({"type":"close","code":1000,"reason":"","wasClean":true});
                        break;
                    }
                    Ok(n) => {
                        bytes.truncate(n);
                        Some(Ok(
                            serde_json::json!({"type":"message","binary":true,"bytes_base64":base64::engine::general_purpose::STANDARD.encode(bytes)}),
                        ))
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        None
                    }
                    Err(e) => Some(Err(e.to_string())),
                }
            }
            Socket::Ws(s) => match s.read() {
                Ok(Message::Text(text)) => Some(Ok(
                    serde_json::json!({"type":"message","binary":false,"data":text.as_str()}),
                )),
                Ok(Message::Binary(bytes)) => Some(Ok(
                    serde_json::json!({"type":"message","binary":true,"bytes_base64":base64::engine::general_purpose::STANDARD.encode(bytes.as_ref())}),
                )),
                Ok(Message::Close(frame)) => {
                    let (code, reason) = frame
                        .map(|f| (u16::from(f.code), f.reason.to_string()))
                        .unwrap_or((1005, String::new()));
                    close_result = serde_json::json!({"type":"close","code":code,"reason":reason,"wasClean":true});
                    let _ = s.flush();
                    break;
                }
                Ok(_) => {
                    let _ = s.flush();
                    None
                }
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    None
                }
                Err(e) => Some(Err(e.to_string())),
            },
        };
        if let Some(result) = result {
            let failed = result.is_err();
            events.complete_wait(pending.take().unwrap(), result);
            if failed {
                break;
            }
        }
    }
    if let Some(id) = pending {
        events.complete_wait(id, Ok(close_result));
    }
}

fn http_request(
    stream: &mut TcpStream,
    origin: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let deadline = std::time::Instant::now()
        .checked_add(timeout)
        .ok_or("Invalid HTTP timeout")?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or("HTTP request timeout")?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        if header.len() >= 16384 {
            return Err("HTTP headers exceeded".into());
        }
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
        header.push(byte[0]);
    }
    let text = std::str::from_utf8(&header).map_err(|e| e.to_string())?;
    let mut lines = text.split("\r\n");
    let start = lines
        .next()
        .ok_or("Missing request line")?
        .split(' ')
        .collect::<Vec<_>>();
    if start.len() != 3
        || !matches!(start[2], "HTTP/1.1" | "HTTP/1.0")
        || !start[1].starts_with('/')
        || start[1].starts_with("//")
    {
        return Err("Invalid request line".into());
    }
    let method = reqwest::Method::from_bytes(start[0].as_bytes()).map_err(|e| e.to_string())?;
    let mut headers = Vec::new();
    let mut length = None;
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("Invalid HTTP header")?;
        let name =
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|e| e.to_string())?;
        let value = value.trim();
        reqwest::header::HeaderValue::from_str(value).map_err(|e| e.to_string())?;
        if name == reqwest::header::TRANSFER_ENCODING {
            return Err("Chunked requests are not implemented".into());
        }
        if name == reqwest::header::CONTENT_LENGTH {
            if length.is_some() {
                return Err("Duplicate content-length".into());
            }
            length = Some(value.parse::<usize>().map_err(|e| e.to_string())?);
        }
        headers.push((name.to_string(), value.to_string()));
    }
    let length = length.unwrap_or(0);
    if length > max_bytes {
        return Err("HTTP body exceeded".into());
    }
    let mut bytes = vec![0u8; length];
    let mut offset = 0;
    while offset < length {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or("HTTP request timeout")?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let n = stream
            .read(&mut bytes[offset..])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("Truncated HTTP request".into());
        }
        offset += n;
    }
    Ok(
        serde_json::json!({"type":"http-request","method":method.as_str(),"url":format!("{origin}{}",start[1]),"headers":headers,"bytes_base64":base64::engine::general_purpose::STANDARD.encode(bytes)}),
    )
}
fn http_response(stream: &mut TcpStream, payload: Payload, max_bytes: usize) -> Result<(), String> {
    let Payload::Http {
        metadata: payload,
        bytes,
    } = payload
    else {
        return Err("HTTP response requires metadata and bytes".into());
    };
    let response: serde_json::Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
    let status = u16::try_from(response["status"].as_u64().ok_or("Invalid HTTP status")?)
        .map_err(|e| e.to_string())?;
    let status = reqwest::StatusCode::from_u16(status).map_err(|e| e.to_string())?;
    if bytes.len() > max_bytes {
        return Err("HTTP response size exceeded".into());
    }
    let headers: Vec<(String, String)> =
        serde_json::from_value(response["headers"].clone()).map_err(|e| e.to_string())?;
    let mut output = format!(
        "HTTP/1.1 {} {}\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    );
    for (name, value) in headers {
        let name =
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|e| e.to_string())?;
        reqwest::header::HeaderValue::from_str(&value).map_err(|e| e.to_string())?;
        if matches!(
            name.as_str(),
            "connection" | "content-length" | "transfer-encoding"
        ) {
            continue;
        }
        output.push_str(name.as_str());
        output.push_str(": ");
        output.push_str(&value);
        output.push_str("\r\n");
    }
    output.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    ));
    stream
        .write_all(output.as_bytes())
        .and_then(|_| stream.write_all(&bytes))
        .map_err(|e| e.to_string())
}
