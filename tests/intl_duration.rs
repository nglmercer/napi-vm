use napi_vm::{Interpreter, Lexer, Parser, Value};
fn check(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&program).unwrap();
    assert!(
        matches!(&result,Value::String(s) if s==expected),
        "prepared: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.run_program_body(&statements).unwrap();
    assert!(
        matches!(&result,Value::String(s) if s==expected),
        "AST: {result:?}"
    );
}
#[test]
fn digital_units_and_parts_preserve_exact_subseconds() {
    check(
        r#"
      var d=new Intl.DurationFormat('en',{style:'digital'}),l=new Intl.DurationFormat('en',{style:'long'});
      JSON.stringify([d.format({hours:1,seconds:2}),
        d.format({seconds:1,milliseconds:2,microseconds:3,nanoseconds:9007199254740991}),
        d.format({milliseconds:4503599627370497000,microseconds:4503599627370495000000}),
        l.formatToParts({days:1,hours:2})]);
    "#,
        r#"["1:00:02","0:00:9007200.256743991","0:00:9007199254740991.975424",[{"type":"integer","value":"1","unit":"day"},{"type":"literal","value":" ","unit":"day"},{"type":"unit","value":"day","unit":"day"},{"type":"literal","value":", "},{"type":"integer","value":"2","unit":"hour"},{"type":"literal","value":" ","unit":"hour"},{"type":"unit","value":"hours","unit":"hour"}]]"#,
    );
}
#[test]
fn duration_record_reads_are_ordered_and_bounds_are_exact() {
    check(
        r#"
      var d=new Intl.DurationFormat('en'),log=[],mixed=false,fraction=false,tooLarge=false;
      try {d.format(new Proxy({days:1,hours:-1},{get(t,k){log.push(k);return t[k];}}));}catch(e){mixed=e instanceof RangeError;}
      try {d.format({get days(){return 0.1;},get hours(){throw 'wrong order';}});}catch(e){fraction=e instanceof RangeError;}
      try {d.format({seconds:9007199254740991,nanoseconds:1000000000});}catch(e){tooLarge=e instanceof RangeError;}
      JSON.stringify([log,mixed,fraction,tooLarge,typeof d.format({seconds:9007199254740991,nanoseconds:999999999})]);
    "#,
        r#"[["days","hours","microseconds","milliseconds","minutes","months","nanoseconds","seconds","weeks","years"],true,true,true,"string"]"#,
    );
}
#[test]
fn temporal_duration_formatting_reads_slots_without_prototype_getters() {
    check(
        r#"
      var d=new Temporal.Duration(0,0,0,1,2),f=new Intl.DurationFormat('en',{style:'long'});
      Object.defineProperty(Temporal.Duration.prototype,'days',{get(){throw 'tainted';}});
      JSON.stringify([f.format(d),f.format('P1DT2H')]);
    "#,
        r#"["1 day, 2 hours","1 day, 2 hours"]"#,
    );
}

#[test]
fn duration_options_resolve_numeric_chains_and_zero_signs() {
    check(
        r#"
      var d=new Intl.DurationFormat('en',{hours:'numeric'}),o=d.resolvedOptions();
      var a=new Intl.DurationFormat('en',{style:'digital',hoursDisplay:'auto'});
      var b=new Intl.DurationFormat('en',{style:'long',yearsDisplay:'always'});
      JSON.stringify([o.hours,o.minutes,o.seconds,o.milliseconds,o.minutesDisplay,
        a.format({hours:0,minutes:-1,seconds:-2}),b.format({hours:-2}),b.format({hours:-0})]);
    "#,
        r#"["numeric","2-digit","2-digit","numeric","always","-01:02","-0 years, 2 hours","0 years"]"#,
    );
}
