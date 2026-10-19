use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let value = Interpreter::with_builtins().execute(&program).unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "bytecode: {value:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let value = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "AST: {value:?}"
    );
}

#[test]
fn constructor_properties_precede_user_string_members() {
    both(
        "class C{a(){} [2](){} b(){} static a(){} static [2](){} static b(){}}[Object.getOwnPropertyNames(C.prototype).join(','),Object.getOwnPropertyNames(C).join(','),C.prototype.constructor===C].join('|')",
        "2,constructor,a,b|2,length,name,prototype,a,b|true",
    );
}

#[test]
fn computed_constructor_override_keeps_the_original_property_position() {
    both(
        "class C{a(){} ['constructor'](){return 9;}b(){}}[Object.getOwnPropertyNames(C.prototype).join(','),new C().constructor()].join('|')",
        "constructor,a,b|9",
    );
}

#[test]
fn static_name_and_length_methods_override_values_and_attributes_in_place() {
    both(
        "class C{static a(){} static length(){return 2;}static name(){return 3;}static b(){}}var d=Object.getOwnPropertyDescriptor(C,'length');var n=Object.getOwnPropertyDescriptor(C,'name');[Object.getOwnPropertyNames(C).join(','),C.length(),C.name(),d.writable,n.writable].join('|')",
        "length,name,prototype,a,b|2|3|true|true",
    );
}

#[test]
fn invalid_static_prototype_methods_stop_later_computed_keys() {
    both(
        "var log=[],reason;var key={};key[Symbol.toPrimitive]=function(){log.push('key');return 'prototype';};try{class C{static [key](){}[(log.push('later'),'x')](){}}}catch(e){reason=e.name;}log.join(',')+','+reason",
        "key,TypeError",
    );
}

#[test]
fn prototype_accessors_and_generator_methods_are_rejected() {
    both(
        "var errors=0;try{class C{static get ['prototype'](){}}}catch(e){if(e instanceof TypeError)errors++;}try{class C{static set ['prototype'](v){}}}catch(e){if(e instanceof TypeError)errors++;}try{class C{static *['prototype'](){}}}catch(e){if(e instanceof TypeError)errors++;}try{class C{static async ['prototype'](){}}}catch(e){if(e instanceof TypeError)errors++;}String(errors)",
        "4",
    );
}

#[test]
fn prototype_static_fields_fail_after_initializers_and_symbols_are_allowed() {
    both(
        "var log=[],reason;try{class C{static ['prototype']=(log.push('init'),1);}}catch(e){reason=e.name;}var key=Symbol('prototype');class D{static [key](){return 7;}}[log.join(','),reason,D[key]() ].join('|')",
        "init|TypeError|7",
    );
}

#[test]
fn private_static_prototype_method_is_allowed() {
    both(
        "class C{static #prototype(){return 8;}static call(){return this.#prototype();}}String(C.call())",
        "8",
    );
}
