use napi_vm::{Interpreter, Value};
fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "{result:?}"
    );
}
#[test]
fn casing_observes_language_context_and_preserves_unpaired_surrogates() {
    check(
        r#"var result=['ΟΣ'.toLowerCase(),'Iİ'.toLocaleLowerCase('tr'),'iı'.toLocaleUpperCase('az'),'\ud800ΟΣ\udc00'.toLowerCase()];"#,
        r#"["ος","ıi","İI","\ud800ος\udc00"]"#,
    );
}
#[test]
fn normalization_forms_preserve_surrogates_and_observe_conversion_order() {
    check(
        r#"var log=[],receiver={toString(){log.push('receiver');return '\ud800e\u0301\udc00'}},form={toString(){log.push('form');return 'NFC'}};var normalized=String.prototype.normalize.call(receiver,form);var error;try{'a'.normalize('bad')}catch(e){error=e.name}var result=[normalized,'\ufb00'.normalize('NFKD'),log,error];"#,
        r#"["\ud800é\udc00","ff",["receiver","form"],"RangeError"]"#,
    );
}
#[test]
fn codec_errors_preserve_only_complete_decoded_prefixes() {
    check(
        r#"var a=new Uint8Array(4),error;try{a.setFromBase64('AQIDZ!',{lastChunkHandling:'strict'})}catch(e){error=e.name}var b=new Uint8Array(1);var count=b.setFromBase64('Zg==');var result=[Array.from(a),error,count,b.toHex(),Uint8Array.fromBase64('-_8=',{alphabet:'base64url'}).toBase64({alphabet:'base64url',omitPadding:true})];"#,
        r#"[[1,2,3,0],"SyntaxError",{"read":4,"written":1},"66","-_8"]"#,
    );
}
