use napi_vm::{Interpreter, Lexer, Parser, Value};

fn check(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&prepared).unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "prepared: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let mut ast = Interpreter::with_builtins();
    let result = ast.run_program_body(&statements).unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "AST: {result:?}"
    );
}

#[test]
fn formats_relative_words_signed_zero_and_rounded_numeric_parts() {
    check(
        r#"
        var r = new Intl.RelativeTimeFormat('en-US', {numeric:'auto'});
        var a = new Intl.RelativeTimeFormat('en-US');
        JSON.stringify([r.format(-1,'day'), r.format(0,'seconds'), r.format(0.0001,'second'),
          a.format(-0,'second'), a.format(1.2345,'second'), a.formatToParts(1000,'seconds'),
          r.formatToParts(0,'second')]);
    "#,
        r#"["yesterday","now","in 0 seconds","0 seconds ago","in 1.235 seconds",[{"type":"literal","value":"in "},{"type":"integer","value":"1","unit":"second"},{"type":"group","value":",","unit":"second"},{"type":"integer","value":"000","unit":"second"},{"type":"literal","value":" seconds"}],[{"type":"literal","value":"now"}]]"#,
    );
}

#[test]
fn locale_numbering_resolution_uses_supported_options_and_extensions() {
    check(
        r#"
        var a = new Intl.RelativeTimeFormat('en-u-nu-arab', {numberingSystem:'invalid'});
        var b = new Intl.RelativeTimeFormat('en-u-nu-latn', {numberingSystem:'arab'});
        var c = new Intl.RelativeTimeFormat('pl-PL');
        JSON.stringify([a.resolvedOptions(), b.resolvedOptions(), a.format(2,'day'), c.format(2,'day'), new Intl.RelativeTimeFormat('en-u-nu').resolvedOptions().numberingSystem]);
    "#,
        r#"[{"locale":"en-u-nu-arab","style":"long","numeric":"always","numberingSystem":"arab"},{"locale":"en","style":"long","numeric":"always","numberingSystem":"arab"},"in ٢ days","za 2 dni","latn"]"#,
    );
}

#[test]
fn coercions_option_order_and_branding_remain_observable() {
    check(
        r#"
        var events = [];
        var options = new Proxy({}, {get(t,k){events.push(k);return undefined;}});
        var r = new Intl.RelativeTimeFormat('en', options);
        var text = r.format({valueOf(){events.push('number');return 2;}}, {toString(){events.push('unit');return 'days';}});
        var branded = false;
        try { r.format.call(new Proxy(r,{}), {valueOf(){throw 1;}}, 'day'); } catch(e){branded = e instanceof TypeError;}
        JSON.stringify([events, text, branded]);
    "#,
        r#"[["localeMatcher","numberingSystem","style","numeric","number","unit"],"in 2 days",true]"#,
    );
}
