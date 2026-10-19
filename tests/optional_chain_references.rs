use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let result = Interpreter::with_builtins().execute(&program).unwrap();
    assert!(
        matches!(&result, Value::String(value) if value == expected),
        "bytecode: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&result, Value::String(value) if value == expected),
        "AST: {result:?}"
    );
}

#[test]
fn optional_private_fields_keep_brand_checks_and_short_circuit() {
    both(
        "class C{#field=7;static read(o){return o?.#field;}}var c=new C(),caught;try{C.read({'#field':9});}catch(e){caught=e.name;}[C.read(c),C.read(null),C.read(undefined),caught].join('|')",
        "7|||TypeError",
    );
}

#[test]
fn private_member_continuations_retain_short_circuit_state() {
    both(
        "class C{#field=8;read(o){return o?.nested.#field;}}var c=new C(),caught;try{c.read({nested:{}});}catch(e){caught=e.name;}[c.read({nested:c}),c.read(null),caught].join('|')",
        "8||TypeError",
    );
}

#[test]
fn optional_private_methods_preserve_receivers_and_skip_arguments() {
    both(
        "class C{value=9;#method(x){return this.value+x;}run(o){return o?.#method(2);}other(o){return o.#method?.(3);}}var c=new C();[c.run(c),c.other(c),c.run(null)].join('|')",
        "11|12|",
    );
}

#[test]
fn continuations_skip_keys_arguments_and_calls() {
    both(
        "var hits=0,o=null,f=null;var a=o?.x[(hits++,'key')](hits++).last;var b=f?.()(hits++).last;var errors=0;try{({x:undefined})?.x.y;}catch(e){if(e instanceof TypeError)errors++;}try{(()=>undefined)?.().x;}catch(e){if(e instanceof TypeError)errors++;}[a,b,hits,errors].join('|')",
        "||0|2",
    );
}

#[test]
fn parentheses_end_short_circuits_and_preserve_successful_receivers() {
    both(
        "var o=null,hits=0,errors=0;try{(o?.x).y;}catch(e){if(e instanceof TypeError)errors++;}try{(o?.x)(hits++);}catch(e){if(e instanceof TypeError)errors++;}var skipped=(o?.x)?.(hits++);var obj={value:12,method(){return this.value;}};[errors,hits,skipped,(obj?.method)()].join('|')",
        "2|1||12",
    );
}

#[test]
fn calls_can_continue_into_members_and_more_calls() {
    both(
        "var obj={value:4,make(){return {next:()=>this.value+1};}};var fn=()=>({value:6});[obj?.make().next(),fn?.().value,(null)?.make().next()].join('|')",
        "5|6|",
    );
}

#[test]
fn computed_undefined_keys_are_not_optional_call_markers() {
    both(
        "var o={undefined:7};[o?.[undefined],(null)?.[undefined]].join('|')",
        "7|",
    );
}

#[test]
fn deletion_propagates_short_circuit_without_reading_the_final_property() {
    both(
        "var hits=0,o=null;var a=delete o?.x[(hits++,'key')];var b=delete o?.make().x;var obj={nested:{get value(){hits++;return 1;}}};var c=delete obj?.nested.value;var errors=0;try{delete (o?.x).value;}catch(e){if(e instanceof TypeError)errors++;}[a,b,c,hits,errors,Object.hasOwn(obj.nested,'value')].join('|')",
        "true|true|true|0|1|false",
    );
}
