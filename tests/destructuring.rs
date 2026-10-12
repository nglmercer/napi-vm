use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "prepared: {result:?} for {source}"
    );
    let mut parser = Parser::new(Lexer::new(source).tokenize());
    let statements = parser.parse_program().unwrap();
    let mut ast = Interpreter::with_builtins();
    let result = ast.run_program_body(&statements).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "AST: {result:?} for {source}"
    );
}

#[test]
fn array_assignment_captures_next_once_and_reads_done_before_value() {
    assert_both(
        "var log=[];var it={[Symbol.iterator](){log.push('iter');var n=0;return {get next(){log.push('getNext');return function(){log.push('step');return {get done(){log.push('done');return n>=2},get value(){log.push('value');return ++n}}}}}}};var a,b;[a,b]=it;JSON.stringify([a,b,log.join(',')]);",
        "[1,2,\"iter,getNext,step,done,value,step,done,value\"]",
    );
}

#[test]
fn elision_steps_without_reading_value_and_early_finish_closes() {
    assert_both(
        "var log=[];var it={[Symbol.iterator](){var n=0;return {next(){return {get done(){return false},get value(){log.push('v'+(++n));return n}}},return(){log.push('close');return {}}}}};var a;[,a]=it;JSON.stringify([a,log.join(',')]);",
        "[1,\"v1,close\"]",
    );
}

#[test]
fn abrupt_target_closes_iterator_preserving_error() {
    assert_both(
        "var log=[];var it={[Symbol.iterator](){return {next(){return {done:false,value:1}},return(){log.push('close');return {}}}}};var o={};Object.defineProperty(o,'x',{set(v){throw new Error('boom')}});try{({x:o.x}={x:1})}catch(e){log.push(e.message)}var a;try{[o.x]=it}catch(e){log.push(e.message)}JSON.stringify(log.join(','));",
        "\"boom,close,boom\"",
    );
}

#[test]
fn rest_consumes_remaining_and_object_target_prepared_before_get() {
    assert_both(
        "var log=[];var src={get x(){log.push('get');return 9}};var o={};var key={toString(){log.push('key');return 'x'}};({[key]:o.y}=src);var r;[,...r]=[1,2,3];JSON.stringify([o.y,r.join(','),log.join(',')]);",
        "[9,\"2,3\",\"key,get\"]",
    );
}

#[test]
fn defaults_run_only_for_undefined_and_infer_names() {
    assert_both(
        "var log=[];function d(v){log.push('d');return v}var {a=d(1),b=d(2)}={a:undefined,b:null};var [c=function(){}]=[];var {e=()=>3}={};JSON.stringify([a,b,c.name,e.name,log.join(',')]);",
        "[1,null,\"c\",\"e\",\"d\"]",
    );
}

#[test]
fn array_declaration_uses_iterator_and_closes() {
    assert_both(
        "var log=[];var it={[Symbol.iterator](){var n=0;return {next(){return n<5?{done:false,value:++n}:{done:true}},return(){log.push('close');return {}}}}};var [a,,b]=it;JSON.stringify([a,b,log.join(',')]);",
        "[1,3,\"close\"]",
    );
}

#[test]
fn step_abrupt_skips_close_and_close_errors_propagate() {
    // next() throwing is an iteration abrupt completion: no IteratorClose.
    assert_both(
        "var rc=0;var it={[Symbol.iterator](){return {next(){throw new Error('step')},return(){rc++;return {}}}}};var a;var err='';try{[a]=it}catch(e){err=e.message}JSON.stringify([err,rc]);",
        "[\"step\",0]",
    );
    // Early finish: a throwing return() replaces the normal completion.
    assert_both(
        "var it={[Symbol.iterator](){var n=0;return {next(){return {done:false,value:++n}},return(){throw new Error('close')}}}};var a;var err='';try{[a]=it}catch(e){err=e.message}JSON.stringify(err);",
        "\"close\"",
    );
    // A non-object return() result is a TypeError on normal completion.
    assert_both(
        "var it={[Symbol.iterator](){return {next(){return {done:false,value:1}},return(){return null}}}};var a;var err='';try{[a]=it}catch(e){err=e.name}JSON.stringify(err);",
        "\"TypeError\"",
    );
    // Rest exhausts the iterator, so no close runs; a declaration that
    // finishes early does close, and its close error propagates.
    assert_both(
        "var rc=0;var it={[Symbol.iterator](){var n=0;return {next(){return n<2?{done:false,value:++n}:{done:true}},return(){rc++;return {}}}}};var [a,...r]=it;JSON.stringify([r.join(','),rc]);",
        "[\"2\",0]",
    );
    assert_both(
        "var it={[Symbol.iterator](){return {next(){return {done:false,value:1}},return(){throw new Error('declclose')}}}};var err='';try{var [a]=it}catch(e){err=e.message}JSON.stringify(err);",
        "\"declclose\"",
    );
}

#[test]
fn nested_and_loop_patterns_share_iterator_semantics() {
    assert_both(
        "var out=[];for(var [a,{b}]=[1,{b:2}];;){out.push(a+b);break}var {p:[q=5]}={p:[]};JSON.stringify([out.join(','),q]);",
        "[\"3\",5]",
    );
}

#[test]
fn iterator_acquisition_and_step_failures_skip_close() {
    for pattern in ["[a]", "[,a]", "[...a]", "var [a]", "var [...a]"] {
        for (body, expected) in [
            ("get next(){throw new Error('next')}", "next"),
            ("next(){throw new Error('step')}", "step"),
            ("next(){return null}", "TypeError"),
            (
                "next(){return {get done(){throw new Error('done')}}}",
                "done",
            ),
        ] {
            let source = format!(
                "var a,rc=0,err='';var it={{[Symbol.iterator](){{return {{{body},return(){{rc++;return {{}}}}}}}}}};try{{{pattern}=it}}catch(e){{err=e.name==='TypeError'?e.name:e.message}}JSON.stringify([err,rc]);"
            );
            assert_both(&source, &format!("[\"{expected}\",0]"));
        }
    }
    for pattern in ["[a]", "[...a]", "var [a]", "var [...a]"] {
        let source = format!(
            "var a,rc=0,err='';var it={{[Symbol.iterator](){{return {{next(){{return {{done:false,get value(){{throw new Error('value')}}}}}},return(){{rc++;return {{}}}}}}}}}};try{{{pattern}=it}}catch(e){{err=e.message}}JSON.stringify([err,rc]);"
        );
        assert_both(&source, "[\"value\",0]");
    }
}

#[test]
fn default_failure_closes_and_preserves_original_error() {
    for pattern in ["[a=fail()]", "var [a=fail()]"] {
        let source = format!(
            "var a,rc=0,err='';function fail(){{throw new Error('default')}}var it={{[Symbol.iterator](){{return {{next(){{return {{done:false,value:undefined}}}},return(){{rc++;throw new Error('close')}}}}}}}};try{{{pattern}=it}}catch(e){{err=e.message}}JSON.stringify([err,rc]);"
        );
        assert_both(&source, "[\"default\",1]");
    }
}

#[test]
fn falsy_values_do_not_trigger_defaults() {
    assert_both(
        "var c=0;function counter(){c++;return 99}var {a=counter(),b=counter(),d=counter(),e=counter(),f=counter()}={a:null,b:0,d:false,e:''};JSON.stringify([a,b,d,e,f,c]);",
        "[null,0,false,\"\",99,1]",
    );
}
