use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn worker(request: Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_napi-vm-test262"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn worker_separates_parse_runtime_and_harness_errors() {
    assert_eq!(worker(json!({"source":"1+2"}))["status"], "ok");
    let parse = worker(json!({"source":"var = 1"}));
    assert_eq!(parse["phase"], "parse");
    assert_eq!(parse["error_type"], "SyntaxError");
    let runtime = worker(json!({"source":"throw new TypeError('expected')"}));
    assert_eq!(runtime["phase"], "runtime");
    assert_eq!(runtime["error_type"], "TypeError");
    assert_eq!(
        worker(json!({"source":"1", "harness":"throw new Error('broken harness')"}))["phase"],
        "harness"
    );
}

#[test]
fn contextual_early_errors_precede_harness_or_runtime_execution() {
    for source in [
        "function f(){await 1;}",
        "class C{constructor(){}constructor(){}}",
        "class C{#x;static #x;}",
        "class C{m(){return this.#missing;}}",
        "class C{get x(a){}}",
        "class C{constructor(){super();}}",
        "try{}catch(x){let x;}",
    ] {
        let report =
            worker(json!({"source":source,"harness":"throw new Error('harness must not run');"}));
        assert_eq!(report["phase"], "parse", "{source}: {report}");
        assert_eq!(report["error_type"], "SyntaxError");
    }
    let report = worker(json!({"source":"export {missing};","module":true}));
    assert_eq!(report["phase"], "parse");
}
#[test]
fn worker_requires_exact_async_completion() {
    assert_eq!(
        worker(
            json!({"source":"Promise.resolve().then(function() { $DONE(); });", "asynchronous":true})
        )["status"],
        "ok"
    );
    assert_eq!(
        worker(
            json!({"source":"Promise.resolve().then(function() { $DONE(new TypeError('failed')); }).catch(function() {});", "asynchronous":true})
        )["error_type"],
        "TypeError"
    );
    assert_eq!(
        worker(json!({"source":"Promise.resolve().then(() => $DONE());", "asynchronous":true}))["status"],
        "ok"
    );
    assert_eq!(
        worker(json!({"source":"Promise.resolve();", "asynchronous":true}))["error_type"],
        "Test262AsyncError"
    );
    assert_eq!(
        worker(json!({"source":"$DONE(); $DONE();", "asynchronous":true}))["status"],
        "error"
    );
    assert_eq!(
        worker(json!({"source":"$DONE(new TypeError('failed'));", "asynchronous":true}))["error_type"],
        "TypeError"
    );
}
#[test]
fn worker_loads_module_fixtures() {
    let result = worker(json!({"id":"pkg/test.js", "module":true,
        "source":"import { answer } from './value_FIXTURE.js'; if (answer !== 42) throw new Error('wrong');",
        "modules":{"pkg/value_FIXTURE.js":"export const answer = 42;"}}));
    assert_eq!(result["status"], "ok", "{result}");
}
#[test]
fn worker_distinguishes_module_linking_and_evaluation_errors() {
    let linked = worker(
        json!({"id":"pkg/test.js","module":true,"source":"throw 1; import {missing} from './dep_FIXTURE.js';","modules":{"pkg/dep_FIXTURE.js":"export const available=1;"}}),
    );
    assert_eq!(linked["phase"], "resolution", "{linked}");
    assert_eq!(linked["error_type"], "SyntaxError");
    let evaluated = worker(
        json!({"id":"pkg/test.js","module":true,"source":"import {available} from './dep_FIXTURE.js';throw new TypeError('body');","modules":{"pkg/dep_FIXTURE.js":"export const available=1;"}}),
    );
    assert_eq!(evaluated["phase"], "runtime", "{evaluated}");
    assert_eq!(evaluated["error_type"], "TypeError");
}

#[test]
fn worker_eval_script_uses_the_global_environment() {
    let report = worker(
        json!({"source":"function local() { var hidden = 1; $262.evalScript('var installed=42;'); } local(); if(installed!==42 || $262.global!==globalThis) throw new Error('wrong realm global');"}),
    );
    assert_eq!(report["status"], "ok", "{report}");
}

