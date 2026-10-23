use napi_vm_core::{Interpreter, Value, parser::parse_cached};

fn yes(source: &str) {
    let mut prepared = Interpreter::with_builtins();
    assert!(
        matches!(prepared.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
    let mut ast = Interpreter::with_builtins();
    assert!(matches!(
        ast.run_program_body(&parse_cached(source).unwrap())
            .unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn plain_values_share_intl_patterns_and_ignore_zone_offsets() {
    yes(
        "var date=new Temporal.PlainDateTime(2021,8,4,0,30,45);var options={timeZone:'Pacific/Apia',hourCycle:'h23',year:'numeric',month:'numeric',day:'numeric',hour:'numeric',minute:'numeric',second:'numeric'};date.toLocaleString('en',options)==='8/4/2021, 00:30:45' && date.toLocaleString('en',options)===new Intl.DateTimeFormat('en',options).format(date)",
    );
    yes(
        "var time=new Temporal.PlainTime(14,46);time.toLocaleString('en',{era:'narrow'}).startsWith('2') && !time.toLocaleString('en',{era:'narrow'}).includes('A')",
    );
}

#[test]
fn zoned_values_use_their_zone_and_reject_an_option_override() {
    yes(
        "var value=new Temporal.ZonedDateTime(0n,'America/New_York');var options={hour:'numeric',minute:'numeric',timeZoneName:'short'};var same=value.toLocaleString('en',options)===new Date(0).toLocaleString('en',{hour:'numeric',minute:'numeric',timeZoneName:'short',timeZone:'America/New_York'});var caught=false;try{value.toLocaleString('en',{timeZone:'America/New_York'});}catch(e){caught=e instanceof TypeError;}same && caught",
    );
}

#[test]
fn calendars_styles_and_receiver_brands_are_checked() {
    yes(
        "var caught=0;try{new Temporal.PlainMonthDay(1,1).toLocaleString('en');}catch(e){if(e instanceof RangeError)caught++;}try{new Temporal.PlainDate(2026,1,20).toLocaleString('en',{dateStyle:'full',timeStyle:'full'});}catch(e){if(e instanceof TypeError)caught++;}try{Temporal.PlainTime.prototype.toLocaleString.call(new Temporal.PlainDate(2000,1,1));}catch(e){if(e instanceof TypeError)caught++;}caught===3 && typeof new Temporal.PlainMonthDay(1,1).toLocaleString('en',{calendar:'iso8601'})==='string'",
    );
}

#[test]
fn durations_use_the_intrinsic_formatter_and_branded_fields() {
    yes(
        "var value=Temporal.Duration.from({hours:2,minutes:30});var expected=new Intl.DurationFormat('de',{style:'long'}).format(value);Intl.DurationFormat=function(){throw 99;};value.toLocaleString('de',{style:'long'})===expected",
    );
}

#[test]
fn default_hour_cycles_keep_midnight_and_calendar_reference_dates() {
    yes(
        "var time=new Temporal.PlainTime(0);time.toLocaleString('en',{hourCycle:'h24'}).includes('24:00:00') && time.toLocaleString('en',{hourCycle:'h11'}).includes('0:00:00')",
    );
    yes(
        "var value=Temporal.PlainYearMonth.from({year:1445,monthCode:'M09',calendar:'islamic-tbla'});value.toLocaleString('en-u-ca-islamic-tbla',{dateStyle:'long'}).includes('Ramadan')",
    );
    yes(
        "var value=Temporal.PlainMonthDay.from({monthCode:'M09',day:1,calendar:'islamic-tbla'});value.toLocaleString('en-u-ca-islamic-tbla',{dateStyle:'long'}).includes('Ramadan')",
    );
}

#[test]
fn formatting_keeps_temporal_dates_outside_date_time_clip() {
    yes(
        "var f=new Intl.DateTimeFormat('en',{calendar:'iso8601'});f.format(new Temporal.PlainDate(-271821,4,19)).includes('271822') && f.format(new Temporal.PlainYearMonth(-271821,4)).includes('271822')",
    );
}
