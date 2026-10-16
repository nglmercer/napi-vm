use napi_vm::{Interpreter, Value};
fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    vm.collect_cycles();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "{result:?}"
    );
}
#[test]
fn datetime_preserves_time_zone_identifiers() {
    check(
        r#"var result=["UTC","Australia/Canberra","Etc/GMT","america/yellowknife","Asia/Calcutta","utc","-00"].map(function(name){return new Intl.DateTimeFormat([],{timeZone:name}).resolvedOptions().timeZone});"#,
        r#"["UTC","Australia/Canberra","Etc/GMT","America/Yellowknife","Asia/Calcutta","UTC","+00:00"]"#,
    );
}
#[test]
fn datetime_derives_hour12_from_resolved_cycle() {
    check(
        r#"var f=function(l,o){var r=new Intl.DateTimeFormat(l,o).resolvedOptions();return r.hourCycle+"|"+r.hour12};var result=[f("de-u-hc-h23",{hour:"numeric"}),f("en",{hour:"numeric"}),f("fr",{hour:"numeric",hour12:true,hourCycle:"h23"}),f("ja",{hour:"numeric",hour12:true}),f("en",{hour:"numeric",hour12:false})];"#,
        r#"["h23|false","h12|true","h12|true","h11|true","h23|false"]"#,
    );
}
#[test]
fn datetime_hides_hour_cycle_without_time() {
    check(
        r#"var f=function(l,o){var r=new Intl.DateTimeFormat(l,o).resolvedOptions();return r.hourCycle+"|"+r.hour12};var result=[f("de-u-hc-h11",{dateStyle:"short"}),f("en-US",{dateStyle:"short",hourCycle:"h23"})];"#,
        r#"["undefined|undefined","undefined|undefined"]"#,
    );
}
#[test]
fn datetime_reflects_matching_unicode_extensions() {
    check(
        r#"var f=function(l,o){return new Intl.DateTimeFormat(l,o).resolvedOptions().locale};var result=[f("en-u-hc-h23",{hour:"numeric",hourCycle:"h23"}),f("en-u-hc-h23",{hour:"numeric",hourCycle:"h11"}),f("en-u-ca-iso8601",{calendar:"invalid"}),f("en-u-ca-gregory",{calendar:"iso8601"})];"#,
        r#"["en-u-hc-h23","en","en-u-ca-iso8601","en"]"#,
    );
}
#[test]
fn datetime_formats_full_numeric_year() {
    check(
        r#"var result=new Intl.DateTimeFormat("en-US").format(new Date(2019, 0, 3));"#,
        r#""1/3/2019""#,
    );
}
#[test]
fn datetime_formats_minute_second_without_hour() {
    check(
        r#"var dtf=new Intl.DateTimeFormat("en",{minute:"numeric",second:"numeric"});var result=[dtf.format(new Date(2019,7,10,1,2,3)),dtf.formatToParts(new Date(2019,7,10,1,2,3)).length];"#,
        r#"["02:03",3]"#,
    );
}
#[test]
fn datetime_pads_arab_two_digit_hour() {
    check(
        r#"var result=new Intl.DateTimeFormat("en-US-u-nu-arab",{hour:"2-digit",minute:"2-digit",second:"2-digit"}).format(new Date(2024,0,1,2,35,6)).includes("٠٢:٣٥:٠٦");"#,
        r#"true"#,
    );
}
#[test]
fn datetime_names_day_period_with_hour() {
    check(
        r#"var dtf=new Intl.DateTimeFormat("en",{dayPeriod:"long",hour:"numeric"});var result=[dtf.format(new Date(2017,11,12,7)),dtf.format(new Date(2017,11,12,12))];"#,
        r#"["7 in the morning","12 noon"]"#,
    );
}
#[test]
fn datetime_fraction_part_type() {
    check(
        r#"var parts=new Intl.DateTimeFormat("en",{minute:"numeric",second:"numeric",fractionalSecondDigits:1}).formatToParts(new Date(2019,7,10,1,2,3,234));var result=[parts.length,parts[3].type,parts[4].type,parts[4].value];"#,
        r#"[5,"literal","fractionalSecond","2"]"#,
    );
}
#[test]
fn datetime_formats_year_month_with_time() {
    check(
        r#"var result=[new Date(0).toLocaleString([],{year:"numeric",month:"numeric",timeZone:"UTC"}),new Date(0).toLocaleDateString([],{hour:"numeric",minute:"numeric",timeZone:"UTC"}).length > 0];"#,
        r#"["1/1970",true]"#,
    );
}
#[test]
fn datetime_formats_arab_fraction_separator() {
    check(
        r#"var result=new Intl.DateTimeFormat("en-US-u-nu-arab",{hour:"numeric",minute:"numeric",second:"numeric",fractionalSecondDigits:3}).format(new Date(2024,0,1,2,35,6,789)).includes("٢:٣٥:٠٦٫٧٨٩");"#,
        r#"true"#,
    );
}
#[test]
fn datetime_formats_day_period_names() {
    check(
        r#"var result=[[7,"long"],[12,"long"],[15,"long"],[19,"long"],[22,"long"],[12,"narrow"],[0,"long"]].map(function(args){return new Intl.DateTimeFormat("en",{dayPeriod:args[1]}).format(new Date(2017,11,12,args[0]))});"#,
        r#"["in the morning","noon","in the afternoon","in the evening","at night","n","in the morning"]"#,
    );
}
#[test]
fn datetime_day_period_only_part_type() {
    check(
        r#"var parts=new Intl.DateTimeFormat("en",{dayPeriod:"long"}).formatToParts(new Date(2017,11,12,7));var result=[parts.length,parts[0].type,parts[0].value];"#,
        r#"[1,"dayPeriod","in the morning"]"#,
    );
}
#[test]
fn datetime_formats_h24_midnight_as_24() {
    check(
        r#"var result=new Intl.DateTimeFormat("en-US-u-hc-h24",{hour:"numeric",minute:"numeric",timeZone:"UTC"}).format(new Date("2019-08-10T00:05:00Z"));"#,
        r#""24:05""#,
    );
}
#[test]
fn datetime_collapses_identical_range() {
    check(
        r#"var dtf=new Intl.DateTimeFormat("en-US");var d=new Date(2019,0,3);var result=[dtf.formatRange(d,d),dtf.formatRangeToParts(d,d).length,dtf.formatRangeToParts(d,d).every(function(p){return p.source==="shared"})];"#,
        r#"["1/3/2019",5,true]"#,
    );
}
#[test]
fn datetime_format_defaults_to_now() {
    check(
        r#"var result=typeof new Intl.DateTimeFormat().format();"#,
        r#""string""#,
    );
}
#[test]
fn datetime_range_rejects_undefined() {
    check(
        r#"var dtf=new Intl.DateTimeFormat();var result=["TypeError","TypeError"].map(function(name,i){try{i===0?dtf.formatRange():dtf.formatRangeToParts(undefined,undefined);return "no-throw"}catch(e){return e.name}});"#,
        r#"["TypeError","TypeError"]"#,
    );
}
#[test]
fn datetime_formats_time_range_edges() {
    check(
        r#"var dtf=new Intl.DateTimeFormat();var result=[typeof dtf.format(-8.64e15),typeof dtf.format(8.64e15)];"#,
        r#"["string","string"]"#,
    );
}
#[test]
fn datetime_exposes_chinese_related_year_parts() {
    check(
        r#"var parts=new Intl.DateTimeFormat("zh-u-ca-chinese",{year:"numeric"}).formatToParts(new Date(2019,5,1));var result=parts.map(function(p){return p.type});"#,
        r#"["relatedYear","yearName","literal"]"#,
    );
}
#[test]
fn datetime_call_on_instance_installs_fallback_symbol() {
    check(
        r#"var object=new Intl.DateTimeFormat();var back=Intl.DateTimeFormat.call(object);var symbols=Object.getOwnPropertySymbols(back);var result=[back===object,symbols.length,symbols[0].description,back[symbols[0]] instanceof Intl.DateTimeFormat];"#,
        r#"[true,1,"IntlLegacyConstructedSymbol",true]"#,
    );
}
#[test]
fn datetime_call_on_fresh_object_returns_it() {
    check(
        r#"var object=Object.create(Intl.DateTimeFormat.prototype);var back=Intl.DateTimeFormat.call(object);var symbols=Object.getOwnPropertySymbols(back);var desc=Object.getOwnPropertyDescriptor(back,symbols[0]);var result=[back===object,symbols[0].description,back[symbols[0]] instanceof Intl.DateTimeFormat,desc.writable,desc.enumerable,desc.configurable];"#,
        r#"[true,"IntlLegacyConstructedSymbol",true,false,false,false]"#,
    );
}
#[test]
fn datetime_adapts_bare_default_to_temporal_class() {
    check(
        r#"var f=new Intl.DateTimeFormat("en-US",{timeZone:"Pacific/Apia"});var result=[f.format(new Temporal.PlainDate(2021,8,4)),f.format(new Temporal.PlainDateTime(2021,8,4,0,30,45)),f.format(new Temporal.PlainMonthDay(8,4,"gregory")),f.format(new Temporal.PlainTime(0,30,45)),f.format(new Temporal.PlainYearMonth(2021,8,"gregory"))];"#,
        r#"["8/4/2021","8/4/2021, 12:30:45 AM","8/4","12:30:45 AM","8/2021"]"#,
    );
}
#[test]
fn datetime_rejects_mismatched_temporal_fields() {
    check(
        r#"var result=[];try{new Intl.DateTimeFormat("en",{hour:"numeric"}).format(new Temporal.PlainDate(2021,8,4));result.push("no-throw")}catch(e){result.push(e.name)}try{new Intl.DateTimeFormat("en",{dateStyle:"short"}).format(new Temporal.PlainTime(12,30));result.push("no-throw")}catch(e){result.push(e.name)}try{new Intl.DateTimeFormat("en",{timeZoneName:"long"}).format(new Temporal.PlainDate(2021,8,4));result.push("named")}catch(e){result.push(e.name)};"#,
        r#"["TypeError","TypeError","named"]"#,
    );
}
#[test]
fn datetime_range_checks_temporal_pair() {
    check(
        r#"var dtf=new Intl.DateTimeFormat();var result=[];try{dtf.formatRange(new Temporal.PlainDate(2021,8,4),new Temporal.PlainTime(12,30));result.push("no-throw")}catch(e){result.push(e.name)}try{dtf.formatRange(Temporal.PlainDateTime.from("1976-11-18T14:23:30+00:00[UTC]"),Temporal.PlainDateTime.from("2020-02-20T15:44:56-05:00[America/New_York]").withCalendar("japanese"));result.push("no-throw")}catch(e){result.push(e.name)};"#,
        r#"["TypeError","RangeError"]"#,
    );
}
#[test]
fn date_to_locale_methods_use_intl_defaults() {
    check(
        r#"var d=new Date(Date.UTC(2019,0,3,14,5,6));var result=[d.toLocaleDateString("en-US",{timeZone:"UTC"}),d.toLocaleTimeString("en-US",{timeZone:"UTC",hour12:false}),new Date(NaN).toLocaleString()];"#,
        r#"["1/3/2019","14:05:06","Invalid Date"]"#,
    );
}
