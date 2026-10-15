use napi_vm::{Interpreter, Value};
fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    vm.collect_cycles();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result,Value::String(s) if s==expected),
        "{result:?}"
    );
}
#[test]
fn grapheme_boundaries_preserve_utf16_surrogates_and_containing_indices() {
    check(
        r#"var s=new Intl.Segmenter('en-US'),text='A\u0301😀\ud800B',segments=s.segment(text),a=[...segments].map(function(p){return [p.segment,p.index]});var result=[a,segments.containing(3).index,segments.containing(-1),segments.containing(99),s.resolvedOptions()];"#,
        r#"[[["Á",0],["😀",2],["\ud800",4],["B",5]],2,null,null,{"locale":"en-US","granularity":"grapheme"}]"#,
    );
}
#[test]
fn segment_iterators_are_independent_and_survive_collection() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var segments=new Intl.Segmenter('en',{granularity:'word'}).segment('Hi 42!'),a=segments[Symbol.iterator](),b=segments[Symbol.iterator]();a.next();segments=null;").unwrap();
    vm.collect_cycles();
    let result=vm.eval_source("JSON.stringify([a.next().value,b.next().value,[...a].map(function(x){return [x.segment,x.isWordLike]})])").unwrap();
    assert!(
        matches!(&result,Value::String(s) if s==r#"[{"segment":" ","index":2,"input":"Hi 42!","isWordLike":false},{"segment":"Hi","index":0,"input":"Hi 42!","isWordLike":true},[["42",true],["!",false]]]"#),
        "{result:?}"
    );
}
#[test]
fn list_patterns_use_context_and_parts_preserve_original_utf16() {
    check(
        r#"var f=new Intl.ListFormat('es',{type:'conjunction'}),a=f.format(['España','Italia']);var parts=new Intl.ListFormat('en-US').formatToParts(['','\ud800','😀']);var result=[a,parts,new Intl.ListFormat('en-US',{type:'disjunction'}).format(['a','b','c'])];"#,
        r#"["España e Italia",[{"type":"element","value":""},{"type":"literal","value":", "},{"type":"element","value":"\ud800"},{"type":"literal","value":", and "},{"type":"element","value":"😀"}],"a, b, or c"]"#,
    );
}
#[test]
fn list_iteration_closes_on_non_string_and_brand_checks_precede_coercion() {
    check(
        r#"var log=[],items={[Symbol.iterator](){return {next(){return {done:false,value:1}},return(){log.push('closed');throw 'cleanup'}}}};try{new Intl.ListFormat().format(items)}catch(e){log.push(e.name)}try{Intl.ListFormat.prototype.format.call({},items)}catch(e){log.push(e.name)}try{Intl.Segmenter.prototype.segment.call({}, {toString(){log.push('coerced')}})}catch(e){log.push(e.name)}var result=log;"#,
        r#"["closed","TypeError","TypeError","TypeError"]"#,
    );
}
#[test]
fn regexp_compile_resets_the_observable_last_index() {
    check(
        "var r=/a/g;r.lastIndex=23;r.compile(/b/);var result=[r.lastIndex,r.test('b')];",
        "[0,true]",
    );
}
