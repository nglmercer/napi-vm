#![cfg(feature = "runtime-typescript")]
use napi_vm::runtime::typescript::transform;
use napi_vm::{Interpreter, Value};
use std::path::Path;
#[test]
fn typescript_runs_as_javascript_with_source_map() {
    let source = "interface Item { count:number }\nconst item:Item={count:42};\nitem.count;";
    let transformed = transform(Path::new("app.ts"), source).unwrap();
    assert!(!transformed.javascript.contains("interface"));
    let map: serde_json::Value = serde_json::from_str(&transformed.source_map).unwrap();
    assert_eq!(map["sources"][0], "app.ts");
    let mut engine = Interpreter::with_builtins();
    assert!(matches!(
        engine.eval_source(&transformed.javascript).unwrap(),
        Value::Number(42.0)
    ));
    assert!(transform(Path::new("bad.ts"), "const value: = ;").is_err());
}
#[test]
fn enums_and_tsx_transform_without_engine_syntax_changes() {
    let transformed =
        transform(Path::new("enum.ts"), "enum Color { Red, Blue } Color.Blue;").unwrap();
    assert!(matches!(
        Interpreter::with_builtins()
            .eval_source(&transformed.javascript)
            .unwrap(),
        Value::Number(1.0)
    ));
    let transformed = transform(Path::new("app.tsx"), "const view = <div title='ok' />;").unwrap();
    assert!(!transformed.javascript.contains("<div"));
}
