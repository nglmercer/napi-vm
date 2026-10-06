use napi_vm::{Interpreter, Value};
fn check(source: &str) {
    for ast in [false, true] {
        let mut vm = Interpreter::with_builtins();
        let result = if ast {
            let mut parser = napi_vm::Parser::new(napi_vm::Lexer::new(source).tokenize());
            let statements = parser.parse();
            vm.run(&statements)
        } else {
            let prepared = Interpreter::compile(source).unwrap();
            assert_eq!(
                prepared.tier(),
                napi_vm::interpreter::ExecutionTier::Bytecode,
                "regression must exercise bytecode: {source}"
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
fn code_units_and_surrogates() {
    check(
        r#"'😀'.length===2 && '😀'[0]==='\uD83D' && '😀'[1]==='\uDE00' && '\uD800'!=='\uFFFD' && '\uD800'.charCodeAt(0)===55296 && String.fromCharCode(0xD800)==='\uD800' && String.fromCodePoint(0x1F600)==='😀'"#,
    );
}
#[test]
fn slicing_search_padding() {
    check(
        r#"'😀x'.slice(1,2)==='\uDE00' && '😀x'.substring(2,1)==='\uDE00' && '😀x'.indexOf('x')===2 && '😀😀'.lastIndexOf('😀')===2 && '😀'.split('').length===2 && '😀'.at(-1)==='\uDE00' && 'x'.padStart(2,'😀')==='\uD83Dx' && '😀'.codePointAt(0)===128512 && '😀'.codePointAt(1)===56832"#,
    );
}
#[test]
fn coercion_and_iteration() {
    check(
        r#"String('\uD800')==='\uD800' && '\uD800'+'\uDC00'==='𐀀' && ['\uD800',null,'\uDC00'].join('')==='𐀀' && `${'\uD800'}`==='\uD800' && `\uD800`==='\uD800' && [...'😀\uD800'].length===2 && [...'😀\uD800'][1]==='\uD800' && '\uD800'.replaceAll('','x')==='x\uD800x'"#,
    );
}
#[test]
fn property_keys_and_json() {
    check(
        r#"let o={'\uD800':1,'\uFFFD':2,'\uFDD0sD800':3};o['\uD800']===1 && o['\uFFFD']===2 && o['\uFDD0sD800']===3 && Object.keys(o)[0]==='\uD800' && Reflect.ownKeys(o)[0]==='\uD800' && JSON.parse(JSON.stringify(o))['\uD800']===1 && JSON.parse('"\\ud800"')==='\uD800' && JSON.stringify('\uD800')==='"\\ud800"'"#,
    );
}
#[test]
fn regex_offsets_and_unicode() {
    check(
        r#"/x/.exec('😀x').index===2 && /./.exec('😀')[0]==='\uD83D' && /./u.exec('😀')[0]==='😀' && /\uD800/.test('\uD800') && '😀'.match(/./g).length===2 && '😀'.match(/./gu).length===1 && '😀x'.replace(/x/,'\uD800')==='😀\uD800'"#,
    );
}
#[test]
fn literal_validation() {
    for s in [r#"'\uXXXX'"#, r#"'\xG0'"#, r#"'\u{110000}'"#] {
        assert!(Interpreter::with_builtins().eval_source(s).is_err(), "{s}");
    }
}

#[test]
fn generated_source_and_descriptions() {
    check(
        r#"let s='\uD800'; eval("'"+s+"'")===s && Function("return '"+s+"'")()===s && Symbol(s).description===s && Symbol.for(s)===Symbol.for(s) && Symbol.keyFor(Symbol.for(s))===s && new Error(s).message===s && String(new Error(s))==='Error: '+s"#,
    );
}

#[test]
fn unicode_regex_atoms_and_string_patterns() {
    check(
        r#"/^\uD83D\uDE00+$/u.test('😀😀') && /^[\uD83D\uDE00]$/u.test('😀') && '\uD800'.match('\uD800')[0]==='\uD800' && '😀\uD800'.search('\uD800')===2 && 'abc'.match('a.c')[0]==='abc' && !/\u{D83D}\u{DE00}/u.test('😀') && !new RegExp('\uD83D'+'\\uDE00','u').test('😀')"#,
    );
}

#[test]
fn host_conversions_and_serialization_are_explicit() {
    use napi_vm::JsString;
    for units in [
        vec![],
        vec![0],
        vec![0xd800],
        vec![0xdc00, 0xd800],
        vec![0xd83d, 0xde00],
        vec![0xfdd0, 0x73, 0x44, 0x38, 0x30, 0x30],
    ] {
        let text = JsString::from_units(units);
        assert_eq!(JsString::from_key(&text.to_key()), text);
        let json = serde_json::to_string(&text).unwrap();
        assert_eq!(serde_json::from_str::<JsString>(&json).unwrap(), text);
    }
    let lone = JsString::from_units(vec![0xd800]);
    assert!(lone.to_utf8().is_err());
    assert_eq!(lone.as_str(), "\u{fffd}");
    assert!(
        napi_vm::convert::value_to_json(&mut Interpreter::with_builtins(), &Value::String(lone))
            .is_err()
    );
}

#[test]
fn assigned_keys_across_receivers() {
    check(
        r#"let k='\uD800';let o={};let a=[];function f(){}globalThis[k]=1;o[k]=2;a[k]=3;f[k]=4;o[k]++;Object.defineProperty(o,k,{writable:false});o[k]=8;globalThis[k]===1 && o[k]===3 && a[k]===3 && f[k]===4 && Object.keys(o)[0]===k && delete a[k] && a[k]===undefined"#,
    );
}

#[test]
fn host_json_property_keys_do_not_alias_guest_slots() {
    let mut vm = Interpreter::with_builtins();
    let marker = napi_vm::JsString::from("\u{fdd0}sD800");
    let value = Value::object(vec![(marker.to_key(), Value::Number(1.0))]);
    let output = napi_vm::convert::value_to_json(&mut vm, &value).unwrap();
    assert_eq!(output.get(marker.as_str()), Some(&serde_json::json!(1)));
    let lone = napi_vm::JsString::from_units(vec![0xd800]);
    let value = Value::object(vec![(lone.to_key(), Value::Number(1.0))]);
    assert!(napi_vm::convert::value_to_json(&mut vm, &value).is_err());
}

#[test]
fn proxy_traps_receive_decoded_keys() {
    check(
        r#"let k='\uD800';let seen='';let p=new Proxy({}, {get(t,key){return key;},set(t,key,v){seen=key;return true;},has(t,key){return key===k;},ownKeys(){return [k];}});p[k]=1;p[k]===k && seen===k && k in p && Reflect.ownKeys(p)[0]===k"#,
    );
}

#[test]
fn spread_and_rest_preserve_keys() {
    check(
        r#"let k='\uD800';let o={a:0};o[k]=7;let copy={...o};let {a,...rest}=o;copy[k]===7 && rest[k]===7 && Object.keys(rest)[0]===k"#,
    );
}

#[test]
fn inline_caches_do_not_alias_surrogate_keys() {
    check(
        r#"let o={'\uD800':1,'\uFFFD':2,'\uFDD0sD800':3};function read(k){return o[k];}function write(k,v){o[k]=v;}let sum=0;for(let i=0;i<40;i++){sum+=read('\uD800');sum+=read('\uFFFD');sum+=read('\uFDD0sD800');write('\uD800',1);}sum===240 && o['\uD800']===1 && o['\uFFFD']===2 && o['\uFDD0sD800']===3"#,
    );
}

#[test]
fn unicode_regex_last_index_inside_pair() {
    check(
        r#"let r=/./uy;r.lastIndex=1;let m=r.exec('😀');m.index===0 && m[0]==='😀' && r.lastIndex===2"#,
    );
}

#[test]
fn ordering_uses_utf16_units() {
    check(
        r#"'😀'<'\uFFFF' && '\uD800'<'\uDC00' && ['\uFFFF','😀','\uD800','\uDC00'].sort().join('|')==='\uD800|😀|\uDC00|\uFFFF'"#,
    );
}

#[test]
fn raw_utf16_script_sources() {
    let mut units: Vec<u16> = "'".encode_utf16().collect();
    units.push(0xd800);
    units.extend("'.charCodeAt(0)".encode_utf16());
    let source = napi_vm::JsString::from_units(units);
    let program = Interpreter::compile_utf16(&source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.execute(&program), Ok(Value::Number(value)) if value == 55296.0));
    assert!(matches!(vm.eval_utf16(&source), Ok(Value::Number(value)) if value == 55296.0));
}
