use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(&result, Value::String(value) if value == expected),
        "prepared {source}: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&result, Value::String(value) if value == expected),
        "AST {source}: {result:?}"
    );
}

#[test]
fn initializers_have_a_different_binding_from_the_first_iteration() {
    both(
        "var before,test,body,incr; var run=true; for(let x='outside', _=before=()=>x; run && (x='inside',test=()=>x); incr=()=>x) { body=()=>x; run=false; } [before(),test(),body(),incr()].join(',')",
        "outside,inside,inside,inside",
    );
}

#[test]
fn all_lexical_head_names_exist_before_any_initializer() {
    both(
        "var errors=0; try { for(let x=y, y=1;;) {break;} }catch(e){if(e instanceof ReferenceError)errors++;} try{for(const x=typeof x;;){break;}}catch(e){if(e instanceof ReferenceError)errors++;} try{for(let [x=y,y]=[];;){break;}}catch(e){if(e instanceof ReferenceError)errors++;} try{for(let [x]=x;;){break;}}catch(e){if(e instanceof ReferenceError)errors++;} String(errors)",
        "4",
    );
}

#[test]
fn shorthand_reads_and_typeof_respect_tdz_and_missing_bindings() {
    both(
        "var errors=0;try{({x});let x;}catch(e){if(e instanceof ReferenceError)errors++;}try{var object={missingShorthand};}catch(e){if(e instanceof ReferenceError)errors++;}try{typeof undefined; let undefined;}catch(e){if(e instanceof ReferenceError)errors++;} [errors,typeof ((neverDeclared)),(function(){let undefined=3;return typeof undefined;})()].join(',')",
        "3,undefined,number",
    );
}

#[test]
fn in_and_of_head_environments_remain_in_tdz_for_escaped_closures() {
    both(
        "let x=1;var probeIn,probeOf;var errors=0;try{for(const x in {x}){}}catch(e){if(e instanceof ReferenceError)errors++;}for(let x in (probeIn=()=>typeof x,{})){}for(let x of (probeOf=()=>typeof x,[])){}for(var probe of [probeIn,probeOf]){try{probe();}catch(e){if(e instanceof ReferenceError)errors++;}} String(errors)",
        "3",
    );
}

#[test]
fn captured_const_heads_remain_immutable_and_let_heads_rotate() {
    both(
        "var probes=[],errors=0;for(const x=1;probes.length<2;){probes.push(()=>x);try{x=2;}catch(e){if(e instanceof TypeError)errors++;}}for(let y=0;y<3;y++){probes.push(()=>y);} [errors,...probes.map(f=>f())].join(',')",
        "2,1,1,0,1,2",
    );
}

#[test]
fn bare_var_heads_do_not_erase_existing_values() {
    both(
        "var x=7,y=8;for(var x;false;){}for(var [z]=[9],y;false;){}[x,y,z].join(',')",
        "7,8,9",
    );
}

#[test]
fn for_initializer_function_names_are_inferred() {
    both(
        "var result;for(let f=function(){},g=()=>{};!result;){result=f.name+','+g.name;}result",
        "f,g",
    );
}

#[test]
fn catch_parameter_initializers_do_not_see_body_lexicals() {
    both(
        "var probeParam,probeBlock;let x='outside';try{throw [];}catch([_=probeParam=()=>x]){probeBlock=()=>x;let x='inside';}[probeParam(),probeBlock()].join(',')",
        "outside,inside",
    );
}

#[test]
fn direct_eval_closures_retain_initializer_and_iteration_environments() {
    both(
        "var before,probes=[];for(let i=(before=eval('()=>i'),0);i<2;i++){probes.push(eval('()=>i'));}[before(),...probes.map(f=>f())].join(',')",
        "0,0,1",
    );
}

#[test]
fn tdz_errors_in_destructuring_close_an_unfinished_iterator() {
    both(
        "var closed=0,reason;var source={[Symbol.iterator](){return {next(){return {value:undefined,done:false};},return(){closed++;return {};}};}};try{for(let [x=y,y]=source;false;){}}catch(e){reason=e.name;}[reason,closed].join(',')",
        "ReferenceError,1",
    );
}

#[test]
fn ordinary_heads_and_local_typeof_use_verified_bytecode() {
    let source = "var errors=0;try{for(let x=y,y=1;false;){}}catch(e){if(e instanceof ReferenceError)errors++;}function local(){try{return typeof x;}catch(e){return e.name;}let x;}String(errors)+','+local()";
    let program = Interpreter::compile(source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&program).unwrap();
    assert!(matches!(&result,Value::String(value) if value=="1,ReferenceError"));
    assert!(
        matches!(vm.global_value("local"),Some(Value::Function(ref function)) if function.bytecode.is_some())
    );
    both(source, "1,ReferenceError");
}
