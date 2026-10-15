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

#[test]
fn temporal_instants_preserve_epoch_precision_and_rounding() {
    assert_tiers(
        r#"var t=new Temporal.Instant(-1n);var u=t.add({nanoseconds:2});JSON.stringify([t.epochNanoseconds.toString(),t.epochMilliseconds,u.toString(),t.until(u).toString(),u.round('second').epochNanoseconds.toString(),Temporal.Instant.compare(t,u),u.equals('1970-01-01T00:00:00.000000001Z')]);"#,
        r#"["-1",-1,"1970-01-01T00:00:00.000000001Z","PT0.000000002S","0",-1,true]"#,
    );
}

#[test]
fn temporal_instant_factories_coerce_and_check_bounds() {
    assert_tiers(
        r#"var log=[];var t=Temporal.Instant.fromEpochNanoseconds({valueOf:function(){log.push('bigint');return '1234567890123456789'}});var errors=[];for(var f of [function(){return new Temporal.Instant(1)},function(){return Temporal.Instant.fromEpochMilliseconds(1.5)},function(){return new Temporal.Instant(8640000000000000000001n)},function(){return Temporal.Instant.prototype.toString.call({})},function(){return t.add({days:1})}]){try{f()}catch(e){errors.push(e.name)}}JSON.stringify([t.epochNanoseconds.toString(),log,errors,Temporal.Instant.fromEpochMilliseconds(-1).toJSON()]);"#,
        r#"["1234567890123456789",["bigint"],["TypeError","RangeError","RangeError","TypeError","RangeError"],"1969-12-31T23:59:59.999Z"]"#,
    );
}

#[test]
fn temporal_dates_balance_calendar_arithmetic_and_retain_identity() {
    assert_tiers(
        r#"var d=new Temporal.PlainDate(2024,2,29);var n=d.add({years:1});JSON.stringify([d.toString(),n.toString(),d.until(n,{largestUnit:'year'}).toString(),d.with({year:2023}).toString(),d.dayOfWeek,d.dayOfYear,d.daysInYear,d.inLeapYear,d.monthCode,d.calendarId,Temporal.PlainDate.compare(d,n),d.equals('2024-02-29'),d!==Temporal.PlainDate.from(d)]);"#,
        r#"["2024-02-29","2025-02-28","P11M30D","2023-02-28",4,60,366,true,"M02","iso8601",-1,true,true]"#,
    );
}

#[test]
fn temporal_date_fields_have_observable_coercion_order() {
    assert_tiers(
        r#"var log=[];var d=Temporal.PlainDate.from({get calendar(){log.push('calendar')},get day(){log.push('day');return {valueOf:function(){log.push('day number');return 40}}},get month(){log.push('month');return 2},get monthCode(){log.push('monthCode')},get year(){log.push('year');return 2024}},{get overflow(){log.push('overflow');return 'constrain'}});var errors=[];for(var f of [function(){return new Temporal.PlainDate(2023,2,29)},function(){return d.with({})},function(){return d.with({calendar:'iso8601'})},function(){return Temporal.PlainDate.prototype.year},function(){return d.with({day:40},{overflow:'reject'})}]){try{f()}catch(e){errors.push(e.name)}}JSON.stringify([d.toString(),log,errors]);"#,
        r#"["2024-02-29",["calendar","day","day number","month","monthCode","year","overflow"],["RangeError","TypeError","TypeError","TypeError","RangeError"]]"#,
    );
}

#[test]
fn temporal_date_calendars_accept_valid_iso_strings_and_ignore_unused_eras() {
    assert_tiers(
        r#"var d=Temporal.PlainDate.from({year:2024,monthCode:'M02',day:29,calendar:'2020-01-01[u-ca=iso8601]'});var c=Temporal.PlainDate.from({year:2025,month:1,day:1,calendar:'chinese',era:'ignored',eraYear:1});var errors=[];for(var monthCode of [5,5n,false,null,{toString:function(){return 5}}]){try{Temporal.PlainDate.from({year:2024,day:1,monthCode:monthCode})}catch(e){errors.push(e.name)}}JSON.stringify([d.toString(),d.withCalendar('T15:23').calendarId,c.calendarId,c.year,errors]);"#,
        r#"["2024-02-29","iso8601","chinese",2025,["TypeError","TypeError","TypeError","TypeError","TypeError"]]"#,
    );
}

#[test]
fn temporal_datetime_balances_exact_time_and_calendar_fields() {
    assert_tiers(
        r#"var d=new Temporal.PlainDateTime(2024,2,29,23,59,59,999,999,999);var n=d.add({nanoseconds:2});JSON.stringify([d.toString(),n.toString(),d.until(n).toString(),d.round('second').toString(),n.toPlainDate().toString(),n.toPlainTime().toString(),Temporal.PlainDate.from(n).toString(),Temporal.PlainTime.from(n).toString(),d.withPlainTime().toString(),d.with({year:2023}).toString()]);"#,
        r#"["2024-02-29T23:59:59.999999999","2024-03-01T00:00:00.000000001","PT0.000000002S","2024-03-01T00:00:00","2024-03-01","00:00:00.000000001","2024-03-01","00:00:00.000000001","2024-02-29T00:00:00","2023-02-28T23:59:59.999999999"]"#,
    );
}

