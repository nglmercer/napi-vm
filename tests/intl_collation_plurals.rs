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
fn bound_comparison_survives_collection_and_observes_locale_options() {
    check(
        r#"var c=new Intl.Collator('en',{numeric:true,sensitivity:'base'}),compare=c.compare;var result=[compare===c.compare,compare('file2','file10')<0,compare('É','e')===0,'o\u0308'.localeCompare('ö'),c.resolvedOptions().numeric];"#,
        r#"[true,true,true,0,true]"#,
    );
}
#[test]
fn plural_selection_uses_fraction_precision_and_ordinal_rules() {
    check(
        r#"var en=new Intl.PluralRules('en'),decimal=new Intl.PluralRules('en',{minimumFractionDigits:1}),ordinal=new Intl.PluralRules('en',{type:'ordinal'}),ar=new Intl.PluralRules('ar');var result=[en.select(1),decimal.select(1),ordinal.select(2),ar.resolvedOptions().pluralCategories, en.selectRange(1,2),new Intl.PluralRules('fr',{notation:'compact'}).select(1500000)];"#,
        r#"["one","other","two",["zero","one","two","few","many","other"],"other","many"]"#,
    );
}
#[test]
fn stack_accessor_preserves_original_capture_and_uses_proxy_operations() {
    check(
        r#"var e=new Error('original'),get=Object.getOwnPropertyDescriptor(Error.prototype,'stack').get,set=Object.getOwnPropertyDescriptor(Error.prototype,'stack').set;var initial=e.stack;var ownBefore=Object.hasOwn(e,'stack');e.stack='replacement';var events=[],proxy=new Proxy({}, {getOwnPropertyDescriptor(t,k){events.push('own');return undefined},defineProperty(t,k,d){events.push('define');return Reflect.defineProperty(t,k,d)}});set.call(proxy,'value');var result=[ownBefore,e.stack,get.call(e)===initial,proxy.stack,events];"#,
        r#"[false,"replacement",true,"value",["own","define"]]"#,
    );
}
#[test]
fn bigint_width_coercion_precedes_value_coercion_and_radix_is_observable() {
    check(
        r#"var log=[],a=BigInt.asIntN({valueOf(){log.push('bits');return 8}},{valueOf(){log.push('value');return 255n}});var result=[String(a),log,BigInt.asUintN(9007199254740991,12n).toString(16),255n.toString({valueOf(){return 16}}),RegExp.escape('a-b\u00a0\ud800')];"#,
        r#"["-1",["bits","value"],"c","ff","\\x61\\x2db\\xa0\\ud800"]"#,
    );
}
