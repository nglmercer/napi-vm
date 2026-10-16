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
fn supported_values_of_has_builtin_shape() {
    check(
        r#"var f=Intl.supportedValuesOf,a=f('calendar'),b=f('calendar');var result=[typeof f,f.length,f.name,Object.prototype.hasOwnProperty.call(f,'prototype'),Object.isExtensible(f),Object.getPrototypeOf(f)===Function.prototype,a!==b,JSON.stringify(a)===JSON.stringify(b.slice().sort()),new Set(a).size===a.length];"#,
        r#"["function",1,"supportedValuesOf",false,true,true,true,true,true]"#,
    );
}
#[test]
fn supported_values_of_property_is_writable_nonenumerable() {
    check(
        r#"var d=Object.getOwnPropertyDescriptor(Intl,'supportedValuesOf');var result=[d.writable,d.enumerable,d.configurable];"#,
        r#"[true,false,true]"#,
    );
}
#[test]
fn supported_values_of_rejects_invalid_keys() {
    check(
        r#"var bad=['','calendars','collations','currencies','numberingSystems','timeZones','units','CALENDAR','Collation','Currency','numberingsystem','timezone','UNIT','hourCycle','locale',undefined,null,false,true,NaN,0,{},[]];var result=bad.map(function(k){try{Intl.supportedValuesOf(k);return 'no-throw'}catch(e){return e.name}});var symbolCase;try{Intl.supportedValuesOf(Symbol())}catch(e){symbolCase=e.name}result.push(symbolCase);"#,
        r#"["RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","RangeError","TypeError"]"#,
    );
}
#[test]
fn supported_values_of_coerces_keys() {
    check(
        r#"var direct=Intl.supportedValuesOf('calendar');var result=[JSON.stringify(Intl.supportedValuesOf(new String('calendar')))===JSON.stringify(direct),JSON.stringify(Intl.supportedValuesOf({toString:function(){return 'currency'}}))===JSON.stringify(Intl.supportedValuesOf('currency'))];"#,
        r#"[true,true]"#,
    );
}
#[test]
fn supported_calendars_match_era_monthcode_requirements() {
    check(
        r#"var result=Intl.supportedValuesOf('calendar');"#,
        r#"["buddhist","chinese","coptic","dangi","ethioaa","ethiopic","gregory","hebrew","indian","islamic-civil","islamic-tbla","islamic-umalqura","iso8601","japanese","persian","roc"]"#,
    );
}
#[test]
fn supported_collations_match_collator_behavior() {
    check(
        r#"var list=Intl.supportedValuesOf('collation');var result=[list,!list.includes('standard')&&!list.includes('search'),list.every(function(c){return new Intl.Locale('und',{collation:c}).collation===c})];"#,
        r#"[["compat","dict","ducet","emoji","eor","phonebk","phonetic","pinyin","searchjl","stroke","trad","unihan","zhuyin"],true,true]"#,
    );
}
#[test]
fn supported_currencies_are_iso_codes_accepted_by_numberformat() {
    check(
        r#"var list=Intl.supportedValuesOf('currency');var result=[list.length,list.includes('USD')&&list.includes('EUR')&&list.includes('JPY'),list.every(function(c){return /^[A-Z]{3}$/.test(c)}),list.every(function(c){return new Intl.NumberFormat('en',{style:'currency',currency:c}).resolvedOptions().currency===c})];"#,
        r#"[162,true,true,true]"#,
    );
}
#[test]
fn supported_time_zones_include_non_continental() {
    check(
        r#"var list=Intl.supportedValuesOf('timeZone');var result=[list.includes('UTC'),list.includes('America/New_York'),list.includes('Etc/GMT+1'),list.includes('Etc/GMT-14'),!list.includes('Etc/UTC')&&!list.includes('Etc/GMT'),new Set(list).size===list.length];"#,
        r#"[true,true,true,true,true,true]"#,
    );
}
#[test]
fn supported_units_match_numberformat_behavior() {
    check(
        r#"var list=Intl.supportedValuesOf('unit');var result=[list.length,list.includes('meter')&&list.includes('kilometer')&&list.includes('mile-scandinavian'),JSON.stringify(list)===JSON.stringify(list.slice().sort()),list.every(function(u){return new Intl.NumberFormat('en',{style:'unit',unit:u}).resolvedOptions().unit===u})];"#,
        r#"[45,true,true,true]"#,
    );
}
#[test]
fn supported_numbering_systems_cover_digit_mappings() {
    check(
        r#"var list=Intl.supportedValuesOf('numberingSystem');var result=[list.length,list.includes('latn')&&list.includes('arab')&&list.includes('ahom')&&list.includes('tibt'),JSON.stringify(list)===JSON.stringify(list.slice().sort()),list.every(function(nu){return new Intl.Locale('und',{numberingSystem:nu}).numberingSystem===nu})];"#,
        r#"[78,true,true,true]"#,
    );
}
#[test]
fn constructors_honor_numbering_systems_without_icu_support() {
    check(
        r#"var n=new Intl.NumberFormat('en',{numberingSystem:'ahom'}),r=new Intl.RelativeTimeFormat('en',{numberingSystem:'ahom'}),d=new Intl.DurationFormat('en',{numberingSystem:'ahom'});var result=[n.resolvedOptions().numberingSystem,r.resolvedOptions().numberingSystem,d.resolvedOptions().numberingSystem,n.format(12),r.format(3,'day'),d.format({hours:5}),new Intl.NumberFormat('en',{numberingSystem:'roman'}).resolvedOptions().numberingSystem];"#,
        r#"["ahom","ahom","ahom","𑜱𑜲","in 𑜳 days","𑜵 hr","latn"]"#,
    );
}
