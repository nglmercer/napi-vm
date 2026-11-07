//! Isolated-process Linux RSS/GC probes. Instantiate exactly one engine per run.
use napi_vm_core::{Interpreter, Value};
use rquickjs::{Context, Function, Runtime, context::EvalOptions};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::time::Instant;

fn snapshot(phase: &str, batch: usize, gc_ns: Option<u128>) -> Json {
    let status = std::fs::read_to_string("/proc/self/status")
        .expect("memory probes require Linux /proc/self/status");
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .expect("missing process memory field")
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            * 1024
    };
    json!({"phase":phase,"batch":batch,"rss_bytes":field("VmRSS:"),
           "process_high_water_bytes":field("VmHWM:"),"gc_ns":gc_ns})
}

fn napi_snapshot(vm: &Interpreter, phase: &str, batch: usize, gc_ns: Option<u128>) -> Json {
    let mut result = snapshot(phase, batch, gc_ns);
    let stats = vm.runtime_stats();
    result["engine_stats"] = json!({"heap_tracked":stats.heap_tracked,
        "heap_collected_total":stats.heap_collected_total,"shapes_created":stats.shapes_created});
    result
}

fn quick_snapshot(rt: &Runtime, phase: &str, batch: usize, gc_ns: Option<u128>) -> Json {
    let mut result = snapshot(phase, batch, gc_ns);
    let stats = rt.memory_usage();
    result["engine_stats"] = json!({"malloc_size":stats.malloc_size,"memory_used_size":stats.memory_used_size,
        "obj_count":stats.obj_count,"shape_count":stats.shape_count,"atom_count":stats.atom_count});
    result
}

fn main() {
    let (mut engine, mut workload) = (String::new(), String::new());
    let (mut iterations, mut objects) = (8usize, 2000usize);
    let args: Vec<_> = std::env::args().skip(1).collect();
    for pair in args.chunks(2) {
        assert_eq!(pair.len(), 2);
        match pair[0].as_str() {
            "--engine" => engine = pair[1].clone(),
            "--workload" => workload = pair[1].clone(),
            "--iterations" => iterations = pair[1].parse().unwrap(),
            "--objects" => objects = pair[1].parse().unwrap(),
            flag => panic!("unknown argument {flag}"),
        }
    }
    assert!(objects > 0 && iterations > 0);
    let body = match workload.as_str() {
        "objects" => "let a=[];for(let i=0;i<COUNT;i++){a.push({x:i,y:i+1});}return a.length;",
        "cycles" => "for(let i=0;i<COUNT;i++){let a={};let b={peer:a};a.peer=b;}return COUNT;",
        "unique_keys" => "let a=[];for(let i=0;i<COUNT;i++){let o={x:i};o['key_'+generation+'_'+i]=i;if(o.x+o.x!==i*2){throw Error('read mismatch');}a.push(o);}return a.length;",
        "live_cycles" => "retained=[];for(let i=0;i<COUNT;i++){let a={};let b={peer:a};a.peer=b;retained.push(a);}return retained.length;",
        other => panic!("unknown workload {other}"),
    }.replace("COUNT", &objects.to_string());
    let setup =
        format!("var generation=0;var retained=null;function bench(){{generation++;{body}}}");
    let hash = format!("{:x}", Sha256::digest(setup.as_bytes()));
    let mut snapshots = vec![snapshot("baseline", 0, None)];
    let mut tier = "engine bytecode";
    match engine.as_str() {
        "napi-vm" => {
            let mut vm = Interpreter::with_builtins();
            vm.eval_source(&setup).unwrap();
            let function = vm.global.borrow().get("bench").unwrap();
            tier = match &function {
                Value::Function(f) if f.bytecode.is_some() => "bytecode",
                Value::Function(_) => "ast",
                _ => panic!("bench must be callable"),
            };
            vm.collect_cycles();
            snapshots.push(napi_snapshot(&vm, "ready", 0, None));
            for batch in 1..=iterations {
                let value = vm
                    .call_host_function(&function, Value::Undefined, vec![])
                    .unwrap();
                assert!(matches!(value, Value::Number(n) if n == objects as f64));
                snapshots.push(napi_snapshot(&vm, "allocated", batch, None));
                let started = Instant::now();
                vm.collect_cycles();
                let elapsed = started.elapsed().as_nanos();
                snapshots.push(napi_snapshot(&vm, "collected", batch, Some(elapsed)));
            }
            vm.eval_source("retained=null;").unwrap();
            vm.collect_cycles();
            snapshots.push(napi_snapshot(&vm, "released", iterations, None));
            drop(function);
            drop(vm);
            Interpreter::new().collect_cycles();
        }
        "rquickjs" => {
            let rt = Runtime::new().unwrap();
            let ctx = Context::full(&rt).unwrap();
            ctx.with(|ctx| {
                let mut options = EvalOptions::default();
                options.strict = false;
                ctx.eval_with_options::<(), _>(setup.as_str(), options)
                    .unwrap();
            });
            rt.run_gc();
            snapshots.push(quick_snapshot(&rt, "ready", 0, None));
            for batch in 1..=iterations {
                ctx.with(|ctx| {
                    let function: Function = ctx.globals().get("bench").unwrap();
                    assert_eq!(function.call::<_, f64>(()).unwrap(), objects as f64);
                });
                snapshots.push(quick_snapshot(&rt, "allocated", batch, None));
                let started = Instant::now();
                rt.run_gc();
                let elapsed = started.elapsed().as_nanos();
                snapshots.push(quick_snapshot(&rt, "collected", batch, Some(elapsed)));
            }
            ctx.with(|ctx| {
                ctx.eval::<(), _>("retained=null;").unwrap();
            });
            rt.run_gc();
            snapshots.push(quick_snapshot(&rt, "released", iterations, None));
            drop(ctx);
            rt.run_gc();
            drop(rt);
        }
        other => panic!("unknown engine {other}"),
    }
    snapshots.push(snapshot("dropped", iterations, None));
    println!(
        "{}",
        json!({"schema":1,"engine":engine,"workload":workload,
        "objects_per_call":objects,"iterations":iterations,"source":setup,"source_sha256":hash,
        "guest_function_tier":tier,"snapshots":snapshots})
    );
}
