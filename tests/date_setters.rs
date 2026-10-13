use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "prepared: {result:?} for {source}"
    );
    let mut parser = Parser::new(Lexer::new(source).tokenize());
    let statements = parser.parse_program().unwrap();
    let mut ast = Interpreter::with_builtins();
    let result = ast.run_program_body(&statements).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "AST: {result:?} for {source}"
    );
}

#[test]
fn utc_time_setters_use_make_date_time() {
    assert_both(
        "JSON.stringify([new Date(0).setUTCHours(1), new Date(0).setUTCHours(25), new Date(0).setUTCMinutes(61), new Date(0).setUTCSeconds(90), new Date(0).setUTCMilliseconds(1500), new Date(1000).setUTCMilliseconds(-1)]);",
        "[3600000,90000000,3660000,90000,1500,999]",
    );
}

#[test]
fn utc_date_setters_use_make_day() {
    assert_both(
        "JSON.stringify([new Date(Date.UTC(2020,0,1)).setUTCFullYear(2021), new Date(Date.UTC(2020,5,15)).setUTCFullYear(99), new Date(Date.UTC(2020,0,15)).setUTCMonth(13), new Date(Date.UTC(2020,0,15)).setUTCMonth(-1), new Date(Date.UTC(2021,2,15)).setUTCDate(0), new Date(Date.UTC(2021,0,31)).setUTCDate(32)]);",
        "[1609459200000,-59028739200000,1613347200000,1576368000000,1614470400000,1612137600000]",
    );
}

#[test]
fn local_setters_match_utc_setters_without_timezone() {
    assert_both(
        "var t = Date.UTC(2021,2,15,12,30,45,123); JSON.stringify([[new Date(t).setHours(7), new Date(t).setUTCHours(7)], [new Date(t).setMinutes(7), new Date(t).setUTCMinutes(7)], [new Date(t).setSeconds(7), new Date(t).setUTCSeconds(7)], [new Date(t).setMilliseconds(7), new Date(t).setUTCMilliseconds(7)], [new Date(t).setDate(7), new Date(t).setUTCDate(7)], [new Date(t).setMonth(7), new Date(t).setUTCMonth(7)], [new Date(t).setFullYear(1999), new Date(t).setUTCFullYear(1999)]]);",
        "[[1615793445123,1615793445123],[1615810065123,1615810065123],[1615811407123,1615811407123],[1615811445007,1615811445007],[1615120245123,1615120245123],[1629030645123,1629030645123],[921501045123,921501045123]]",
    );
}

#[test]
fn setters_coerce_arguments_in_order_before_nan_check() {
    assert_both(
        "var log=[];function arg(n){return {valueOf:function(){log.push(n);return 0}}};var d=new Date(NaN);var r=d.setHours(arg('h'),arg('m'),arg('s'),arg('ms'));JSON.stringify([log.join(','),String(r),String(d.getTime())]);",
        "[\"h,m,s,ms\",\"NaN\",\"NaN\"]",
    );
    assert_both(
        "var d=new Date(0);d.setHours(1,2,3,4);JSON.stringify([d.getUTCHours(),d.getUTCMinutes(),d.getUTCSeconds(),d.getUTCMilliseconds()]);",
        "[1,2,3,4]",
    );
}

#[test]
fn nan_this_returns_nan_except_full_year_uses_epoch() {
    assert_both(
        "JSON.stringify([String(new Date(NaN).setUTCHours(1)), String(new Date(NaN).setMonth(1)), String(new Date(NaN).setDate(1)), new Date(NaN).setUTCFullYear(2000), String(new Date(NaN).setUTCHours(1e300)), String(new Date(0).setUTCHours(1e300))]);",
        "[\"NaN\",\"NaN\",\"NaN\",946684800000,\"NaN\",\"NaN\"]",
    );
}

#[test]
fn missing_leading_argument_coerces_to_nan() {
    assert_both(
        "JSON.stringify([String(new Date(0).setHours()), String(new Date(0).setMinutes()), String(new Date(0).setSeconds()), String(new Date(0).setMilliseconds()), String(new Date(0).setDate()), String(new Date(0).setMonth()), String(new Date(0).setFullYear())]);",
        "[\"NaN\",\"NaN\",\"NaN\",\"NaN\",\"NaN\",\"NaN\",\"NaN\"]",
    );
    assert_both(
        "var d=new Date(0);JSON.stringify([d.setMinutes(1), d.setSeconds(2), d.setMilliseconds(3)]);",
        "[60000,62000,62003]",
    );
}

#[test]
fn setters_update_in_place_and_reject_bad_receivers() {
    assert_both(
        "var d=new Date(0);var r=d.setUTCHours(2);JSON.stringify([r,d.getTime(),d.setUTCFullYear.length,d.setUTCHours.length,d.setUTCMonth.length,d.setUTCDate.length,d.setUTCMinutes.length,d.setUTCSeconds.length,d.setUTCMilliseconds.length]);",
        "[7200000,7200000,3,4,2,1,3,2,1]",
    );
    assert_both(
        "var names=['setFullYear','setMonth','setDate','setHours','setMinutes','setSeconds','setMilliseconds'];var bad=0;for(var i=0;i<names.length;i++){try{Date.prototype[names[i]].call({},1)}catch(e){if(e instanceof TypeError)bad++}}JSON.stringify(bad);",
        "7",
    );
}

#[test]
fn symbol_to_primitive_follows_hint_order() {
    assert_both(
        "var d=new Date(0);JSON.stringify([d[Symbol.toPrimitive]('number'),typeof d[Symbol.toPrimitive]('string'),d[Symbol.toPrimitive]('default')]);",
        "[0,\"string\",\"1970-01-01T00:00:00.000Z\"]",
    );
    assert_both(
        "var d=new Date(0);var bad=0;try{d[Symbol.toPrimitive]()}catch(e){if(e instanceof TypeError)bad++}try{d[Symbol.toPrimitive]('bogus')}catch(e){if(e instanceof TypeError)bad++}try{Date.prototype[Symbol.toPrimitive].call(1,'number')}catch(e){if(e instanceof TypeError)bad++}JSON.stringify([bad,Date.prototype[Symbol.toPrimitive].name,Date.prototype[Symbol.toPrimitive].length]);",
        "[3,\"[Symbol.toPrimitive]\",1]",
    );
}

#[test]
fn annex_b_year_and_gmt_alias() {
    assert_both(
        "JSON.stringify([new Date(Date.UTC(2020,5,15)).getYear(), new Date(Date.UTC(2020,5,15)).setYear(50), new Date(Date.UTC(2020,5,15)).setYear(1999), new Date(Date.UTC(2020,5,15)).setYear(99.999999)]);",
        "[120,-616896000000,929404800000,929404800000]",
    );
    assert_both(
        "JSON.stringify([String(new Date(0).setYear()), Date.prototype.toGMTString === Date.prototype.toUTCString, new Date(0).getYear.length, new Date(0).setYear.length]);",
        "[\"NaN\",true,0,1]",
    );
}

#[test]
fn getters_reject_non_date_receivers() {
    assert_both(
        "var bad=0;try{Date.prototype.getTime()}catch(e){if(e instanceof TypeError)bad++}JSON.stringify(bad);",
        "1",
    );
}
