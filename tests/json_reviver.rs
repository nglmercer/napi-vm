use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_tiers(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "bytecode: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "AST: {result:?}"
    );
}

#[test]
fn reviver_walks_postorder_and_deletes_array_elements() {
    assert_tiers(
        r#"var log=[];var result=JSON.parse('{"a":[1,2],"b":3}',function(k,v,c){log.push([k,c.source]);if(k==='0')return undefined;if(k==='b')return 9;return v;});JSON.stringify([result,log,0 in result.a]);"#,
        r#"[{"a":[null,2],"b":9},[["0","1"],["1","2"],["a",null],["b","3"],["",null]],false]"#,
    );
}

#[test]
fn reviver_invalidates_source_for_modified_containers_and_values() {
    assert_tiers(
        r#"var sources=[];JSON.parse('{"a":1,"b":{"c":2},"d":3}',function(k,v,c){if(k==='a'){this.b={c:2};this.d=4;}sources.push([k,c.source]);return v;});JSON.stringify(sources);"#,
        r#"[["a","1"],["c",null],["b",null],["d",null],["",null]]"#,
    );
}

#[test]
fn reviver_ignores_failed_property_changes_and_keeps_last_duplicate_source() {
    assert_tiers(
        r#"var source;var value=JSON.parse('{"a":1,"b":2,"b":2.0}',function(k,v,c){if(k==='a')Object.defineProperty(this,'b',{value:7,configurable:false});if(k==='b')return undefined;return v;});JSON.parse('{"x":1,"x":1.0}',function(k,v,c){if(k==='x')source=c.source;return v;});JSON.stringify([value,source]);"#,
        r#"[{"a":1,"b":7},"1.0"]"#,
    );
}

#[test]
fn reviver_source_preserves_utf16_and_numeric_spelling() {
    assert_tiers(
        r#"var sources=[];JSON.parse('["'+String.fromCharCode(0xd800)+'",1.00e+2,-0]',function(k,v,c){if(k!=='')sources.push(c.source);return v;});JSON.stringify([sources[0].charCodeAt(1),sources[1],sources[2]]);"#,
        r#"[55296,"1.00e+2","-0"]"#,
    );
}

#[test]
fn raw_json_is_frozen_branded_and_runs_after_replacer() {
    assert_tiers(
        r#"var raw=JSON.rawJSON('9007199254740993');var proxy=new Proxy(raw,{});JSON.stringify([Object.getPrototypeOf(raw)===null,Object.isFrozen(raw),Object.keys(raw),JSON.isRawJSON(raw),JSON.isRawJSON(proxy),JSON.isRawJSON({rawJSON:'1'}),JSON.stringify({n:1},function(k,v){return k==='n'?raw:v}),JSON.stringify({n:raw},null,2)]);"#,
        r#"[true,true,["rawJSON"],true,false,false,"{\"n\":9007199254740993}","{\n  \"n\": 9007199254740993\n}"]"#,
    );
}

#[test]
fn raw_json_validates_grammar_and_preserves_source_code_units() {
    assert_tiers(
        r#"var bad=0;for(var text of ['', ' 1', '1 ', '{}', '[]', '01', 'undefined']){try{JSON.rawJSON(text)}catch(e){if(e instanceof SyntaxError)bad++}}var text='"'+String.fromCharCode(0xd800)+'"';var raw=JSON.rawJSON(text);JSON.stringify([bad,JSON.stringify(raw)===text,JSON.stringify({a:raw},null,2).indexOf(text)>=0,JSON.stringify(JSON.rawJSON('1e+400'))]);"#,
        r#"[7,true,true,"1e+400"]"#,
    );
}
