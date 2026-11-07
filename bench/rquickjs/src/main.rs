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
    (
        "bitwise_loop",
        "let s=0; for(let i=0;i<5000;i++){s=(s^i)&65535;} return s;",
        0.0,
    ),
    (
        "array_index_updates",
        "let a=new Array(1000).fill(0); for(let i=0;i<5000;i++){a[i%1000]++;} return a.reduce((s,x)=>s+x,0);",
        5000.0,
    ),
    (
        "array_sort_numeric",
        "let a=[]; for(let i=999;i>=0;i--){a.push(i);} a.sort((a,b)=>a-b); return a[0]+a[999];",
        999.0,
    ),
    (
        "map_set_get",
        "const m=new Map(); for(let i=0;i<1000;i++){m.set(i,i*2);} let s=0; for(let i=0;i<1000;i++){s+=m.get(i);} return s+m.size;",
        1_000_000.0,
    ),
    (
        "set_add_has_delete",
        "const s=new Set(); for(let i=0;i<1000;i++){s.add(i);} let n=0; for(let i=0;i<1000;i++){if(s.has(i)){n++;} if(i%2===0){s.delete(i);}} return n+s.size;",
        1500.0,
    ),
    (
        "typed_array_index",
        "const a=new Uint32Array(1000); let s=0; for(let i=0;i<1000;i++){a[i]=i*3;s+=a[i];} return s;",
        1_498_500.0,
    ),
    (
        "regexp_exec",
        "const re=/item(\\d+)/g; let s=0; for(let i=0;i<500;i++){re.lastIndex=0; const m=re.exec('item123 item456'); s+=Number(m[1]);} return s;",
        61_500.0,
    ),
    (
        "unicode_strings",
        "let s=0; for(let i=0;i<1000;i++){s+='A😀é'.length+'A😀é'.codePointAt(1);} return s;",
        128_516_000.0,
    ),
    (
        "prototype_getter",
        "const p={get x(){return this.y+1;}}; const o=Object.create(p); o.y=4; let s=0; for(let i=0;i<1000;i++){s+=o.x;} return s;",
        5000.0,
    ),
    (
        "property_key_churn",
        "let s=0; for(let i=0;i<300;i++){const o={a:i}; o['k'+i]=i; s+=o.a; delete o.a; s+=Object.keys(o).length;} return s;",
        45_150.0,
    ),
    (
        "exception_catch",
        "let s=0; for(let i=0;i<500;i++){try{throw i;}catch(e){s+=e;}} return s;",
        124_750.0,
    ),
    (
        "spread_destructure",
        "let s=0; const a=[1,2,3]; for(let i=0;i<500;i++){const [x,...rest]=[...a,4]; s+=x+rest.length;} return s;",
        2000.0,
    ),
    (
        "math_builtins",
        "let s=0; for(let i=0;i<1000;i++){s+=Math.floor(Math.sqrt(i*i))+Math.abs(-i);} return s;",
        999_000.0,
    ),
    (
        "bigint_arithmetic",
        "let s=0n; for(let i=0n;i<500n;i++){s+=i*2n;} return Number(s);",
        249_500.0,
    ),
    (
        "cyclic_allocations",
        "let n=0; for(let i=0;i<300;i++){const a={}; const b={peer:a}; a.peer=b; n+=a.peer.peer===a?1:0;} return n;",
        300.0,
    ),
    (
        "generator_iteration",
        "function* values(){for(let i=0;i<200;i++)yield i;}let s=0;for(const n of values())s+=n;return s;",
        19_900.0,
    ),
    (
        "custom_iterator",
        "const a={[Symbol.iterator](){let n=0;return{next(){return n<200?{value:n++,done:false}:{done:true};}};}};let s=0;for(const n of a)s+=n;return s;",
        19_900.0,
    ),
    (
        "proxy_reflect_get",
        "const p=new Proxy({x:1},{get(t,k,r){return Reflect.get(t,k,r)+1;}});let s=0;for(let i=0;i<1000;i++)s+=p.x;return s;",
        2000.0,
    ),
    (
        "reflect_define_property",
        "const o={};let s=0;for(let i=0;i<500;i++){Reflect.defineProperty(o,'k'+i,{value:i,enumerable:true,configurable:true,writable:true});s+=Reflect.get(o,'k'+i);}return s;",
        124_750.0,
    ),
    (
        "reflect_own_keys",
        "const o=Object.create(null);for(let i=0;i<100;i++)o['k'+i]=i;Object.defineProperty(o,'hidden',{value:1});let s=0;for(let i=0;i<100;i++)s+=Reflect.ownKeys(o).length+Object.keys(o).length;return s;",
        20_100.0,
    ),
    (
        "date_utc_fields",
        "const d=new Date(Date.UTC(2020,0,2,3,4,5));let s=0;for(let i=0;i<500;i++)s+=d.getUTCFullYear()+d.getUTCMonth()+d.getUTCDate()+d.getUTCHours()+d.getUTCMinutes()+d.getUTCSeconds();return s;",
        1_017_000.0,
    ),
    (
        "date_iso_parse",
        "const n=Date.UTC(2020,0,2,3,4,5);const text=new Date(n).toISOString();let s=0;for(let i=0;i<200;i++)if(Date.parse(text)===n)s++;return s;",
        200.0,
    ),
    (
        "bigint_division_modulo",
        "const n=12345678901234567890n;let s=0n;for(let i=1n;i<=200n;i++)s+=(n/i)%97n;return Number(s);",
        9320.0,
    ),
    (
        "string_replace_callback",
        "let s=0;for(let i=0;i<500;i++)if('v1-v2-v3'.replace(/(\\d+)/g,(_,n)=>String(Number(n)+1))==='v2-v3-v4')s++;return s;",
        500.0,
    ),
    (
        "data_view_read_write",
        "const d=new DataView(new ArrayBuffer(4096));let s=0;for(let i=0;i<1000;i++){d.setUint32(i*4,i,true);s+=d.getUint32(i*4,true);}return s;",
        499_500.0,
    ),
    (
        "typed_array_bulk_set",
        "const a=new Uint8Array(4096).fill(7);const b=new Uint8Array(8192);for(let i=0;i<100;i++)b.set(a,2048);return b[2048]+b[6143]+b[0];",
        14.0,
    ),
    (
        "weak_map_objects",
        "const keys=[];const m=new WeakMap();for(let i=0;i<256;i++){const k={id:i};keys.push(k);m.set(k,i);}let s=0;for(let i=0;i<256;i++)s+=m.get(keys[i]);return s;",
        32_640.0,
    ),
    (
        "weak_set_objects",
        "const keys=[];const s=new WeakSet();for(let i=0;i<256;i++){const k={id:i};keys.push(k);s.add(k);}let n=0;for(let i=0;i<256;i++){if(s.has(keys[i]))n++;if(i%2===0)s.delete(keys[i]);}for(let i=0;i<256;i++)if(s.has(keys[i]))n++;return n;",
        384.0,
    ),
    (
        "array_buffer_slice",
        "const a=new ArrayBuffer(4096);new Uint8Array(a).fill(7);let b;for(let i=0;i<100;i++)b=a.slice(1024,3072);return b.byteLength+new Uint8Array(b)[2047];",
        2055.0,
    ),
    (
        "string_normalization",
        "let n=0;for(let i=0;i<500;i++)if('e\\u0301'.normalize('NFC')==='é')n++;return n;",
        500.0,
    ),
    (
        "sparse_array_map",
        "const a=new Array(1000);for(let i=0;i<1000;i+=2)a[i]=i;const b=a.map(x=>x+1);let s=0;for(let i=0;i<b.length;i++)if(i in b)s+=b[i];return s;",
        250_000.0,
    ),
];

