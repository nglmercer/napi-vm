use napi_vm_core::interpreter::{DrainPolicy, EvaluationOptions};
use napi_vm_core::{Interpreter, Value};
use rquickjs::{Context, Function, Runtime, context::EvalOptions};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{hint::black_box, time::Instant};

const CASES: &[(&str, &str, f64)] = &[
    ("resolved", "return Promise.resolve(42);", 42.0),
    (
        "chain_128",
        "let p=Promise.resolve(0);for(let i=0;i<128;i++)p=p.then(x=>x+1);return p;",
        128.0,
    ),
    (
        "fanout_128",
        "const p=Promise.resolve(2),a=[];for(let i=0;i<128;i++)a.push(p.then(x=>x+i));return Promise.all(a).then(a=>a.reduce((s,x)=>s+x,0));",
        8384.0,
    ),
    (
        "all_values_128",
        "return Promise.all(Array.from({length:128},(_,i)=>i)).then(a=>a.reduce((s,x)=>s+x,0));",
        8128.0,
    ),
    (
        "all_promises_128",
        "return Promise.all(Array.from({length:128},(_,i)=>Promise.resolve(i))).then(a=>a.reduce((s,x)=>s+x,0));",
        8128.0,
    ),
    (
        "all_settled_128",
        "return Promise.allSettled(Array.from({length:128},(_,i)=>i%2?Promise.reject(i):Promise.resolve(i))).then(a=>a.reduce((s,x)=>s+(x.status==='fulfilled'?x.value:x.reason),0));",
        8128.0,
    ),
    (
        "race_128",
        "return Promise.race(Array.from({length:128},(_,i)=>Promise.resolve(i)));",
        0.0,
    ),
    (
        "any_reject_127",
        "return Promise.any(Array.from({length:128},(_,i)=>i===127?Promise.resolve(i):Promise.reject(i)));",
        127.0,
    ),
    (
        "thenable_chain_64",
        "let p=Promise.resolve(0);for(let i=0;i<64;i++)p=p.then(x=>({then(resolve){resolve(x+1);}}));return p;",
        64.0,
    ),
    (
        "catch_finally_64",
        "let p=Promise.resolve(0);for(let i=0;i<64;i++)p=p.then(x=>{throw x+1;}).catch(x=>x).finally(()=>{});return p;",
        64.0,
    ),
    (
        "await_values_128",
        "return (async()=>{let s=0;for(let i=0;i<128;i++)s+=await i;return s;})();",
        8128.0,
    ),
    (
        "await_promises_128",
        "return (async()=>{let s=0;for(let i=0;i<128;i++)s+=await Promise.resolve(i);return s;})();",
        8128.0,
    ),
    (
        "async_generator_64",
        "return (async()=>{async function* values(){for(let i=0;i<64;i++)yield i;}let s=0;for await(const x of values())s+=x;return s;})();",
        2016.0,
    ),
];
const MODES: &[&str] = &[
    "enqueue",
    "checkpoint",
    "source_completed",
    "call_completed",
];

