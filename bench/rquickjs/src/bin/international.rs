use napi_vm_core::{Interpreter, Value};
use rquickjs::{Context, Function, Runtime, context::EvalOptions};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, hint::black_box, time::Instant};

fn export() -> Json {
    let data: Json =
        serde_json::from_str(include_str!("../../../international-workloads.json")).unwrap();
    let mut names = BTreeSet::new();
    let workloads: Vec<_> = data["cases"].as_array().unwrap().iter().map(|w| {
        for name in w["requires"].as_array().unwrap() { names.insert(name.as_str().unwrap()); }
        let setup = format!("{}\nfunction bench(){{{}}}", w["setup"].as_str().unwrap(), w["body"].as_str().unwrap());
        let source = format!("{setup}\nbench();");
        json!({"name":w["name"],"requires":w["requires"],"setup":setup,"invocation":"bench();","source":source,"expected":w["expected"],"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes()))})
    }).collect();
    let probe = format!(
        "JSON.stringify((function(){{var out={{}};for(var path of {}){{var x=globalThis;for(var key of path.split('.')){{if(x==null){{x=undefined;break;}}x=x[key];}}out[path]=typeof x==='function';}}return out;}})());",
        json!(names)
    );
    json!({"schema":1,"kind":"international_workloads","workloads":workloads,"probe_source":probe})
}
fn number(v: Value) -> f64 {
    match v {
        Value::Number(n) => n,
        other => panic!("expected number, got {other:?}"),
    }
}
fn qeval<T: for<'a> rquickjs::FromJs<'a>>(ctx: &rquickjs::Ctx<'_>, source: &str) -> T {
    let mut options = EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options(source, options)
        .unwrap_or_else(|e| panic!("rquickjs: {e:?}; {:?}", ctx.catch()))
}
fn measure(call: &mut dyn FnMut() -> f64, expected: f64, iterations: usize) -> f64 {
    let mut sum = 0.0;
    let start = Instant::now();
    for _ in 0..iterations {
        sum += black_box(call());
    }
    let elapsed = start.elapsed().as_nanos() as f64 / iterations as f64;
    assert_eq!(sum, expected * iterations as f64);
    elapsed
}
fn row(
    engine: &str,
    mode: &str,
    w: &Json,
    values: Vec<f64>,
    missing: &[String],
    problem: &Option<String>,
) -> Json {
    let mut row = json!({"engine":engine,"mode":mode,"workload":w["name"],"source_sha256":w["source_sha256"],"unit":"ns/call","status":if !missing.is_empty(){"unsupported"}else if problem.is_some(){"incompatible_result"}else{"measured"},"missing":missing});
    if let Some(message) = problem {
        row["diagnostic"] = json!(message);
    }
    if missing.is_empty() && problem.is_none() {
        assert!(!values.is_empty() && values.iter().all(|v| v.is_finite() && *v > 0.0));
        let mut sorted = values.clone();
        sorted.sort_by(f64::total_cmp);
        let mid = sorted[sorted.len() / 2];
        let mut deviations: Vec<_> = values.iter().map(|v| (v - mid).abs()).collect();
        deviations.sort_by(f64::total_cmp);
        row["samples"] = json!(values);
        row["median"] = json!(mid);
        row["mad"] = json!(deviations[deviations.len() / 2]);
    }
    row
}
fn missing(w: &Json, caps: &Json) -> Vec<String> {
    w["requires"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| !caps[n.as_str().unwrap()].as_bool().unwrap())
        .map(|n| n.as_str().unwrap().to_owned())
        .collect()
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let fixtures = export();
    if args.get(1).map(String::as_str) == Some("--emit-workloads") {
        println!("{fixtures}");
        return;
    }
    let option = |name: &str, default: &str| {
        args.windows(2)
            .find(|a| a[0] == name)
            .map(|a| a[1].clone())
            .unwrap_or_else(|| default.into())
    };
    let samples: usize = option("--samples", "15").parse().unwrap();
    let iterations: usize = option("--iterations", "10").parse().unwrap();
    assert!(samples > 0 && samples % 2 == 1 && iterations > 0);
    let mut vm = Interpreter::with_builtins();
    let probe = vm
        .eval_source(fixtures["probe_source"].as_str().unwrap())
        .unwrap();
    let native_caps: Json = match &probe {
        Value::String(s) => serde_json::from_str(&s.to_utf8().unwrap()).unwrap(),
        _ => panic!("invalid probe"),
    };
    drop(probe);
    drop(vm);
    Interpreter::new().collect_cycles();
    let rt = Runtime::new().unwrap();
    let context = Context::full(&rt).unwrap();
    let quick_caps: Json = context.with(|ctx| {
        serde_json::from_str(&qeval::<String>(
            &ctx,
            fixtures["probe_source"].as_str().unwrap(),
        ))
        .unwrap()
    });
    drop(context);
    drop(rt);
    let mut rows = Vec::new();
    let mut nonce = 0;
    for w in fixtures["workloads"].as_array().unwrap() {
        let nm = missing(w, &native_caps);
        let qm = missing(w, &quick_caps);
        let expected = w["expected"].as_f64().unwrap();
        let source = w["source"].as_str().unwrap();
        let setup = w["setup"].as_str().unwrap();
        let native_problem = if nm.is_empty() {
            let mut owner = Interpreter::with_builtins();
            let error = match owner.eval_source(source) {
                Ok(Value::Number(n)) if n == expected => None,
                Ok(v) => Some(format!("expected {expected}, received {v:?}")),
                Err(e) => Some(e.to_string()),
            };
            drop(owner);
            Interpreter::new().collect_cycles();
            error
        } else {
            None
        };
        let native_ok = nm.is_empty() && native_problem.is_none();
        let mut vm: Option<Interpreter> = None;
        let mut function: Option<Value> = None;
        let rt = Runtime::new().unwrap();
        let context = Context::full(&rt).unwrap();
        context.with(|ctx| {
            let quick_problem = if qm.is_empty() {
                let mut options = EvalOptions::default();
                options.strict = false;
                match ctx.eval_with_options::<f64, _>(source, options) {
                    Ok(n) if n == expected => None,
                    Ok(n) => Some(format!("expected {expected}, received {n}")),
                    Err(e) => Some(format!("{e:?}; {:?}", ctx.catch())),
                }
            } else {
                None
            };
            let quick_ok = qm.is_empty() && quick_problem.is_none();
            let mut quick: Option<Function> = None;
            for mode in ["fresh_source", "warm_source", "warm_call"] {
                if mode == "warm_source" {
                    if native_ok {
                        let mut owner = Interpreter::with_builtins();
                        owner.eval_source(setup).unwrap();
                        function = owner.global.borrow().get("bench");
                        vm = Some(owner);
                    }
                    if quick_ok {
                        qeval::<()>(&ctx, setup);
                        quick = Some(ctx.globals().get("bench").unwrap());
                    }
                }
                let mut native = || {
                    if mode == "fresh_source" {
                        nonce += 1;
                        let text = format!("{source}\n// fresh {nonce}");
                        let mut owner = Interpreter::with_builtins();
                        let value = number(owner.eval_source(black_box(&text)).unwrap());
                        drop(owner);
                        Interpreter::new().collect_cycles();
                        value
                    } else if mode == "warm_source" {
                        number(
                            vm.as_mut()
                                .unwrap()
                                .eval_source(black_box("bench();"))
                                .unwrap(),
                        )
                    } else {
                        number(
                            vm.as_mut()
                                .unwrap()
                                .call_host_function(
                                    black_box(function.as_ref().unwrap()),
                                    Value::Undefined,
                                    Vec::new(),
                                )
                                .unwrap(),
                        )
                    }
                };
                let mut qnonce = 0;
                let mut quick_call = || {
                    if mode == "fresh_source" {
                        qnonce += 1;
                        let text = format!("{source}\n// fresh {qnonce}");
                        let runtime = Runtime::new().unwrap();
                        let context = Context::full(&runtime).unwrap();
                        context.with(|ctx| qeval::<f64>(&ctx, black_box(&text)))
                    } else if mode == "warm_source" {
                        qeval::<f64>(&ctx, black_box("bench();"))
                    } else {
                        quick.as_ref().unwrap().call::<_, f64>(()).unwrap()
                    }
                };
                for _ in 0..5 {
                    if native_ok {
                        assert_eq!(native(), expected);
                    }
                    if quick_ok {
                        assert_eq!(quick_call(), expected);
                    }
                }
                Interpreter::new().collect_cycles();
                ctx.run_gc();
                let mut ns = Vec::new();
                let mut qs = Vec::new();
                for sample in 0..samples {
                    if sample % 2 == 0 {
                        if native_ok {
                            ns.push(measure(&mut native, expected, iterations));
                        }
                        if quick_ok {
                            qs.push(measure(&mut quick_call, expected, iterations));
                        }
                    } else {
                        if quick_ok {
                            qs.push(measure(&mut quick_call, expected, iterations));
                        }
                        if native_ok {
                            ns.push(measure(&mut native, expected, iterations));
                        }
                    }
                    Interpreter::new().collect_cycles();
                    ctx.run_gc();
                }
                rows.push(row("napi-vm", mode, w, ns, &nm, &native_problem));
                rows.push(row("rquickjs", mode, w, qs, &qm, &quick_problem));
            }
        });
        drop(function);
        drop(vm);
        Interpreter::new().collect_cycles();
    }
    println!(
        "{}",
        json!({"schema":1,"kind":"international_child","samples":samples,"iterations":iterations,"warmup_calls":5,"capabilities":{"napi-vm":native_caps,"rquickjs":quick_caps},"results":rows})
    );
}
