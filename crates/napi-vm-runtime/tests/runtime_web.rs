#![cfg(all(
    feature = "runtime-web",
    feature = "runtime-net",
    not(target_arch = "wasm32")
))]
use napi_vm_runtime::Value;
use napi_vm_runtime::runtime::{RuntimeBuilder, RuntimeLimits, permissions::Permissions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

fn server(response: &'static str) -> (u16, std::thread::JoinHandle<String>) {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = socket.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0u8; 4096];
        let n = stream.read(&mut request).unwrap();
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8_lossy(&request[..n]).into_owned()
    });
    (port, worker)
}
#[test]
fn headers_bodies_and_abort_work_without_network_grants() {
    let mut runtime = RuntimeBuilder::new().web_apis().build().unwrap();
    let value = runtime
        .eval("const h = new Headers([['X-Test', ' a '], ['x-test','b']]); h.get('X-TEST')")
        .unwrap();
    assert!(matches!(value, Value::String(ref s) if s == "a, b"));
    runtime.eval("let text = ''; const r = Response.json({ok:true}); r.json().then(v => text=String(v.ok)); text").unwrap();
    let value = runtime.eval("text").unwrap();
    assert!(matches!(value, Value::String(ref s) if s == "true"));
    let value = runtime.eval("const c = new AbortController(); let calls = 0; c.signal.addEventListener('abort', () => calls++); c.abort(); c.abort(); calls").unwrap();
    assert!(matches!(value, Value::Number(1.0)));
    assert!(
        matches!(runtime.eval("typeof fetch").unwrap(), Value::String(ref s) if s == "undefined")
    );
}
#[test]
fn fetch_completes_on_owner_and_preserves_response() {
    let (port, worker) = server(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
    );
    let policy = Permissions::new().allow_net("127.0.0.1", Some(port));
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(policy)
        .build()
        .unwrap();
    runtime.eval(&format!("let result = ''; fetch('http://127.0.0.1:{port}/data').then(r => r.json()).then(v => result=String(v.ok));")).unwrap();
    runtime.run_event_loop().unwrap();
    assert!(matches!(runtime.eval("result").unwrap(), Value::String(ref s) if s == "true"));
    assert!(worker.join().unwrap().starts_with("GET /data HTTP/1.1"));
}
#[test]
fn denied_redirect_never_reaches_its_target() {
    let denied = TcpListener::bind("127.0.0.1:0").unwrap();
    let denied_port = denied.local_addr().unwrap().port();
    denied.set_nonblocking(true).unwrap();
    let response = Box::leak(format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{denied_port}/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_boxed_str());
    let (port, worker) = server(response);
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new().allow_net("127.0.0.1", Some(port)))
        .build()
        .unwrap();
    runtime
        .eval(&format!(
            "let rejected = ''; fetch('http://127.0.0.1:{port}').catch(e => rejected=String(e));"
        ))
        .unwrap();
    runtime.run_event_loop().unwrap();
    assert!(
        matches!(runtime.eval("rejected").unwrap(), Value::String(ref s) if s.contains("PermissionDenied"))
    );
    assert!(denied.accept().is_err());
    worker.join().unwrap();
}
#[test]
fn permission_body_limit_and_preaborted_requests_reject() {
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new())
        .build()
        .unwrap();
    runtime
        .eval("let denied=''; fetch('https://example.com').catch(e => denied=String(e));")
        .unwrap();
    assert!(
        matches!(runtime.eval("denied").unwrap(), Value::String(ref s) if s.contains("PermissionDenied"))
    );
    runtime.eval("let aborted=false; fetch('https://example.com', {signal:AbortSignal.abort('stop')}).catch(e => aborted=e==='stop');").unwrap();
    assert!(matches!(
        runtime.eval("aborted").unwrap(),
        Value::Bool(true)
    ));
    let (port, worker) =
        server("HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n12345678");
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new().allow_net("127.0.0.1", Some(port)))
        .limits(RuntimeLimits {
            file_bytes: 4,
            ..RuntimeLimits::default()
        })
        .build()
        .unwrap();
    runtime
        .eval(&format!(
            "let limited=''; fetch('http://127.0.0.1:{port}').catch(e => limited=String(e));"
        ))
        .unwrap();
    runtime.run_event_loop().unwrap();
    assert!(
        matches!(runtime.eval("limited").unwrap(), Value::String(ref s) if s.contains("ResourceLimit"))
    );
    worker.join().unwrap();
}

#[test]
fn websocket_events_echo_and_close_on_the_vm_owner() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let message = socket.read().unwrap();
        socket.send(message).unwrap();
        let close = socket.read().unwrap();
        assert!(close.is_close());
        let _ = socket.flush();
    });
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new().allow_net("127.0.0.1", Some(port)))
        .limits(RuntimeLimits {
            timeout: Some(Duration::from_secs(5)),
            ..RuntimeLimits::default()
        })
        .build()
        .unwrap();
    runtime.eval(&format!("let events=[]; const ws=new WebSocket('ws://127.0.0.1:{port}/echo'); ws.onopen=()=>{{events.push('open');ws.send('hello');}}; ws.onmessage=e=>{{events.push(e.data);ws.close();}};ws.onclose=()=>events.push('close');")).unwrap();
    runtime.run_event_loop().unwrap();
    assert!(
        matches!(runtime.eval("events.join(',')").unwrap(),Value::String(ref s) if s=="open,hello,close")
    );
    worker.join().unwrap();
}
#[test]
fn tcp_read_write_and_eof_are_permission_checked() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = [0u8; 4];
        socket.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ping");
        socket.write_all(b"pong").unwrap();
    });
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new().allow_net("127.0.0.1", Some(port)))
        .limits(RuntimeLimits {
            timeout: Some(Duration::from_secs(5)),
            ..RuntimeLimits::default()
        })
        .build()
        .unwrap();
    runtime.eval(&format!("let tcpResult=''; napiVm.connect({{hostname:'127.0.0.1',port:{port}}}).then(async socket=>{{await socket.write(new TextEncoder().encode('ping'));const data=await socket.read();tcpResult=new TextDecoder().decode(data);socket.close();}});")).unwrap();
    runtime.run_event_loop().unwrap();
    assert!(matches!(runtime.eval("tcpResult").unwrap(),Value::String(ref s) if s=="pong"));
    worker.join().unwrap();
}