fn all_workloads() -> Vec<(String, String, f64, String)> {
    let mut workloads: Vec<_> = WORKLOADS
        .iter()
        .map(|(name, body, expected)| {
            (name.to_string(), body.to_string(), *expected, String::new())
        })
        .collect();
    // Warm lookup fixtures are populated before timing. Fresh-source mode still
    // includes that setup, as it does for all source and builtin initialization.
    for size in [16, 64, 256, 1024] {
        for kind in ["Map", "Set"] {
            let insert = if kind == "Map" { "set(i,i)" } else { "add(i)" };
            workloads.push((
                format!("collection_{}_construct_{size}", kind.to_lowercase()),
                format!("const m=new {kind}();for(let i=0;i<{size};i++)m.{insert};return m.size;"),
                size as f64,
                String::new(),
            ));
        }
        for (keys, key, prepare) in [
            ("numbers", "i", String::new()),
            ("strings", "'key'+i", String::new()),
            (
                "objects",
                "collectionKeys[i]",
                format!(
                    "var collectionKeys=[];for(var j=0;j<{size};j++)collectionKeys.push({{id:j}});"
                ),
            ),
        ] {
            workloads.push((
                format!("collection_map_lookup_{keys}_{size}"),
                format!("let s=0;for(let i=0;i<{size};i++)s+=collectionFixture.get({key});return s;"),
                (size * (size - 1) / 2) as f64,
                format!("{prepare}var collectionFixture=new Map();for(var i=0;i<{size};i++)collectionFixture.set({key},i);\n"),
            ));
        }
        workloads.push((
            format!("collection_set_has_{size}"),
            format!("let s=0;for(let i=0;i<{size};i++){{if(collectionFixture.has(i))s++;if(collectionFixture.has(i+{size}))s++;}}return s;"),
            size as f64,
            format!("var collectionFixture=new Set();for(var i=0;i<{size};i++)collectionFixture.add(i);\n"),
        ));
    }
    workloads
}

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
    if std::env::args().nth(1).as_deref() == Some("--emit-workloads") {
        let workloads: Vec<_> = all_workloads()
            .into_iter()
            .map(|(name, body, expected, prelude)| {
                let setup = format!("{prelude}function bench(){{{body}}}");
                let source = format!("{setup}\nbench();");
                let hash = format!("{:x}", Sha256::digest(source.as_bytes()));
                json!({"name":name,"setup":setup,"source":source,"invocation":"bench();",
                       "expected":expected,"source_sha256":hash})
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"schema":1,"kind":"guest_workloads","workloads":workloads})
            )
            .unwrap()
        );
        return;
    }
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
    for (name, body, expected, prelude) in all_workloads() {
        if !name.contains(&filter) {
            continue;
        }
        let setup = format!("{prelude}function bench(){{{body}}}");
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
        results.push(row("napi-vm", "fresh_source", &name, ns, &hash));
        results.push(row("rquickjs", "fresh_source", &name, qs, &hash));

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
                results.push(row("napi-vm", mode, &name, ns, &hash));
                results.push(row("rquickjs", mode, &name, qs, &hash));
            }
        });
        drop(function);
        drop(vm);
        Interpreter::new().collect_cycles();
    }
    assert!(!results.is_empty(), "filter matched no workloads");
    println!("{}",serde_json::to_string_pretty(&json!({"schema":1,"rquickjs_version":"0.14.0","napi_vm_version":"0.1.5","features":{"napi-vm-core":"default (no napi/wasm/runtime capabilities)","rquickjs":"default (std), Context::full"},"samples":samples,"iterations_per_sample":iterations,"warmup_calls":5,"workloads":workloads,"results":results})).unwrap());
}
