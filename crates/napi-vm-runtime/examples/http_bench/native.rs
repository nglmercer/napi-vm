//! Loopback transport benchmark; compiles the production HTTP worker unchanged.
//! Input allocation and result checks are outside the per-request timer.
#[path = "../../src/runtime/network.rs"]
mod network;
mod permissions {
    pub use napi_vm_runtime::runtime::permissions::Permissions;
}
use permissions::Permissions;
use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let stream = stream.unwrap();
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_string();
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut remaining = length;
                let mut buffer = [0u8; 8192];
                while remaining != 0 {
                    let chunk = remaining.min(buffer.len());
                    reader.read_exact(&mut buffer[..chunk]).unwrap();
                    assert!(buffer[..chunk].iter().all(|b| *b == 7));
                    remaining -= chunk;
                }
                let stream = reader.get_mut();
                if let Some(hops) = path.strip_prefix("/redirect/") {
                    let hops: usize = hops.parse().unwrap();
                    if hops > 0 {
                        write!(stream, "HTTP/1.1 307 Temporary Redirect\r\nLocation: /redirect/{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", hops-1).unwrap();
                        continue;
                    }
                }
                let response = if let Some(size) = path.strip_prefix("/bytes/") {
                    vec![9; size.parse().unwrap()]
                } else {
                    length.to_string().into_bytes()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}
pub fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (mut samples, mut iterations) = (9usize, 5usize);
    for pair in args.chunks(2) {
        assert_eq!(pair.len(), 2);
        match pair[0].as_str() {
            "--samples" => samples = pair[1].parse().unwrap(),
            "--iterations" => iterations = pair[1].parse().unwrap(),
            flag => panic!("unknown argument {flag}"),
        }
    }
    assert!(samples > 0 && samples % 2 == 1 && iterations > 0);
    let server = Server::new();
    let permissions = Permissions::new().allow_net("127.0.0.1", Some(server.address.port()));
    let mut results = Vec::new();
    for (name, path, upload_bytes, response_bytes) in [
        ("get_empty", "/bytes/0", 0, 0),
        ("get_64k", "/bytes/65536", 0, 65536),
        ("post_4m", "/upload", 4 * 1024 * 1024, 0),
        ("post_4m_redirect_3", "/redirect/3", 4 * 1024 * 1024, 0),
    ] {
        let mut timings = Vec::new();
        // One untimed batch warms allocator and library initialization.
        for sample in 0..=samples {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iterations {
                let request = network::FetchRequest {
                    url: format!("http://{}{path}", server.address),
                    method: if upload_bytes == 0 { "GET" } else { "POST" }.into(),
                    headers: vec![],
                    body: (upload_bytes != 0).then(|| vec![7; upload_bytes]),
                    redirect: "follow".into(),
                };
                let cancelled = Arc::new(AtomicBool::new(false));
                let grants = permissions.clone();
                let started = Instant::now();
                let response = network::fetch(
                    request,
                    grants,
                    8 * 1024 * 1024,
                    Duration::from_secs(10),
                    cancelled,
                )
                .unwrap();
                elapsed += started.elapsed();
                assert_eq!(response["status"], 200);
                assert_eq!(response["redirected"], name.ends_with("redirect_3"));
                let body = network::response_bytes(&response, 8 * 1024 * 1024).unwrap();
                if upload_bytes != 0 {
                    assert_eq!(body, upload_bytes.to_string().as_bytes());
                } else {
                    assert_eq!(body, vec![9; response_bytes]);
                }
            }
            if sample != 0 {
                timings.push(elapsed.as_nanos() as f64 / iterations as f64);
            }
        }
        results.push(json!({"name":name,"unit":"ns/request","samples":timings}));
    }
    println!(
        "{}",
        json!({"schema":1,"samples":samples,"iterations":iterations,"results":results})
    );
}
