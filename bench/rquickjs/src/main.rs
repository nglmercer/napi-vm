use napi_vm_core::{Interpreter, Value};
use rquickjs::{Context, Function, Runtime, context::EvalOptions};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{hint::black_box, time::Instant};

// Every body has a stable numeric result, checked after every timed batch.
const WORKLOADS: &[(&str, &str, f64)] = &[
    ("startup_dominated", "return 1;", 1.0),
    (
        "arithmetic_loop",
        "let s=0; for(let i=0;i<10000;i++){s+=i*2-1;} return s;",
        99_980_000.0,
    ),
    (
        "recursion_fib",
        "function fib(n){return n<2?n:fib(n-1)+fib(n-2);} return fib(10);",
        55.0,
    ),
    (
        "array_chain",
        "let a=[]; for(let i=0;i<1000;i++){a.push(i);} return a.map(x=>x*2).filter(x=>x%3===0).reduce((s,x)=>s+x,0);",
        333_666.0,
    ),
    (
        "string_ops",
        "let a=[]; for(let i=0;i<1000;i++){a.push('item'+i);} return a.join(',').split(',').length;",
        1000.0,
    ),
    (
        "closures",
        "function counter(){let n=0; return ()=>++n;} const c=counter(); for(let i=0;i<1000;i++){c();} return c();",
        1001.0,
    ),
    (
        "json_roundtrip",
        "const o={a:1,b:[1,2,3],c:{d:'x',e:[true,null]}}; let r; for(let i=0;i<200;i++){r=JSON.parse(JSON.stringify(o));} return r.c.e.length+r.b.length;",
        5.0,
    ),
    (
        "property_reads",
        "const o={x:3,y:5}; let s=0; for(let i=0;i<5000;i++){s+=o.x+o.y;} return s;",
        40_000.0,
    ),
    (
        "property_updates",
        "const o={x:0,y:0}; for(let i=0;i<3000;i++){o.x++;o.y+=2;} return o.x+o.y;",
        9000.0,
    ),
    (
        "class_methods",
        "class P{constructor(x,y){this.x=x;this.y=y;} sum(){return this.x+this.y;}} let t=0; for(let i=0;i<1000;i++){t+=new P(i,i+1).sum();} return t;",
        1_000_000.0,
    ),
];

fn number(v: Value) -> f64 {
    match v {
        Value::Number(n) => n,
        other => panic!("unexpected result: {other:?}"),
    }
}

fn qeval(ctx: &rquickjs::Ctx<'_>, source: &str) -> f64 {
    // rquickjs defaults to strict scripts; napi-vm defaults to sloppy scripts.
    let mut options = EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options(source, options)
        .unwrap_or_else(|e| panic!("rquickjs eval failed: {e:?}; {:?}", ctx.catch()))
}

fn measure(mut call: impl FnMut() -> f64, expected: f64, iterations: usize) -> f64 {
    let mut sum = 0.0;
    let start = Instant::now();
    for _ in 0..iterations {
        sum += black_box(call());
    }
    let elapsed = start.elapsed().as_nanos() as f64 / iterations as f64;
    assert_eq!(
        sum,
        expected * iterations as f64,
        "benchmark returned wrong result"
    );
    elapsed
}

fn row(engine: &str, mode: &str, workload: &str, mut samples: Vec<f64>, hash: &str) -> Json {
    assert!(samples.iter().all(|n| n.is_finite() && *n > 0.0));
    let original = samples.clone();
    samples.sort_by(f64::total_cmp);
    let median = samples[samples.len() / 2];
    let mut deviations: Vec<_> = samples.iter().map(|v| (v - median).abs()).collect();
    deviations.sort_by(f64::total_cmp);
    json!({"engine":engine,"mode":mode,"workload":workload,"unit":"ns/call","source_sha256":hash,"samples":original,"median":median,"mad":deviations[deviations.len()/2]})
}