#[test]
fn crypto_digest_random_values_and_uuid_are_runtime_only() {
    let mut runtime = RuntimeBuilder::new().web_apis().build().unwrap();
    runtime.eval("let hash='';crypto.subtle.digest('SHA-256',new TextEncoder().encode('abc')).then(buffer=>{hash=Array.from(new Uint8Array(buffer)).map(n=>n.toString(16).padStart(2,'0')).join('');});").unwrap();
    assert!(
        matches!(runtime.eval("hash").unwrap(),Value::String(ref s) if s=="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert!(matches!(
        runtime.eval("crypto.randomUUID().length").unwrap(),
        Value::Number(36.0)
    ));
    assert!(
        runtime
            .eval("crypto.getRandomValues(new Float32Array(2))")
            .is_err()
    );
    assert!(
        runtime
            .eval("crypto.getRandomValues(new Uint8Array(65537))")
            .is_err()
    );
    assert!(matches!(
        runtime
            .eval("const bytes=new Uint8Array(32);crypto.getRandomValues(bytes)===bytes")
            .unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn http_server_dispatches_requests_to_guest_and_shuts_down() {
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new().allow_net("127.0.0.1", Some(0)))
        .limits(RuntimeLimits {
            timeout: Some(Duration::from_secs(5)),
            ..RuntimeLimits::default()
        })
        .build()
        .unwrap();
    runtime.eval("let server; napiVm.serve({hostname:'127.0.0.1',port:0},r=>{return r.json().then(value=>{Promise.resolve().then(()=>server.shutdown());return Response.json({answer:value.answer+1});});}).then(value=>server=value);").unwrap();
    for _ in 0..100 {
        runtime.poll().unwrap();
        if matches!(runtime.eval("typeof server").unwrap(),Value::String(ref s) if s!="undefined") {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let Value::Number(port) = runtime.eval("server.addr.port").unwrap() else {
        panic!("no server port")
    };
    let client = std::thread::spawn(move || {
        reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{}/answer", port as u16))
            .body("{\"answer\":41}")
            .send()
            .unwrap()
            .text()
            .unwrap()
    });
    runtime.run_event_loop().unwrap();
    assert_eq!(client.join().unwrap(), "{\"answer\":42}");
}

#[test]
fn abort_cancels_an_inflight_response_body_without_waiting_for_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0u8; 4096];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n")
            .unwrap();
        ready_tx.send(()).unwrap();
        let _ = stop_rx.recv_timeout(Duration::from_secs(5));
    });
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new().allow_net("127.0.0.1", Some(port)))
        .limits(RuntimeLimits {
            timeout: Some(Duration::from_secs(3)),
            ..RuntimeLimits::default()
        })
        .build()
        .unwrap();
    runtime.eval(&format!("let cancelled='';const controller=new AbortController();fetch('http://127.0.0.1:{port}',{{signal:controller.signal}}).catch(error=>cancelled=String(error));")).unwrap();
    ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let start = std::time::Instant::now();
    runtime.eval("controller.abort('stop')").unwrap();
    runtime.run_event_loop().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(matches!(runtime.eval("cancelled").unwrap(),Value::String(ref s) if s=="stop"));
    stop_tx.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn binary_bodies_above_the_js_array_cap_roundtrip_without_number_arrays() {
    use std::io::BufRead;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = std::io::BufReader::new(&mut stream);
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        assert_eq!(length, 300000);
        let mut bytes = vec![0u8; length];
        reader.read_exact(&mut bytes).unwrap();
        assert!(bytes.iter().all(|b| *b == 173));
        drop(reader);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    let mut runtime = RuntimeBuilder::new()
        .web_apis()
        .network(Permissions::new().allow_net("127.0.0.1", Some(port)))
        .build()
        .unwrap();
    runtime.eval(&format!("let bodySize=0;let byte=0;let binaryError='';const outgoing=new Uint8Array(300000);outgoing.fill(173);fetch('http://127.0.0.1:{port}',{{method:'POST',body:outgoing}}).then(r=>r.arrayBuffer()).then(buffer=>{{bodySize=buffer.byteLength;byte=new Uint8Array(buffer)[299999];}}).catch(e=>binaryError=String(e));")).unwrap();
    runtime.run_event_loop().unwrap();
    assert!(
        matches!(runtime.eval("bodySize").unwrap(), Value::Number(300000.0)),
        "{:?}",
        runtime.eval("binaryError").unwrap()
    );
    assert!(matches!(
        runtime.eval("byte").unwrap(),
        Value::Number(173.0)
    ));
    server.join().unwrap();
}
