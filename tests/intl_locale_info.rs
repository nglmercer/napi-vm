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
fn locale_get_collations_keyword_and_root() {
    check(
        "var result=[new Intl.Locale('en-u-co-phonebk').getCollations(),new Intl.Locale('und').getCollations(),new Intl.Locale('qfz').getCollations()];",
        r#"[["phonebk"],["emoji","eor"],["emoji","eor"]]"#,
    );
}
#[test]
fn locale_get_collations_sorted_without_standard_or_search() {
    check(
        "var c=new Intl.Locale('de').getCollations();var result=[c.length>0,c.join(',')===[].concat(c).sort().join(','),c.indexOf('standard'),c.indexOf('search')];",
        r#"[true,true,-1,-1]"#,
    );
}
#[test]
fn locale_get_numbering_systems_keyword_and_default() {
    check(
        "var result=[new Intl.Locale('en-u-nu-arab').getNumberingSystems(),new Intl.Locale('en').getNumberingSystems().length>0];",
        r#"[["arab"],true]"#,
    );
}
#[test]
fn locale_get_time_zones_region_or_undefined() {
    check(
        "var z=new Intl.Locale('en-US').getTimeZones();var result=[new Intl.Locale('en').getTimeZones()===undefined,z.length>0,z.join('|')===[].concat(z).sort().join('|'),z.indexOf('America/New_York')>=0];",
        r#"[true,true,true,true]"#,
    );
}
