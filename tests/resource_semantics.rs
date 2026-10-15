use napi_vm::{Interpreter, TurnBudget, Value};

fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    vm.poll_event_loop(TurnBudget::jobs(10_000)).unwrap();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "{result:?}"
    );
}

#[test]
fn lexical_disposal_preserves_return_and_suppresses_nested_throws() {
    check(
        r#"var log=[],result;function resource(n){return {[Symbol.dispose](){log.push(n);throw n}}}function f(){using a=resource('a');{using b=resource('b');throw 'body'}}try{f()}catch(e){result=[log,e.error,e.suppressed.error,e.suppressed.suppressed]}"#,
        r#"[["b","a"],"a","b","body"]"#,
    );
    check(
        r#"var log=[],result;function f(){using a={[Symbol.dispose](){log.push('disposed')}};return 7}result=[f(),log];"#,
        r#"[7,["disposed"]]"#,
    );
}

#[test]
fn loop_disposal_precedes_iterator_close_and_catch_disposes_its_scope() {
    check(
        r#"var log=[],result;function* g(){try{yield {[Symbol.dispose](){log.push('dispose')}}}finally{log.push('close')}}for(using x of g()){break}try{throw 1}catch(e){using x={[Symbol.dispose](){log.push('catch')}}}result=log;"#,
        r#"["dispose","close","catch"]"#,
    );
}

#[test]
fn classic_resource_loop_disposes_once_and_async_scope_waits() {
    check(
        r#"var result,log=[];async function f(){for(using x={[Symbol.dispose](){log.push('loop')}};log.length<2;){log.push('body')} {await using x={[Symbol.asyncDispose](){log.push('start');return Promise.resolve().then(function(){log.push('end')})}};}log.push('after')}f().then(function(){result=log});log.push('returned');"#,
        r#"["body","body","loop","start","returned","end","after"]"#,
    );
}
