use napi_vm::{Interpreter, TurnBudget};

fn run(source: &str, result: &str) -> String {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).expect("initial evaluation");
    vm.poll_event_loop(TurnBudget::jobs(10_000))
        .expect("owner jobs");
    let value = vm.eval_source(result).expect("result evaluation");
    vm.vs(&value).unwrap()
}

#[test]
fn requests_wait_for_await_and_preserve_order() {
    assert_eq!(
        run(
            r#"
        let release, log = [];
        let gate = new Promise(resolve => { release = resolve; });
        async function* f() { log.push('start'); yield await gate; yield 2; return 3; }
        let g = f();
        let a = g.next(), b = g.next(), c = g.next();
        if (!(a instanceof Promise)) throw new Error('next must return a promise');
        a.then(r => log.push('a:' + r.value + ':' + r.done));
        b.then(r => log.push('b:' + r.value + ':' + r.done));
        c.then(r => log.push('c:' + r.value + ':' + r.done));
        log.push('queued'); release(1);
    "#,
            "log.join(',')"
        ),
        "start,queued,a:1:false,b:2:false,c:3:true"
    );
}

#[test]
fn return_request_runs_yielding_cleanup_and_awaits_values() {
    assert_eq!(
        run(
            r#"
        let log = [];
        async function* f() { try { yield Promise.resolve(1); } finally { yield Promise.resolve(2); } }
        let g = f();
        g.next().then(r => log.push(r.value + ':' + r.done));
        g.return(Promise.resolve(42)).then(r => log.push(r.value + ':' + r.done));
        g.next().then(r => log.push(r.value + ':' + r.done));
    "#,
            "log.join(',')"
        ),
        "1:false,2:false,42:true"
    );
}

#[test]
fn rejected_return_expression_is_catchable_inside_body() {
    assert_eq!(
        run(
            r#"
        let reason = {}, result;
        async function* f() {
            try { return Promise.reject(reason); }
            catch(e) { yield e === reason; return 9; }
        }
        let g = f();
        g.next().then(r => { result = r.value; });
    "#,
            "result"
        ),
        "true"
    );
}

#[test]
fn ordinary_nested_function_return_is_not_awaited() {
    assert_eq!(
        run(
            r#"
        let promise = Promise.resolve(42), result;
        async function* f() { function ordinary() { return promise; } yield ordinary() === promise; }
        f().next().then(r => { result = r.value; });
    "#,
            "result"
        ),
        "true"
    );
}

#[test]
fn asynchronous_delegation_forwards_return_completion() {
    assert_eq!(
        run(
            r#"
        let log = [];
        async function* inner() { try { yield 1; } finally { yield 2; } }
        async function* outer() { return yield* inner(); }
        let g = outer();
        g.next().then(r => log.push(r.value + ':' + r.done));
        g.return(42).then(r => log.push(r.value + ':' + r.done));
        g.next().then(r => log.push(r.value + ':' + r.done));
    "#,
            "log.join(',')"
        ),
        "1:false,2:false,42:true"
    );
}

#[test]
fn return_before_start_awaits_without_executing_body() {
    assert_eq!(
        run(
            r#"
        let ran = false, log = [];
        async function* f() { ran = true; yield 1; }
        let g = f();
        g.return(Promise.resolve(42)).then(r => log.push(r.value + ':' + r.done));
        g.next().then(r => log.push(r.value + ':' + r.done));
    "#,
            "[ran, log.join(',')].join('|')"
        ),
        "false|42:true,undefined:true"
    );
}

#[test]
fn async_generators_are_async_iterable_and_reject_wrong_brands() {
    assert_eq!(
        run(
            r#"
        let g = (async function*(){})(), failures = 0;
        if (g[Symbol.asyncIterator]() !== g || g[Symbol.iterator] !== undefined) throw new Error('wrong iterator protocol');
        for (let method of ['next', 'return', 'throw']) {
            g[method].call({}).catch(e => { if (e instanceof TypeError) failures++; });
        }
        let sync = (function*(){})();
        try { sync.next.call(g); } catch(e) { if (e instanceof TypeError) failures++; }
    "#,
            "failures"
        ),
        "4"
    );
}

#[test]
fn borrowed_async_next_preserves_the_generators_module_realm() {
    use napi_vm::VirtualLoader;
    use std::rc::Rc;
    let mut vm = Interpreter::with_builtins();
    let parent_loader = Rc::new(VirtualLoader::new());
    parent_loader.insert("value", "export default 1;");
    vm.set_module_loader(parent_loader);
    let mut child = vm.create_realm();
    let child_loader = Rc::new(VirtualLoader::new());
    child_loader.insert("value", "export default 2;");
    child.set_module_loader(child_loader);
    let generator = child
        .eval_source("(async function*(){ yield (await import('value')).default; })();")
        .unwrap();
    vm.set_global_checked("foreign", generator).unwrap();
    drop(child);
    vm.eval_source("let observed; let next = (async function*(){})().next; next.call(foreign).then(r => { observed = r.value; });").unwrap();
    vm.poll_event_loop(TurnBudget::jobs(10_000)).unwrap();
    let result = vm.eval_source("observed;").unwrap();
    assert!(matches!(result, napi_vm::Value::Number(2.0)), "{result:?}");
}

#[test]
fn borrowed_async_next_allocates_its_promise_and_result_in_the_method_realm() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    let generator = child
        .eval_source("(async function*(){ yield 42; })();")
        .unwrap();
    vm.set_global_checked("foreign", generator).unwrap();
    vm.eval_source(r#"
        let owned = false;
        let next = (async function*(){})().next;
        let promise = next.call(foreign);
        if (Object.getPrototypeOf(promise) !== Promise.prototype) throw new Error('wrong promise realm');
        promise.then(r => { owned = Object.getPrototypeOf(r) === Object.prototype && r.value === 42; });
    "#).unwrap();
    vm.poll_event_loop(TurnBudget::jobs(10_000)).unwrap();
    let result = vm.eval_source("owned;").unwrap();
    assert!(matches!(result, napi_vm::Value::Bool(true)), "{result:?}");
}

#[test]
fn for_await_waits_for_generator_cleanup_before_leaving() {
    assert_eq!(
        run(
            r#"
        let log = [];
        async function* f() {
            try { yield 1; }
            finally { log.push('cleanup'); await 0; log.push('finished'); }
        }
        async function consume() {
            for await (let value of f()) { log.push('body'); break; }
            log.push('closed');
        }
        consume();
    "#,
            "log.join(',')"
        ),
        "body,cleanup,finished,closed"
    );
}
