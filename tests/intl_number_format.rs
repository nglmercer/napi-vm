use napi_vm::{Interpreter, Lexer, Parser, Value};
fn check(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&program).unwrap();
    assert!(
        matches!(&result,Value::String(s)if s==expected),
        "prepared: {result:?}"
    );
    let body = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.run_program_body(&body).unwrap();
    assert!(
        matches!(&result,Value::String(s)if s==expected),
        "AST: {result:?}"
    );
}
#[test]
fn decimal_rounding_localized_patterns_and_notation_share_both_tiers() {
    check(
        r#"
      var a=new Intl.NumberFormat('en-US',{roundingMode:'halfEven',maximumFractionDigits:2});
      var c=new Intl.NumberFormat('de-DE',{style:'currency',currency:'EUR'});
      var u=new Intl.NumberFormat('en',{style:'unit',unit:'kilometer-per-hour',unitDisplay:'long'});
      var e=new Intl.NumberFormat('en',{notation:'engineering'}),k=new Intl.NumberFormat('en',{notation:'compact'});
      JSON.stringify([a.format('1.245'),a.format('1.255'),c.format(1234.5),u.format(2),e.format(12345),k.format(12345),(12345n).toLocaleString('en')]);
    "#,
        r#"["1.24","1.26","1.234,50 €","2 kilometers per hour","12.345E3","12K","12,345"]"#,
    );
}
#[test]
fn precision_and_parts_preserve_strings_bigints_and_signed_zero() {
    check(
        r#"
      var n=new Intl.NumberFormat('en',{maximumFractionDigits:20}),z=new Intl.NumberFormat('en',{signDisplay:'negative'});
      JSON.stringify([n.format('1.0000000000000001'),n.format(987654321987654321n),z.format(-0),n.format(-0),n.formatToParts(1234.5)]);
    "#,
        r#"["1.0000000000000001","987,654,321,987,654,321","0","-0",[{"type":"integer","value":"1"},{"type":"group","value":","},{"type":"integer","value":"234"},{"type":"decimal","value":"."},{"type":"fraction","value":"5"}]]"#,
    );
}
#[test]
fn legacy_wrappers_and_range_parts_remain_observable() {
    check(
        r#"
      var base=new Intl.NumberFormat('en',{style:'currency',currency:'USD',maximumFractionDigits:0});
      var wrapper=Object.create(Intl.NumberFormat.prototype);Intl.NumberFormat.call(wrapper,'en');
      JSON.stringify([base.format===base.format,Object.getOwnPropertyNames(base.format),wrapper.format(1234),Object.getOwnPropertySymbols(wrapper)[0].description,base.formatRange(3,5),base.formatRangeToParts(1,1)]);
    "#,
        r#"[true,["length","name"],"1,234","IntlLegacyConstructedSymbol","$3 – $5",[{"type":"approximatelySign","value":"~","source":"shared"},{"type":"currency","value":"$","source":"shared"},{"type":"integer","value":"1","source":"shared"}]]"#,
    );
}
#[test]
fn extracted_bound_format_survives_collection() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var n=new Intl.NumberFormat('en'); var f=n.format;n=null;")
        .unwrap();
    vm.collect_cycles();
    let result = vm.eval_source("f(1234)").unwrap();
    assert!(
        matches!(&result,Value::String(s)if s=="1,234"),
        "{result:?}"
    );
}

#[test]
fn exponent_inputs_compact_quotes_and_numbering_systems_are_lossless() {
    check(
        r#"
      var n=new Intl.NumberFormat('en'),c=new Intl.NumberFormat('de',{notation:'compact'}),t=new Intl.NumberFormat('zh-TW',{notation:'compact'});
      JSON.stringify([n.format(1.23e-30),n.format(-1.23e-30),n.format('1.234567890123456789e2'),c.format(987654321),t.format(987654321),new Intl.NumberFormat('en',{numberingSystem:'ahom'}).format(12),new Intl.NumberFormat('it',{notation:'compact',compactDisplay:'long'}).format(1000)]);
    "#,
        r#"["0","-0","123.457","988 Mio.","9.9億","𑜱𑜲","mille"]"#,
    );
}
