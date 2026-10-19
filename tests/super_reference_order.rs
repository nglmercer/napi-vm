use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str) {
    let program = Interpreter::compile(source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    assert!(
        matches!(
            Interpreter::with_builtins().execute(&program),
            Ok(Value::Bool(true))
        ),
        "bytecode: {source}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    assert!(
        matches!(
            Interpreter::with_builtins().run_program_body(&statements),
            Ok(Value::Bool(true))
        ),
        "AST: {source}"
    );
}

#[test]
fn super_references_check_this_before_evaluating_the_key() {
    for expression in [
        "super[(hits++,'x')]",
        "super[(hits++,'x')]()",
        "super[(hits++,'x')]=1",
        "super[(hits++,'x')]+=1",
        "super[(hits++,'x')]++",
        "super[(hits++,'x')] ||= 1",
        "[super[(hits++,'x')]]=[1]",
        "({x:super[(hits++,'x')]}={x:1})",
        "super[super()]",
    ] {
        both(&format!(
            "var hits=0,caught;class B extends Object{{constructor(){{{expression};}}}}try{{new B();}}catch(e){{caught=e.name;}}hits===0&&caught==='ReferenceError';"
        ));
    }
}

#[test]
fn initialized_super_assignment_preserves_key_conversion_after_rhs() {
    both(
        "var log=[];var key={toString(){log.push('convert');return 'x';}};class A{set x(v){log.push('set:'+v);}}class B extends A{constructor(){super();super[(log.push('key'),key)]=(log.push('rhs'),9);}}new B();log.join(',')==='key,rhs,convert,set:9';",
    );
}

#[test]
fn arrow_super_access_uses_the_owning_constructors_this_binding() {
    both(
        "var hits=0,caught;class B extends Object{constructor(){(()=>super[(hits++,'x')])();}}try{new B();}catch(e){caught=e.name;}hits===0&&caught==='ReferenceError';",
    );
}

#[test]
fn ordinary_property_writes_do_not_require_a_derived_this_binding() {
    both(
        "var box={x:1};class B extends Object{constructor(){box.x++;box.x=3;super();}}new B();box.x===3;",
    );
}
