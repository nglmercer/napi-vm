use napi_vm_core::{Interpreter, Lexer, Parser, Value};
fn both(source: &str) {
    for ast in [false, true] {
        let mut vm = Interpreter::with_builtins();
        let result = if ast {
            let mut parser = Parser::new(Lexer::new(source).tokenize());
            let stmts = parser.parse();
            vm.run_program_body(&stmts)
                .and_then(|v| vm.drain_jobs().map(|_| v))
        } else {
            let prepared = Interpreter::compile(source).unwrap();
            assert_eq!(
                prepared.tier(),
                napi_vm_core::interpreter::ExecutionTier::Bytecode
            );
            vm.execute(&prepared)
        };
        assert!(
            matches!(result, Ok(Value::Bool(true))),
            "AST={ast}: {source}: {result:?}"
        );
    }
}
#[test]
fn await_assimilates_thenables_and_preserves_rejection_identity() {
    both(
        "var order='';var thenable={then(resolve){order+='T';resolve(42);}};var result=await thenable;result===42 && order==='T'",
    );
    both(
        "var reason={tag:42};var same=false;try{await {get then(){throw reason;}};}catch(e){same=e===reason;}same",
    );
    both(
        "var result;async function f(){try{return await Promise.reject(42);}catch(e){return e+1;}}result=await f();result===43",
    );
}
#[test]
fn async_and_generator_fallthrough_return_undefined() {
    both("async function f(){await 0;42;}var value=await f();value===undefined");
    both("function* f(){yield 1;42;}var gen=f();gen.next();gen.next().value===undefined");
}
#[test]
fn async_bytecode_preserves_finally_and_resume_order() {
    both(
        "var order='';async function f(){try{order+='A';await 0;order+='C';return 7;}finally{order+='F';}}var promise=f();order+='B';var value=await promise;value===7 && order==='ABCF'",
    );
    both(
        "var order='';async function f(){await {then(resolve){order+='T';resolve();}};order+='A';}var p=f();order+='S';await p;order==='STA'",
    );
}
#[test]
fn emitted_await_instruction_is_verified() {
    let code = Interpreter::compile("await Promise.resolve(42)").unwrap();
    assert_eq!(
        code.tier(),
        napi_vm_core::interpreter::ExecutionTier::Bytecode
    );
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.execute(&code), Ok(Value::Number(42.))));
}
