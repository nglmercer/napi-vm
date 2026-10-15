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
fn temporal_time_arithmetic_keeps_nanosecond_precision() {
    assert_tiers(
        r#"var t=new Temporal.PlainTime(23,59,59,999,999,999);var next=t.add({nanoseconds:2});JSON.stringify([t.toString(),next.toString(),t.until(next).toString(),next.since(t).toString(),t.round('second').toString(),Temporal.PlainTime.compare(t,next)]);"#,
        r#"["23:59:59.999999999","00:00:00.000000001","-PT23H59M59.999999998S","-PT23H59M59.999999998S","00:00:00",1]"#,
    );
}

#[test]
fn temporal_time_observes_fields_and_options_in_order() {
    assert_tiers(
        r#"var log=[];var item={get hour(){log.push('hour');return {valueOf:function(){log.push('number');return 25}}},get microsecond(){log.push('microsecond')},get millisecond(){log.push('millisecond')},get minute(){log.push('minute');return 60},get nanosecond(){log.push('nanosecond')},get second(){log.push('second')}};var t=Temporal.PlainTime.from(item,{get overflow(){log.push('overflow');return 'constrain'}});JSON.stringify([t.toString(),log]);"#,
        r#"["23:59:00",["hour","number","microsecond","millisecond","minute","nanosecond","second","overflow"]]"#,
    );
}

#[test]
fn temporal_brands_are_private_and_prototype_accessors_validate_receivers() {
    assert_tiers(
        r#"var t=new Temporal.PlainTime(12);var errors=0;try{Temporal.PlainTime()}catch(e){if(e instanceof TypeError)errors++}try{Temporal.PlainTime.prototype.toString.call({})}catch(e){if(e instanceof TypeError)errors++}try{Object.getOwnPropertyDescriptor(Temporal.PlainTime.prototype,'hour').get.call(new Proxy(t,{}))}catch(e){if(e instanceof TypeError)errors++}try{+t}catch(e){if(e instanceof TypeError)errors++}JSON.stringify([errors,Object.keys(t),Object.getOwnPropertyNames(t),Object.getPrototypeOf(t)===Temporal.PlainTime.prototype,Object.prototype.toString.call(t)]);"#,
        r#"[4,[],[],true,"[object Temporal.PlainTime]"]"#,
    );
}

#[test]
fn temporal_duration_preserves_fields_and_rejects_mixed_signs() {
    assert_tiers(
        r#"var d=Temporal.Duration.from('P1DT2H3M4.005006007S');var bad=0;try{new Temporal.Duration(1,-1)}catch(e){if(e instanceof RangeError)bad++}try{new Temporal.Duration(0,0,0,0,1.5)}catch(e){if(e instanceof RangeError)bad++}JSON.stringify([d.toString(),d.negated().toString(),d.with({days:0}).add('PT1H').toString(),d.sign,d.blank,d.microseconds,d.nanoseconds,bad]);"#,
        r#"["P1DT2H3M4.005006007S","-P1DT2H3M4.005006007S","PT3H3M4.005006007S",1,false,6,7,2]"#,
    );
}

#[test]
fn temporal_duration_rounds_and_totals_relative_to_calendar_dates() {
    assert_tiers(
        r#"var d=new Temporal.Duration(1,0,0,0,24);var result=d.total({unit:'days',relativeTo:{year:2000,month:1,day:1}});var rounded=Temporal.Duration.from('PT1H45M').round({smallestUnit:'hour',roundingMode:'halfEven'});var comparison=Temporal.Duration.compare('P1Y','P366D',{relativeTo:'2000-01-01'});JSON.stringify([result,rounded.toString(),comparison,Temporal.Duration.from('PT1H30M').total('minutes')]);"#,
        r#"[367,"PT2H",0,90]"#,
    );
}

#[test]
fn temporal_shorthand_options_have_no_object_prototype() {
    assert_tiers(
        r#"var t=new Temporal.PlainTime(12,30);Object.defineProperty(Object.prototype,'roundingIncrement',{get:function(){throw 'polluted'},configurable:true});var s=t.round('hour').toString();var d=Temporal.Duration.from('PT1H30M').round('hour').toString();delete Object.prototype.roundingIncrement;JSON.stringify([s,d]);"#,
        r#"["13:00:00","PT2H"]"#,
    );
}

#[test]
fn temporal_string_options_coerce_non_numbers_with_string_hint() {
    assert_tiers(
        r#"var log=[];var t=new Temporal.PlainTime(12,34,56,123);var s=t.toString({fractionalSecondDigits:{toString:function(){log.push('string');return 'auto'},valueOf:function(){throw 'number'}}});var bad=0;try{t.toString({fractionalSecondDigits:null})}catch(e){if(e instanceof RangeError)bad++}JSON.stringify([s,log,bad]);"#,
        r#"["12:34:56.123",["string"],1]"#,
    );
}

#[test]
fn temporal_duration_total_uses_timezone_transitions() {
    assert_tiers(
        r#"var day=Temporal.Duration.from('P1D');var spring=day.total({unit:'hours',relativeTo:'2020-03-08T00:00-08:00[America/Los_Angeles]'});var autumn=day.total({unit:'hours',relativeTo:{year:2020,month:11,day:1,timeZone:'America/Los_Angeles'}});JSON.stringify([spring,autumn]);"#,
        "[23,25]",
    );
}