fn fixtures() -> Vec<Json> {
    CASES.iter().map(|(name,body,expected)| {
        let setup=format!("var __done=0,__value=-1,__error='';function bench(){{{body}}}function start(){{__done=0;__value=-1;__error='';bench().then(v=>{{__value=v;__done++;}},e=>{{__error=String(e);__done++;}});}}");
        let source=format!("{setup}\nstart();");
        json!({"name":name,"setup":setup,"invocation":"start();","source":source,"expected":expected,"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes()))})
    }).collect()
}

fn deferred() -> EvaluationOptions {
    EvaluationOptions {
        drain: DrainPolicy::None,
        resume_pending_checkpoint: false,
        retire_execution: false,
    }
}
fn check_napi(vm: &Interpreter, expected: f64) {
    let global = vm.global.borrow();
    assert!(
        matches!(global.get("__done"), Some(Value::Number(1.0))),
        "missing or duplicate completion"
    );
    assert!(
        matches!(global.get("__value"),Some(Value::Number(n)) if n==expected),
        "wrong completion value"
    );
    assert!(
        matches!(global.get("__error"),Some(Value::String(ref s)) if s.is_empty()),
        "guest rejected"
    );
}
fn check_quick(ctx: &rquickjs::Ctx<'_>, expected: f64) {
    assert_eq!(
        ctx.globals().get::<_, i32>("__done").unwrap(),
        1,
        "missing or duplicate completion"
    );
    assert_eq!(ctx.globals().get::<_, f64>("__value").unwrap(), expected);
    assert_eq!(ctx.globals().get::<_, String>("__error").unwrap(), "");
}
fn qeval(ctx: &rquickjs::Ctx<'_>, source: &str) {
    let mut options = EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options::<(), _>(source, options)
        .unwrap_or_else(|e| panic!("eval: {e:?}; {:?}", ctx.catch()));
}
fn qpending(ctx: &rquickjs::Ctx<'_>) -> bool {
    // Context::with holds the runtime RefCell borrow; the Runtime job methods
    // cannot be called recursively here. Use the same FFI as Ctx's job method,
    // retaining its error status rather than discarding thrown job exceptions.
    unsafe { rquickjs::qjs::JS_IsJobPending(rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr())) }
}
fn qdrain(ctx: &rquickjs::Ctx<'_>) -> usize {
    let mut jobs = 0;
    loop {
        let mut job_context = std::ptr::null_mut();
        let status = unsafe {
            rquickjs::qjs::JS_ExecutePendingJob(
                rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr()),
                &mut job_context,
            )
        };
        assert!(status >= 0, "uncaught job error: {:?}", ctx.catch());
        if status == 0 {
            break;
        }
        jobs += 1;
        assert!(jobs <= 100_000, "runaway job queue");
    }
    jobs
}