fn main() {
    let mut samples = 15;
    let mut iterations = 10;
    let mut filter = String::new();
    let args: Vec<_> = std::env::args().skip(1).collect();
    for chunk in args.chunks(2) {
        assert_eq!(
            chunk.len(),
            2,
            "expected --samples N --iterations N --filter text"
        );
        match chunk[0].as_str() {
            "--samples" => samples = chunk[1].parse().unwrap(),
            "--iterations" => iterations = chunk[1].parse().unwrap(),
            "--filter" => filter = chunk[1].clone(),
            other => panic!("unknown option {other}"),
        }
    }
    assert!(
        samples > 0 && samples % 2 == 1 && iterations > 0,
        "samples must be positive and odd; iterations positive"
    );
    let mut results = Vec::new();
    let mut workloads = Vec::new();
    for &(name, body, expected) in WORKLOADS {
        if !name.contains(&filter) {
            continue;
        }
        let setup = format!("function bench(){{{body}}}");
        let source = format!("{setup}\nbench();");
        let hash = format!("{:x}", Sha256::digest(source.as_bytes()));
        let prepared = Interpreter::compile(&source).unwrap();
        let top_level_tier = format!("{:?}", prepared.tier());
        let fallback = prepared.fallback_reason();
        drop(prepared);
        // Keep VM creation/destruction and builtin setup in fresh measurements.
        // Unique comments defeat napi-vm's process-wide parse cache for this mode.
        let mut nonce = 0;
        let mut fresh_collector = Interpreter::new();
        let mut fresh_napi = || {
            nonce += 1;
            let text = format!("{source}\n// fresh {nonce}");
            let mut vm = Interpreter::with_builtins();
            let result = number(vm.eval_source(black_box(&text)).unwrap());
            drop(vm);
            fresh_collector.collect_cycles();
            result
        };
        let mut qnonce = 0;
        let mut fresh_quick = || {
            qnonce += 1;
            let text = format!("{source}\n// fresh {qnonce}");
            let rt = Runtime::new().unwrap();
            let ctx = Context::full(&rt).unwrap();
            ctx.with(|ctx| qeval(&ctx, black_box(&text)))
        };
        assert_eq!(fresh_napi(), expected);
        assert_eq!(fresh_quick(), expected);
        let mut ns = Vec::new();
        let mut qs = Vec::new();
        for sample in 0..samples {
            // Alternate order to reduce systematic temperature/order bias.
            if sample % 2 == 0 {
                ns.push(measure(&mut fresh_napi, expected, iterations));
                qs.push(measure(&mut fresh_quick, expected, iterations));
            } else {
                qs.push(measure(&mut fresh_quick, expected, iterations));
                ns.push(measure(&mut fresh_napi, expected, iterations));
            }
        }
        results.push(row("napi-vm", "fresh_source", name, ns, &hash));
        results.push(row("rquickjs", "fresh_source", name, qs, &hash));

        let mut vm = Interpreter::with_builtins();
        vm.eval_source(&setup).unwrap();
        let function = vm.global.borrow().get("bench").unwrap();
        let function_tier = match &function {
            Value::Function(f) if f.bytecode.is_some() => "bytecode",
            Value::Function(_) => "ast",
            _ => panic!("bench is not a function"),
        };
        workloads.push(json!({"name":name,"source_sha256":hash,"expected":expected,"top_level_tier":top_level_tier,"fallback_reason":fallback,"guest_function_tier":function_tier}));
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        let mut warm_collector = Interpreter::new();
        ctx.with(|ctx| {
            let mut options = EvalOptions::default();
            options.strict = false;
            ctx.eval_with_options::<(), _>(setup.as_str(), options)
                .unwrap();
            let quick_function: Function = ctx.globals().get("bench").unwrap();
            for mode in ["warm_source", "warm_call"] {
                let mut napi = || {
                    if mode == "warm_source" {
                        number(vm.eval_source(black_box("bench();")).unwrap())
                    } else {
                        number(
                            vm.call_host_function(
                                black_box(&function),
                                Value::Undefined,
                                Vec::new(),
                            )
                            .unwrap(),
                        )
                    }
                };
                let mut quick = || {
                    if mode == "warm_source" {
                        qeval(&ctx, black_box("bench();"))
                    } else {
                        quick_function.call::<_, f64>(()).unwrap()
                    }
                };
                for _ in 0..5 {
                    assert_eq!(napi(), expected);
                    assert_eq!(quick(), expected);
                }
                let mut ns = Vec::new();
                let mut qs = Vec::new();
                for sample in 0..samples {
                    if sample % 2 == 0 {
                        ns.push(measure(&mut napi, expected, iterations));
                        qs.push(measure(&mut quick, expected, iterations));
                    } else {
                        qs.push(measure(&mut quick, expected, iterations));
                        ns.push(measure(&mut napi, expected, iterations));
                    }
                    // Bound cyclic garbage between warm batches on both engines.
                    // Collection is outside the latency measurement, unlike fresh_source.
                    warm_collector.collect_cycles();
                    ctx.run_gc();
                }
                results.push(row("napi-vm", mode, name, ns, &hash));
                results.push(row("rquickjs", mode, name, qs, &hash));
            }
        });
        drop(function);
        drop(vm);
        Interpreter::new().collect_cycles();
    }
    assert!(!results.is_empty(), "filter matched no workloads");
    println!("{}",serde_json::to_string_pretty(&json!({"schema":1,"rquickjs_version":"0.14.0","napi_vm_version":"0.1.5","features":{"napi-vm-core":"default (no napi/wasm/runtime capabilities)","rquickjs":"default (std), Context::full"},"samples":samples,"iterations_per_sample":iterations,"warmup_calls":5,"workloads":workloads,"results":results})).unwrap());
}