#[test]
fn temporal_datetime_fields_convert_before_reading_the_next_field() {
    assert_tiers(
        r#"var log=[];function field(k,v){return {get:function(){log.push(k);return v},enumerable:true}}var bag={};for(var kv of [['calendar',undefined],['day',1],['hour',{valueOf:function(){log.push('hour number');return 25}}],['microsecond',undefined],['millisecond',undefined],['minute',-3],['month',3],['monthCode',undefined],['nanosecond',undefined],['second',undefined],['year',2024]]){Object.defineProperty(bag,kv[0],field(kv[0],kv[1]))}var d=Temporal.PlainDateTime.from(bag,{get overflow(){log.push('overflow');return 'constrain'}});JSON.stringify([d.toString(),log,new Temporal.PlainDate(2024,3,1).toPlainDateTime('12:34').toString()]);"#,
        r#"["2024-03-01T23:00:00",["calendar","day","hour","hour number","microsecond","millisecond","minute","month","monthCode","nanosecond","second","year","overflow"],"2024-03-01T12:34:00"]"#,
    );
}

#[test]
fn temporal_year_month_and_month_day_preserve_calendar_semantics() {
    assert_tiers(
        r#"var y=new Temporal.PlainYearMonth(2024,2);var m=new Temporal.PlainMonthDay(2,29);var d=new Temporal.PlainDate(2024,2,29);JSON.stringify([y.toString(),y.add({months:12}).toString(),y.until('2025-03').toString(),y.daysInMonth,y.inLeapYear,y.toPlainDate({day:31}).toString(),m.toString(),m.toPlainDate({year:2023}).toString(),d.toPlainYearMonth().equals(y),d.toPlainMonthDay().equals(m),y.with({month:3}).toString(),m.with({day:28}).toString()]);"#,
        r#"["2024-02","2025-02","P1Y1M",29,true,"2024-02-29","02-29","2023-02-28",true,true,"2024-03","02-28"]"#,
    );
}

#[test]
fn temporal_year_month_reads_only_relevant_fields_in_order() {
    assert_tiers(
        r#"var log=[];var y=Temporal.PlainYearMonth.from({get calendar(){log.push('calendar')},get day(){throw new Error('day')},get month(){log.push('month');return {valueOf:function(){log.push('number');return 13}}},get monthCode(){log.push('monthCode')},get year(){log.push('year');return 2024}},{get overflow(){log.push('overflow');return 'constrain'}});var errors=[];for(var f of [function(){return new Temporal.PlainMonthDay(2,30)},function(){return new Temporal.PlainYearMonth(2024,13)},function(){return y.with({})},function(){return Temporal.PlainMonthDay.prototype.day}]){try{f()}catch(e){errors.push(e.name)}}JSON.stringify([y.toString(),log,errors]);"#,
        r#"["2024-12",["calendar","month","number","monthCode","year","overflow"],["RangeError","RangeError","TypeError","TypeError"]]"#,
    );
}

#[test]
fn temporal_zoned_datetime_respects_dst_calendar_and_exact_epoch() {
    assert_tiers(
        r#"var d=Temporal.ZonedDateTime.from('2024-03-10T00:00-05:00[America/New_York]');var day=d.add({days:1});var hours=d.add({hours:24});JSON.stringify([d.hoursInDay,day.toString(),hours.toString(),d.until(day,{largestUnit:'day'}).toString(),d.until(day,{largestUnit:'hour'}).toString(),d.offsetNanoseconds,d.toInstant().toString(),d.withTimeZone('UTC').hour,d.toPlainDate().toString(),d.toPlainTime().toString(),d.toPlainDateTime().toString(),Temporal.Instant.from(d).equals(d.toInstant())]);"#,
        r#"[23,"2024-03-11T00:00:00-04:00[America/New_York]","2024-03-11T01:00:00-04:00[America/New_York]","P1D","PT23H",-18000000000000,"2024-03-10T05:00:00Z",5,"2024-03-10","00:00:00","2024-03-10T00:00:00",true]"#,
    );
}

#[test]
fn temporal_zoned_conversions_disambiguate_gaps_and_overlap() {
    assert_tiers(
        r#"var gap=new Temporal.PlainDateTime(2024,3,10,2,30);var early=gap.toZonedDateTime('America/New_York',{disambiguation:'earlier'});var late=gap.toZonedDateTime('America/New_York',{disambiguation:'later'});var errors=[];for(var f of [function(){return gap.toZonedDateTime('America/New_York',{disambiguation:'reject'})},function(){return new Temporal.ZonedDateTime(1,'UTC')},function(){return Temporal.ZonedDateTime.prototype.hoursInDay},function(){return late.with({timeZone:'UTC'})}]){try{f()}catch(e){errors.push(e.name)}}JSON.stringify([early.toString(),late.toString(),late.toInstant().toZonedDateTimeISO('UTC').toString(),new Temporal.PlainDate(2024,3,10).toZonedDateTime('America/New_York').hoursInDay,errors]);"#,
        r#"["2024-03-10T01:30:00-05:00[America/New_York]","2024-03-10T03:30:00-04:00[America/New_York]","2024-03-10T07:30:00+00:00[UTC]",23,["RangeError","TypeError","TypeError","TypeError"]]"#,
    );
}

#[test]
fn temporal_zoned_constructor_and_transition_validate_required_arguments() {
    assert_tiers(
        r#"var errors=[];for(var f of [function(){return new Temporal.ZonedDateTime(0n,'2024-01-01T00:00Z[UTC]')},function(){return new Temporal.ZonedDateTime(0n,'UTC').getTimeZoneTransition()},function(){return new Temporal.ZonedDateTime(0n,'UTC').getTimeZoneTransition({})}]){try{f()}catch(e){errors.push(e.name)}}JSON.stringify([errors,new Temporal.ZonedDateTime(-1n,'UTC').epochNanoseconds.toString(),new Temporal.ZonedDateTime(0n,'+01:00').getTimeZoneTransition('next')]);"#,
        r#"[["RangeError","TypeError","RangeError"],"-1",null]"#,
    );
}
