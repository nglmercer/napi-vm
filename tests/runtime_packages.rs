#![cfg(all(feature = "runtime-npm", unix))]
use napi_vm::runtime::{RuntimeBuilder, npm::NpmLoader, permissions::Permissions};
use napi_vm::{CommonJsModuleLoader, DataUrlLoader, ModuleLoader, Value};
use std::path::PathBuf;
use std::rc::Rc;
struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("napi-vm-packages-{}-{name}", std::process::id()));
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        Self(root)
    }
    fn write(&self, path: &str, source: &str) {
        std::fs::write(self.0.join(path), source).unwrap();
    }
    fn loader(&self) -> Rc<NpmLoader> {
        Rc::new(
            NpmLoader::new(
                Permissions::new().allow_read(&self.0).unwrap(),
                &self.0,
                65536,
            )
            .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn conditional_exports_subpaths_imports_and_cjs_interop() {
    let fixture = Fixture::new("exports");
    fixture.write(
        "package.json",
        r##"{"type":"module","imports":{"#answer":"./answer.js"}}"##,
    );
    fixture.write("answer.js", "export const answer=42;");
    fixture.write("node_modules/pkg/package.json",r#"{"name":"pkg","type":"module","exports":{".":{"import":"./esm.js","require":"./cjs.cjs"},"./features/*":"./*.js","./blocked":null}}"#);
    fixture.write("node_modules/pkg/esm.js", "export const answer=42;");
    fixture.write("node_modules/pkg/cjs.cjs", "module.exports={answer:41};");
    fixture.write("node_modules/pkg/extra.js", "export const value=3;");
    let loader = fixture.loader();
    let imported = ModuleLoader::resolve(loader.as_ref(), "pkg", None).unwrap();
    assert!(imported.ends_with("esm.js"));
    let required = CommonJsModuleLoader::resolve(loader.as_ref(), "pkg", None).unwrap();
    assert!(required.filename.ends_with("cjs.cjs"));
    assert!(ModuleLoader::resolve(loader.as_ref(), "pkg/blocked", None).is_err());
    assert!(ModuleLoader::resolve(loader.as_ref(), "pkg/hidden.js", None).is_err());
    assert!(
        ModuleLoader::resolve(loader.as_ref(), "pkg/features/extra", None)
            .unwrap()
            .ends_with("extra.js")
    );
    assert!(
        ModuleLoader::resolve(loader.as_ref(), "#answer", None)
            .unwrap()
            .ends_with("answer.js")
    );
    let mut runtime = RuntimeBuilder::new()
        .module_loader(loader.clone())
        .commonjs_loader(loader.clone())
        .build()
        .unwrap();
    runtime
        .eval("import {answer} from 'pkg'; let value=answer;")
        .unwrap();
    assert!(matches!(
        runtime.eval("value").unwrap(),
        Value::Number(42.0)
    ));
    assert!(matches!(
        runtime.eval("require('pkg').answer").unwrap(),
        Value::Number(41.0)
    ));
    let cjs = fixture.0.join("node_modules/pkg/cjs.cjs");
    let cjs = url::Url::from_file_path(cjs).unwrap();
    runtime
        .eval(&format!(
            "import cjs from '{}'; let cjsValue=cjs.answer;",
            cjs
        ))
        .unwrap();
    assert!(matches!(
        runtime.eval("cjsValue").unwrap(),
        Value::Number(41.0)
    ));
}
#[test]
fn package_targets_and_read_access_fail_closed() {
    let fixture = Fixture::new("denied");
    fixture.write(
        "node_modules/pkg/package.json",
        r#"{"exports":"./../escape.js"}"#,
    );
    fixture.write("node_modules/escape.js", "export default 1;");
    assert!(ModuleLoader::resolve(fixture.loader().as_ref(), "pkg", None).is_err());
    let denied = NpmLoader::new(Permissions::new(), &fixture.0, 1000).unwrap();
    assert!(ModuleLoader::resolve(&denied, "pkg", None).is_err());
    assert!(ModuleLoader::resolve(fixture.loader().as_ref(), "npm:../escape", None).is_err());
}
#[test]
fn data_urls_preserve_plus_decode_base64_and_enforce_limits() {
    let loader = DataUrlLoader::new(100);
    assert_eq!(
        loader
            .load("data:text/javascript,export%20default%201+2")
            .unwrap()
            .source,
        "export default 1+2"
    );
    assert_eq!(
        loader
            .load("data:application/javascript;base64,ZXhwb3J0IGRlZmF1bHQgNDI7")
            .unwrap()
            .source,
        "export default 42;"
    );
    assert!(loader.load("data:text/html,<script></script>").is_err());
    assert!(loader.load("data:text/javascript,%GG").is_err());
    assert!(
        DataUrlLoader::new(1)
            .load("data:text/javascript,abcd")
            .is_err()
    );
}

#[test]
fn npm_semver_and_integrity_have_exact_version_semantics() {
    use base64::Engine;
    use napi_vm::runtime::npm::{select_version, verify_integrity, version_matches};
    use sha2::Digest;
    let metadata = serde_json::json!({"dist-tags":{"latest":"2.0.0"},"versions":{"1.2.3":{},"1.2.9":{},"1.9.0":{},"2.0.0":{}}});
    assert_eq!(select_version(&metadata, "1.2.3").unwrap(), "1.2.3");
    assert_eq!(select_version(&metadata, "1.2").unwrap(), "1.2.9");
    assert_eq!(select_version(&metadata, "^1.2.3").unwrap(), "1.9.0");
    assert_eq!(select_version(&metadata, "latest").unwrap(), "2.0.0");
    assert!(version_matches("2.0.0", "^1.0.0 || ^2.0.0").unwrap());
    let integrity = format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha512::digest(b"package"))
    );
    assert!(verify_integrity(b"package", &integrity).is_ok());
    assert!(verify_integrity(b"tampered", &integrity).is_err());
}
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(gzip);
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, *name, *bytes).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}
#[test]
fn archives_validate_before_writing_and_reject_links() {
    use napi_vm::runtime::npm::unpack_archive;
    let valid = archive(&[("package/index.js", b"export default 42;")]);
    assert_eq!(
        unpack_archive(&valid, 100).unwrap()[0].0.to_str(),
        Some("index.js")
    );
    assert!(unpack_archive(&valid, 1).is_err());
    assert!(unpack_archive(&archive(&[("other/index.js", b"data")]), 100).is_err());
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_link_name("/etc/passwd").unwrap();
    header.set_cksum();
    builder
        .append_data(&mut header, "package/link", &b""[..])
        .unwrap();
    let bytes = builder.into_inner().unwrap().finish().unwrap();
    assert!(unpack_archive(&bytes, 100).is_err());
}
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
#[test]
fn registry_install_verifies_artifact_and_replays_locked_cache() {
    use base64::Engine;
    use napi_vm::runtime::npm::NpmInstaller;
    use sha2::Digest;
    use std::io::{Read, Write};
    let fixture = Fixture::new("registry");
    let package = archive(&[
        (
            "package/package.json",
            br#"{"name":"demo","version":"1.2.3","main":"index.js"}"#,
        ),
        ("package/index.js", b"module.exports={answer:42};"),
    ]);
    let integrity = format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha512::digest(&package))
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let registry = format!("http://127.0.0.1:{port}/");
    let metadata=serde_json::json!({"versions":{"1.2.3":{"name":"demo","version":"1.2.3","dist":{"integrity":integrity,"tarball":format!("{registry}demo.tgz")}}}}).to_string().into_bytes();
    let server = std::thread::spawn(move || {
        for body in [metadata, package] {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0u8; 4096];
            assert!(socket.read(&mut request).unwrap() > 0);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            socket.write_all(&body).unwrap();
        }
    });
    let policy = Permissions::new()
        .allow_read(&fixture.0)
        .unwrap()
        .allow_write(&fixture.0)
        .unwrap()
        .allow_net("127.0.0.1", Some(port));
    let mut installer =
        NpmInstaller::new(policy.clone(), &fixture.0, &registry, 65536, false).unwrap();
    assert_eq!(installer.install("demo@1.2.3").unwrap().packages.len(), 1);
    server.join().unwrap();
    assert!(fixture.0.join("node_modules/demo/index.js").exists());
    let mut locked = NpmInstaller::new(policy, &fixture.0, &registry, 65536, true).unwrap();
    locked.install("demo@1.2.3").unwrap();
    assert!(locked.install("demo@2.0.0").is_err());
}
