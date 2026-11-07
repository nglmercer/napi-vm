use napi_vm_runtime::runtime::{
    loaders::FileLoader, npm::NpmLoader, permissions::Permissions, typescript::transform,
};
use napi_vm_runtime::{CommonJsModuleLoader, Interpreter, ModuleLoader, Value};
use serde_json::json;
use std::{hint::black_box, path::PathBuf, time::Instant};

type Counter = fn() -> (u64, u64);

pub fn run(counter: Option<Counter>) {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 4, "usage: io-bench FIXTURES SAMPLES ITERATIONS");
    let root = PathBuf::from(&args[1]).canonicalize().unwrap();
    let samples: usize = args[2].parse().unwrap();
    let iterations: usize = args[3].parse().unwrap();
    assert!(samples > 0 && samples % 2 == 1 && iterations > 0);
    let permissions = Permissions::new()
        .allow_read(&root)
        .unwrap()
        .allow_write(&root)
        .unwrap();
    let file_loader = FileLoader::new(permissions.clone(), &root, 8 << 20).unwrap();
    let npm = NpmLoader::new(permissions.clone(), &root, 8 << 20).unwrap();
    let mut results = Vec::new();
    let mut measure = |backend: &str,
                       name: &str,
                       expected: usize,
                       operation: &mut dyn FnMut() -> usize| {
        for _ in 0..5 {
            assert_eq!(operation(), expected, "{backend}/{name}");
        }
        let mut values = Vec::new();
        let mut requested = Vec::new();
        for _ in 0..samples {
            if let Some(snapshot) = counter {
                let before = snapshot();
                for _ in 0..iterations {
                    assert_eq!(black_box(operation()), expected);
                }
                let after = snapshot();
                values.push((after.0 - before.0) as f64 / iterations as f64);
                requested.push((after.1 - before.1) as f64 / iterations as f64);
            } else {
                let start = Instant::now();
                for _ in 0..iterations {
                    assert_eq!(black_box(operation()), expected);
                }
                values.push(start.elapsed().as_secs_f64() * 1e9 / iterations as f64);
            }
        }
        results.push(json!({"backend":backend,"name":name,"unit":if counter.is_some(){"allocation requests/op"}else{"ns/op"},"samples":values,"requested_bytes_per_op":requested,"checked_result":expected}));
    };
    for (name, relative, size) in [
        ("read_bytes_empty", "empty", 0),
        ("read_bytes_4k", "4k", 4096),
        ("read_bytes_64k", "64k", 65536),
        ("read_bytes_4m", "4m", 4 << 20),
        ("read_bytes_nested_4k", "a/b/c/d/4k", 4096),
    ] {
        let path = root.join(relative);
        let expected = vec![b'x'; size];
        assert_eq!(permissions.read_bytes(&path, 8 << 20).unwrap(), expected);
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        measure("permissions", name, size, &mut || {
            permissions.read_bytes(&path, 8 << 20).unwrap().len()
        });
        measure("rust_std", name, size, &mut || {
            std::fs::read(&path).unwrap().len()
        });
    }
    let path = root.join("64k");
    assert_eq!(
        permissions.read_text(&path, 8 << 20).unwrap(),
        "x".repeat(65536)
    );
    measure("permissions", "read_text_64k", 65536, &mut || {
        permissions.read_text(&path, 8 << 20).unwrap().len()
    });
    measure("rust_std", "read_text_64k", 65536, &mut || {
        std::fs::read_to_string(&path).unwrap().len()
    });
    let payload = vec![b'w'; 4096];
    for backend in ["permissions", "rust_std"] {
        let path = root.join(format!("write-{backend}"));
        measure(backend, "write_bytes_4k", 4096, &mut || {
            if backend == "permissions" {
                permissions.write_bytes(&path, &payload, 4096).unwrap();
            } else {
                std::fs::write(&path, &payload).unwrap();
            }
            payload.len()
        });
        assert_eq!(std::fs::read(path).unwrap(), payload);
    }
    let path = root.join("a/b/c/d");
    measure("permissions", "mkdir_existing_nested", 1, &mut || {
        permissions.create_dir_all(&path).unwrap();
        1
    });
    measure("rust_std", "mkdir_existing_nested", 1, &mut || {
        std::fs::create_dir_all(&path).unwrap();
        1
    });
    let denied = Permissions::new();
    measure("permissions", "denied_read", 1, &mut || {
        usize::from(denied.read_bytes(&path, 4096).is_err())
    });
    let id = file_loader.resolve("./module.mjs", None).unwrap();
    measure("file_loader", "resolve", id.len(), &mut || {
        file_loader.resolve("./module.mjs", None).unwrap().len()
    });
    assert_eq!(
        file_loader.load(&id).unwrap().source,
        std::fs::read_to_string(root.join("module.mjs")).unwrap()
    );
    measure("file_loader", "load_64k", 65536, &mut || {
        file_loader.load(&id).unwrap().source.len()
    });
    let id = ModuleLoader::resolve(&npm, "perf-local", None).unwrap();
    assert!(id.ends_with("esm.mjs"));
    measure("npm_loader", "resolve_4m", id.len(), &mut || {
        ModuleLoader::resolve(&npm, "perf-local", None)
            .unwrap()
            .len()
    });
    let module = CommonJsModuleLoader::resolve(&npm, "perf-local", None).unwrap();
    assert!(module.filename.ends_with("cjs.cjs"));
    assert_eq!(
        module.source.unwrap(),
        std::fs::read_to_string(root.join("node_modules/perf-local/cjs.cjs")).unwrap()
    );
    measure("npm_loader", "cjs_resolve_read_64k", 65536, &mut || {
        CommonJsModuleLoader::resolve(&npm, "perf-local", None)
            .unwrap()
            .source
            .unwrap()
            .len()
    });
    for count in [100, 1000] {
        let path = root.join(format!("types-{count}.ts"));
        let source = std::fs::read_to_string(&path).unwrap();
        let filename = path.file_name().unwrap().as_ref();
        let output = transform(filename, &source).unwrap();
        let map: serde_json::Value = serde_json::from_str(&output.source_map).unwrap();
        assert_eq!(
            map["sources"][0].as_str().unwrap(),
            filename.to_str().unwrap()
        );
        assert!(
            matches!(Interpreter::with_builtins().eval_source(&output.javascript).unwrap(), Value::Number(n) if n == (count - 1) as f64)
        );
        let size = output.javascript.len() + output.source_map.len();
        measure(
            "oxc",
            &format!("typescript_{count}_sourcemap"),
            size,
            &mut || {
                let output = transform(filename, &source).unwrap();
                output.javascript.len() + output.source_map.len()
            },
        );
    }
    println!(
        "{}",
        json!({"schema":1,"kind":if counter.is_some(){"allocations"}else{"latency"},"samples":samples,"iterations":iterations,"results":results})
    );
}