#[test]
fn worker_loads_nested_and_dynamic_fixtures_within_the_explicit_root() {
    let root =
        std::env::temp_dir().join(format!("napi-vm-test262-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
    std::fs::write(root.join("value.js"), "export const answer=42;").unwrap();
    std::fs::write(
        root.join("nested/deeper/reexport.js"),
        "export {answer} from '../../value.js';",
    )
    .unwrap();
    let report = worker(json!({"id":"nested/test.js", "corpus_root":root,
        "source":"import('./deeper/reexport.js').then(function(m) { if(m.answer!==42) $DONE(new Error('wrong')); else $DONE(); }, $DONE);", "asynchronous":true}));
    assert_eq!(report["status"], "ok", "{report}");
    let outside = root.with_extension("outside.js");
    std::fs::write(&outside, "export default 42;").unwrap();
    let denied = worker(json!({"id":"test.js", "corpus_root":root, "module":true,
        "source":format!("import value from {};", serde_json::to_string(&outside.to_string_lossy()).unwrap())}));
    assert_eq!(denied["status"], "error", "{denied}");
    assert_eq!(denied["phase"], "resolution");
    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_detaches_buffers_and_accepts_gc_requests() {
    let result = worker(
        json!({"source": "var buffer=new ArrayBuffer(8);var view=new Uint8Array(buffer);$262.detachArrayBuffer(buffer);if(buffer.byteLength!==0 || view.length!==0) throw new Error('not detached');$262.detachArrayBuffer(buffer);gc();$262.gc();"}),
    );
    assert_eq!(result["status"], "ok", "{result}");
    for source in [
        "$262.detachArrayBuffer({});",
        "$262.detachArrayBuffer(new SharedArrayBuffer(8));",
        "$262.detachArrayBuffer(new ArrayBuffer(8), 'wrong-key');",
    ] {
        let result = worker(json!({"source": source}));
        assert_eq!(result["error_type"], "TypeError", "{result}");
    }
}

#[test]
fn worker_create_realm_has_distinct_globals_and_intrinsics() {
    let report = worker(
        json!({"source": "var realm=$262.createRealm();if(realm.global===globalThis || realm.global.Object===Object) throw new Error('shared realm');realm.evalScript('var realmSecret=17;');if(realm.global.realmSecret!==17 || typeof realmSecret!=='undefined') throw new Error('leaked global');var evalScript=realm.evalScript;if(evalScript('this')!==realm.global) throw new Error('lost host realm');var nested=realm.createRealm();if(nested.global===realm.global || nested.global.Array===realm.global.Array) throw new Error('shared nested realm');"}),
    );
    assert_eq!(report["status"], "ok", "{report}");
}

#[test]
fn worker_agents_share_memory_and_report_without_transferring_guest_state() {
    let result = worker(json!({"source": r#"
        var sab = new SharedArrayBuffer(16);
        var view = new Int32Array(sab);
        var source = `$262.agent.receiveBroadcast(function(sab, id) {
            var view = new Int32Array(sab);
            Atomics.add(view, 0, 1);
            Atomics.store(view, id, id * 10);
            $262.agent.report(id);
            $262.agent.leaving();
        });`;
        $262.agent.start(source);
        $262.agent.broadcast(sab, 1);
        $262.agent.start(source);
        $262.agent.broadcast(sab, 2);
        var reports = [];
        while (reports.length < 2) {
            var report = $262.agent.getReport();
            if (report !== null) reports.push(report);
            else $262.agent.sleep(1);
        }
        if (Atomics.load(view, 0) !== 2 || view[1] !== 10 || view[2] !== 20)
            throw new Error('shared memory or ids lost');
        if (reports[0] !== '1' || reports[1] !== '2') throw new Error('report order');
        if ($262.agent.getReport() !== null) throw new Error('empty report');
    "#}));
    assert_eq!(result["status"], "ok", "{result}");
}

#[test]
fn worker_agents_wait_notify_and_shutdown_blocked_waits() {
    let result = worker(json!({"source": r#"
        var sab = new SharedArrayBuffer(8);
        var view = new Int32Array(sab);
        $262.agent.start(`$262.agent.receiveBroadcast(function(sab) {
            var view = new Int32Array(sab);
            Atomics.store(view, 1, 1);
            $262.agent.report(Atomics.wait(view, 0, 0));
            $262.agent.leaving();
        });`);
        $262.agent.broadcast(sab);
        var count = 0;
        while (count === 0) {
            count = Atomics.notify(view, 0, 1);
            if (count === 0) $262.agent.sleep(1);
        }
        var report = null;
        while (report === null) {
            report = $262.agent.getReport();
            if (report === null) $262.agent.sleep(1);
        }
        if (report !== 'ok') throw new Error('notification lost');
        $262.agent.start(`$262.agent.receiveBroadcast(function(sab) {
            Atomics.wait(new Int32Array(sab), 0, 0);
        });`);
        $262.agent.broadcast(sab);
        $262.agent.shutdown();
    "#}));
    assert_eq!(result["status"], "ok", "{result}");
}

#[test]
fn worker_propagates_agent_errors_and_preserves_report_utf16() {
    let error = worker(json!({"source": r#"
        $262.agent.start("throw new TypeError('worker failure')");
        $262.agent.sleep(50);
        $262.agent.getReport();
    "#}));
    assert_eq!(error["status"], "error", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("worker failure")
    );
    let result = worker(json!({"source": r#"
        $262.agent.start("$262.agent.report('\\ud800'); $262.agent.leaving();");
        var report = null;
        while (report === null) {
            report = $262.agent.getReport();
            if (report === null) $262.agent.sleep(1);
        }
        if (report.length !== 1 || report.charCodeAt(0) !== 0xd800) throw new Error('UTF16');
    "#}));
    assert_eq!(result["status"], "ok", "{result}");
}

#[test]
fn worker_async_waits_settle_on_owner_after_foreign_notifications() {
    let result = worker(json!({"asynchronous":true,"source": r#"
        var sab = new SharedArrayBuffer(8);
        var view = new Int32Array(sab);
        var waiter = Atomics.waitAsync(view, 0, 0, 2000);
        waiter.value.then(function(result) {
            if (result !== 'ok') $DONE(new Error('notification was lost'));
            else $DONE();
        });
        $262.agent.start(`$262.agent.receiveBroadcast(function(sab, id) {
            if (id !== 9007199254740993n) throw new Error('BigInt broadcast id');
            Atomics.notify(new Int32Array(sab), 0, 1);
            $262.agent.leaving();
        });`);
        $262.agent.broadcast(sab, 9007199254740993n);
    "#}));
    assert_eq!(result["status"], "ok", "{result}");
}

#[test]
fn worker_agent_blocking_permission_is_explicit() {
    let result = worker(json!({"can_block":false,"source": r#"
        var view = new Int32Array(new SharedArrayBuffer(4));
        var throws = 0;
        try { Atomics.wait(view, 0, 1, 0); } catch (error) {
            if (!(error instanceof TypeError)) throw error;
            throws++;
        }
        if (throws !== 1) throw new Error('CanBlock was ignored');
    "#}));
    assert_eq!(result["status"], "ok", "{result}");
}

#[test]
fn worker_gc_keeps_registered_agent_callbacks_and_their_closures_alive() {
    let result = worker(json!({"source": r#"
        var sab = new SharedArrayBuffer(4);
        $262.agent.start(`
            (function() {
                var cycle = { value: 42 }; cycle.self = cycle;
                $262.agent.receiveBroadcast(function() {
                    $262.agent.report(cycle.self.value);
                    $262.agent.leaving();
                });
            })();
            $262.gc();
        `);
        $262.agent.broadcast(sab);
        var report = null;
        while (report === null) {
            report = $262.agent.getReport();
            if (report === null) $262.agent.sleep(1);
        }
        if (report !== '42') throw new Error('agent callback root was lost');
    "#}));
    assert_eq!(result["status"], "ok", "{result}");
}

#[test]
fn worker_agents_parse_source_with_script_goal() {
    let result = worker(json!({"source": r#"
        $262.agent.start('export const value = 1;');
        $262.agent.sleep(30);
        $262.agent.getReport();
    "#}));
    assert_eq!(result["status"], "error", "{result}");
    assert!(
        result["message"].as_str().unwrap().contains("module"),
        "{result}"
    );
}

#[test]
fn conformance_host_timers_use_the_owner_queue_and_can_be_cancelled() {
    let result = worker(json!({
        "source":"var start=Date.now();var cancelled=setTimeout(()=>{throw new Error('cancelled timer ran');},1);clearTimeout(cancelled);setTimeout((value)=>{if(value!==7||Date.now()-start<10)throw new Error('timer fired incorrectly');$DONE();},20,7);",
        "asynchronous":true
    }));
    assert_eq!(result["status"], "ok", "{result}");
}