fn row(engine: &str, mode: &str, w: &Json, values: Vec<f64>) -> Json {
    let mut sorted = values.clone();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted[sorted.len() / 2];
    let mut dev: Vec<_> = values.iter().map(|x| (x - mid).abs()).collect();
    dev.sort_by(f64::total_cmp);
    json!({"engine":engine,"mode":mode,"workload":w["name"],"source_sha256":w["source_sha256"],"unit":"ns/operation","samples":values,"median":mid,"mad":dev[dev.len()/2]})
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let workloads = fixtures();
    if args.get(1).map(String::as_str) == Some("--emit-workloads") {
        println!(
            "{}",
            json!({"schema":1,"kind":"async_workloads","workloads":workloads})
        );
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
    let filter = option("--filter", "");
    assert!(samples > 0 && samples % 2 == 1 && iterations > 0);
    let mut results = Vec::new();
    let mut selected = Vec::new();
    let mut job_counts = Vec::new();
    for w in workloads
        .iter()
        .filter(|w| w["name"].as_str().unwrap().contains(&filter))
    {
        let mut vm = Interpreter::with_builtins();
        vm.eval_source(w["setup"].as_str().unwrap()).unwrap();
        let fun = vm.global.borrow().get("start").unwrap();
        let rt = Runtime::new().unwrap();
        let context = Context::full(&rt).unwrap();
        context.with(|ctx| {
            qeval(&ctx, w["setup"].as_str().unwrap());
            let quick: Function = ctx.globals().get("start").unwrap();
            let expected = w["expected"].as_f64().unwrap();
            let source = w["invocation"].as_str().unwrap();
            // Count actual jobs once outside timing. Counts are engine-specific diagnostics.
            vm.eval_source_with_options(source, deferred()).unwrap();
            let count = vm
                .poll_event_loop(napi_vm_core::TurnBudget::jobs(usize::MAX))
                .unwrap()
                .executed_jobs;
            check_napi(&vm, expected);
            qeval(&ctx, source);
            let qcount = qdrain(&ctx);
            check_quick(&ctx, expected);
            job_counts.push(json!({"workload":w["name"],"napi_jobs":count,"rquickjs_jobs":qcount}));
            for mode in MODES {
                let mut napi = || {
                    if *mode == "checkpoint" {
                        vm.eval_source_with_options(source, deferred()).unwrap();
                    }
                    let start = Instant::now();
                    match *mode {
                        "enqueue" => {
                            vm.eval_source_with_options(black_box(source), deferred())
                                .unwrap();
                        }
                        "checkpoint" => vm.drain_jobs().unwrap(),
                        "source_completed" => {
                            vm.eval_source(black_box(source)).unwrap();
                        }
                        "call_completed" => {
                            vm.call_host_function(black_box(&fun), Value::Undefined, Vec::new())
                                .unwrap();
                        }
                        _ => unreachable!(),
                    }
                    let elapsed = start.elapsed().as_nanos() as f64;
                    if *mode == "enqueue" {
                        vm.drain_jobs().unwrap();
                    }
                    check_napi(&vm, expected);
                    elapsed
                };
                let mut quick_call = || {
                    assert!(!qpending(&ctx));
                    if *mode == "checkpoint" {
                        qeval(&ctx, source);
                    }
                    let start = Instant::now();
                    match *mode {
                        "enqueue" => qeval(&ctx, black_box(source)),
                        "checkpoint" => {
                            qdrain(&ctx);
                        }
                        "source_completed" => {
                            qeval(&ctx, black_box(source));
                            qdrain(&ctx);
                        }
                        "call_completed" => {
                            quick.call::<_, ()>(()).unwrap();
                            qdrain(&ctx);
                        }
                        _ => unreachable!(),
                    }
                    let elapsed = start.elapsed().as_nanos() as f64;
                    if *mode == "enqueue" {
                        qdrain(&ctx);
                    }
                    check_quick(&ctx, expected);
                    elapsed
                };
                for _ in 0..5 {
                    napi();
                    quick_call();
                }
                let mut n = Vec::new();
                let mut q = Vec::new();
                for sample in 0..samples {
                    let measure = |call: &mut dyn FnMut() -> f64| {
                        (0..iterations).map(|_| black_box(call())).sum::<f64>() / iterations as f64
                    };
                    if sample % 2 == 0 {
                        n.push(measure(&mut napi));
                        q.push(measure(&mut quick_call));
                    } else {
                        q.push(measure(&mut quick_call));
                        n.push(measure(&mut napi));
                    }
                    Interpreter::new().collect_cycles();
                    ctx.run_gc();
                }
                results.push(row("napi-vm", mode, w, n));
                results.push(row("rquickjs", mode, w, q));
            }
        });
        selected.push(w.clone());
        drop(fun);
        drop(vm);
        Interpreter::new().collect_cycles();
    }
    assert!(!results.is_empty());
    println!("{}",serde_json::to_string_pretty(&json!({"schema":1,"kind":"async_job_timings","samples":samples,"iterations":iterations,"warmup_calls":5,"workloads":selected,"results":results,"job_counts":job_counts,"limitations":["Warm synchronous host API calls that enqueue and/or drain promise jobs; no I/O, timers, Node or WASM bindings.","enqueue times source evaluation only; its job cleanup is outside timing. checkpoint prepares pending jobs outside timing.","source_completed includes source evaluation and all pending jobs. call_completed retains start() and includes a checkpoint on both engines.","Every invocation checks exactly one numeric completion and no rejection outside timing. Cyclic GC runs outside each batch on both engines.","napi-vm caches invocation source; rquickjs reparses it. Context construction and setup are excluded.","Job counts are engine-specific diagnostics, not equal units of work. Five warmups do not prove equal tiers.","Samples are per-call timer averages; no host isolation or enforced affinity."]})).unwrap());
}
