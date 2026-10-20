#![cfg(all(
    feature = "runtime-fs",
    feature = "runtime-npm",
    feature = "runtime-typescript"
))]
use napi_vm_runtime::runtime::{
    loaders::FileLoader, npm::NpmLoader, permissions::Permissions, typescript::TypeScriptLoader,
};
use napi_vm_runtime::{Interpreter, ModuleLoader, Value};
use std::rc::Rc;
#[test]
fn runtime_binary_adapters_preserve_bytes_permissions_and_limits() {
    let root = std::env::temp_dir().join(format!("runtime-byte-modules-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("payload.ts"), [0, 255, 128, 13, 10]).unwrap();
    let allowed = Permissions::new().allow_read(&root).unwrap();
    let file = Rc::new(FileLoader::new(allowed.clone(), &root, 16).unwrap());
    let npm = Rc::new(NpmLoader::new(allowed, &root, 16).unwrap());
    let wrapped = Rc::new(TypeScriptLoader::new(file.clone()));
    let id = file.resolve("./payload.ts", None).unwrap();
    for loader in [file as Rc<dyn ModuleLoader>, npm, wrapped] {
        assert_eq!(
            loader.load_bytes(&id, &[]).unwrap().bytes,
            [0, 255, 128, 13, 10]
        );
        let mut vm = Interpreter::with_builtins();
        vm.set_module_loader(loader);
        let code = format!(
            "import bytes from {} with {{type:'bytes'}};bytes.buffer.immutable && bytes[1]===255 && bytes.length===5",
            serde_json::to_string(&id).unwrap()
        );
        assert!(matches!(vm.eval_source(&code).unwrap(), Value::Bool(true)));
    }
    let denied = FileLoader::new(Permissions::new(), &root, 16).unwrap();
    assert!(denied.load_bytes(&id, &[]).is_err());
    let limited = FileLoader::new(Permissions::new().allow_read(&root).unwrap(), &root, 4).unwrap();
    assert!(
        limited
            .load_bytes(&id, &[])
            .unwrap_err()
            .to_string()
            .contains("size exceeded")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "runtime-net")]
#[test]
fn http_binary_loader_preserves_non_utf8_and_enforces_network_policy() {
    use napi_vm_runtime::runtime::loaders::HttpLoader;
    use std::io::{Read, Write};
    use std::time::Duration;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\x00\xff\x80",
            )
            .unwrap();
    });
    let url = format!("http://{address}/payload");
    let loader = HttpLoader::new(
        Permissions::new().allow_net("127.0.0.1", Some(address.port())),
        16,
        Duration::from_secs(5),
    );
    assert_eq!(loader.load_bytes(&url, &[]).unwrap().bytes, [0, 255, 128]);
    server.join().unwrap();
    let denied = HttpLoader::new(Permissions::new(), 16, Duration::from_secs(1));
    assert!(denied.load_bytes(&url, &[]).is_err());
}
