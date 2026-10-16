use napi_vm::{Interpreter, Value};

fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "{result:?}"
    );
}

#[test]
fn builtin_exec_coerces_last_index_lazily_and_does_not_advance_empty_matches() {
    check(
        r#"var log=[],r=/./y; r.lastIndex={valueOf(){log.push('convert');return 1.9}};var before=log.length;var m=r.exec('abc');var e=/(?:)/g; e.exec('');var result=[before,m[0],r.lastIndex,e.lastIndex,log];"#,
        r#"[0,"b",2,0,["convert"]]"#,
    );
}

#[test]
fn match_and_search_observe_custom_exec_and_restore_last_index() {
    check(
        r#"var n=0,rx={flags:'g',lastIndex:5,exec(s){if(n++===0)return {0:'x'};return null}};var matches=RegExp.prototype[Symbol.match].call(rx,'xyz');var searcher={lastIndex:-0,exec(s){this.lastIndex=9;return {index:7}}};var found=RegExp.prototype[Symbol.search].call(searcher,'a');var result=[matches,found,Object.is(searcher.lastIndex,-0)];"#,
        r#"[["x"],7,true]"#,
    );
}

#[test]
fn string_protocol_dispatch_precedes_string_conversion_and_flags_are_generic() {
    check(
        r#"var log=[],receiver={toString(){log.push('string');return 'a'}},pattern={[Symbol.match](x){log.push(x===receiver);return 42}};var output=String.prototype.match.call(receiver,pattern);var flags=Object.getOwnPropertyDescriptor(RegExp.prototype,'flags').get.call({global:true,unicodeSets:true,sticky:true});var result=[output,log,flags,RegExp.prototype.toString.call({source:'x',flags:'m'})];"#,
        r#"[42,[true],"gvy","/x/m"]"#,
    );
}

#[test]
fn match_all_iterator_keeps_species_matcher_closures_alive() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(r#"var iterator;(function(){var count=0;var rx={flags:'g',lastIndex:0,constructor:{[Symbol.species]:function(){return {lastIndex:0,exec(){return count++ ? null : {0:'x',index:0,length:1}}}}}};iterator=RegExp.prototype[Symbol.matchAll].call(rx,'x')})();"#).unwrap();
    vm.collect_cycles();
    let result = vm
        .eval_source("JSON.stringify([iterator.next().value[0],iterator.next().done])")
        .unwrap();
    assert!(
        matches!(&result,Value::String(s) if s==r#"["x",true]"#),
        "{result:?}"
    );
}

#[test]
fn split_reads_a_getter_paired_with_a_preceding_setter() {
    check(
        r#"var log=[],rx={constructor:{[Symbol.species]:function(){return {set lastIndex(x){log.push(x)},get lastIndex(){return {valueOf(){log.push('get');return 2.9}}},exec(){return []}}}}};var result=[RegExp.prototype[Symbol.split].call(rx,'abcd'),log];"#,
        r#"[["","cd"],[0,"get",2,"get",3,"get"]]"#,
    );
}

#[test]
fn replacement_and_indices_preserve_named_groups_and_undefined_captures() {
    check(
        r#"var a=/(?<x>a)(b)?/d.exec('a');var text='ab'.replace(/(?<x>a)(b)/,'$<x>-$2-$$-$&');var result=[text,a.indices,Object.getPrototypeOf(a.groups)===null,Object.getPrototypeOf(a.indices.groups)===null,a.indices.groups.x===a.indices[1]];"#,
        r#"["a-b-$-ab",[[0,1],[0,1],null],true,true,true]"#,
    );
}

#[test]
fn duplicate_named_groups_follow_the_matching_alternative_and_clear_each_iteration() {
    check(
        r#"var r=/(?:(?<x>a)|(?<y>a)(?<x>b))(?:(?<z>c)|(?<z>d))/d;var a=r.exec('abc'),b=r.exec('ad'),c=/(?:(?:(?<x>a)|(?<x>b)|c)\k<x>){2}/.exec('aac');var result=[a.groups,b.groups,Object.keys(a.groups),a.indices.groups.x,c.groups.x,/^(a(b)?)+$/.exec('aba')];"#,
        r#"[{"x":"b","y":"a","z":"c"},{"x":"a","z":"d"},["x","y","z"],[1,2],null,["aba","a",null]]"#,
    );
}

#[test]
fn named_backreferences_resolve_forward_and_decode_identifier_escapes() {
    check(
        r#"var errors=[];try{new RegExp('(?<x>a)(?<x>b)')}catch(e){errors.push(e.name)}var result=[/^\k<x>(?<x>a)$/u.test('a'),/^(?<\u0061>a)\k<a>$/u.test('aa'),/\k<x>+/.test('k<x>>'),errors];"#,
        r#"[true,true,true,["SyntaxError"]]"#,
    );
}

#[test]
fn linear_scans_scale_with_input_while_catastrophic_patterns_stay_bounded() {
    check(
        r#"var s='ab'.repeat(300000);var result=[/\d/.test(s),/\d/u.test(s),/\d/v.test(s),s.length];"#,
        r#"[false,false,false,600000]"#,
    );
    check(
        r#"var hit='';try{/(a+)+$/.test('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab')}catch(e){hit=String(e&&e.message||e)}var result=hit.indexOf('backtracking budget')!==-1;"#,
        r#"true"#,
    );
}
