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
fn typed_set_uses_array_like_gets_and_observable_offset() {
    assert_tiers(
        r#"var a=new Int8Array(3),log=[];var bag={get length(){log.push('length');return 3},get 0(){log.push('0');return 1},get 1(){log.push('1');return {valueOf:function(){log.push('number');return 258}}},get 2(){log.push('2');return a[1]+1}};Object.defineProperty(bag,Symbol.iterator,{get:function(){throw new Error('iterator')}});a.set(bag,{valueOf:function(){log.push('offset');return 0}});JSON.stringify([Array.from(a),log]);"#,
        r#"[[1,2,3],["offset","length","0","1","number","2"]]"#,
    );
}

#[test]
fn typed_sort_is_numeric_stable_and_copying_methods_ignore_species() {
    assert_tiers(
        r#"var a=new Float64Array([10,2,NaN,0,-0,1]);var sorted=a.toSorted();var reversed=a.toReversed();var replaced=a.with(-1,{valueOf:function(){return 30}});var stable=new Int16Array([21,11,22,12]);var same=stable.sort(function(a,b){return a%10-b%10})===stable;var b=new BigInt64Array([10n,2n,-1n]);b.sort();JSON.stringify([Array.from(sorted,String),Object.is(sorted[0],-0),Object.is(sorted[1],0),Array.from(a,String),reversed[0],replaced[5],same,Array.from(stable),Array.from(b,String)]);"#,
        r#"[["0","0","1","2","10","NaN"],true,true,["10","2","NaN","0","0","1"],1,30,true,[21,11,22,12],["-1","2","10"]]"#,
    );
}

#[test]
fn html_string_methods_escape_quotes_and_preserve_utf16() {
    assert_tiers(
        r#"var log=[];var result=String.prototype.link.call({toString:function(){log.push('receiver');return '\uD800&<'}},{toString:function(){log.push('argument');return '"&<'}});JSON.stringify([result,log,'x'.bold(),'x'.fontcolor(undefined),String.prototype.trimLeft===String.prototype.trimStart,String.prototype.trimRight===String.prototype.trimEnd,String.prototype.anchor.length,String.prototype.bold.length]);"#,
        r#"["<a href=\"&quot;&<\">\ud800&<</a>",["receiver","argument"],"<b>x</b>","<font color=\"undefined\">x</font>",true,true,1,0]"#,
    );
}

#[test]
fn temporal_now_uses_the_host_clock_and_requested_time_zone() {
    assert_tiers(
        r#"var before=Date.now();var instant=Temporal.Now.instant();var after=Date.now();var zoned=Temporal.Now.zonedDateTimeISO('+05:30');var date=Temporal.Now.plainDateISO('UTC');var time=Temporal.Now.plainTimeISO('UTC');var datetime=Temporal.Now.plainDateTimeISO('UTC');JSON.stringify([instant instanceof Temporal.Instant,instant.epochMilliseconds>=before&&instant.epochMilliseconds<=after,zoned.timeZoneId,zoned.offset,zoned.calendarId,date instanceof Temporal.PlainDate,time instanceof Temporal.PlainTime,datetime instanceof Temporal.PlainDateTime,typeof Temporal.Now.timeZoneId(),Object.prototype.toString.call(Temporal.Now)]);"#,
        r#"[true,true,"+05:30","+05:30","iso8601",true,true,true,"string","[object Temporal.Now]"]"#,
    );
}

#[test]
fn set_composition_observes_records_and_closes_early_iterators() {
    assert_tiers(
        r#"var log=[],s=new Set([1,2,3]);var other={get size(){log.push('size');return 1},get has(){log.push('has');return function(v){return v===2}},get keys(){log.push('keys');return function(){var n=0;return {next:function(){return {done:n++>0,value:2}},return:function(){log.push('close');return {}}}}}};var disjoint=s.isDisjointFrom(other);var intersection=s.intersection(other);var union=s.union(new Set([3,4]));var difference=s.difference(new Set([2]));var symmetric=s.symmetricDifference(new Set([2,4]));JSON.stringify([disjoint,Array.from(intersection),Array.from(union),Array.from(difference),Array.from(symmetric),log]);"#,
        r#"[false,[2],[1,2,3,4],[1,3],[1,3,4],["size","has","keys","close","size","has","keys"]]"#,
    );
}

#[test]
fn collection_foreach_tracks_deletion_clear_and_appended_entries() {
    assert_tiers(
        r#"var s=new Set([1,2,3]),seen=[];s.forEach(function(v){seen.push(v);if(v===1){s.delete(2);s.add(4)}if(v===3){s.clear();s.add(5)}});var m=new Map([[1,'a'],[2,'b']]),keys=[];m.forEach(function(v,k){keys.push(k);if(k===1){m.delete(2);m.set(3,'c')}});JSON.stringify([seen,Array.from(s),s.size,keys,Array.from(m)]);"#,
        r#"[[1,3,5],[5],1,[1,3],[[1,"a"],[3,"c"]]]"#,
    );
}

#[test]
fn disposable_stack_moves_records_and_nests_errors_in_disposal_order() {
    assert_tiers(
        r#"var log=[],s=new DisposableStack(),one={},two={};s.adopt(7,function(v){log.push(v);throw one});s.defer(function(){log.push('last');throw two});var moved=s.move(),caught;try{moved.dispose()}catch(e){caught=e}moved.dispose();var rejected=false;try{s.defer(function(){})}catch(e){rejected=e instanceof ReferenceError}JSON.stringify([s.disposed,moved.disposed,log,caught instanceof SuppressedError,caught.error===one,caught.suppressed===two,rejected,DisposableStack.prototype[Symbol.dispose]===DisposableStack.prototype.dispose]);"#,
        r#"[true,true,["last",7],true,true,true,true,true]"#,
    );
}

#[test]
fn groupby_preserves_key_identity_and_legacy_escape_preserves_code_units() {
    assert_tiers(
        r#"var key={},m=Map.groupBy([1,2,3],function(v,i){return i===1?key:NaN}),o=Object.groupBy(['a','b'],function(){return '__proto__'});var closed=false,it={};it[Symbol.iterator]=function(){return {next:function(){return {done:false,value:1}},return:function(){closed=true;return {}}}};var reason={};var caught;try{Map.groupBy(it,function(){throw reason})}catch(e){caught=e}JSON.stringify([m.get(NaN),m.get(key),Object.getPrototypeOf(o)===null,o.__proto__,closed,caught===reason,escape('\uD800é @'),unescape('%uD800%E9%20@')==='\uD800é @','abc'.substr(-2,1)]);"#,
        r#"[[1,3],[2],true,["a","b"],true,true,"%uD800%E9%20@",true,"b"]"#,
    );
}

#[test]
fn annex_b_accessors_route_through_proxy_traps_and_abrupt_keys() {
    assert_tiers(
        r#"var sentinel={},caught=[],root=Object.defineProperty({},'target',{get:function(){}});var throwing=function(){throw sentinel};var lookup=new Proxy(root,{getOwnPropertyDescriptor:throwing});try{lookup.__lookupGetter__('target')}catch(e){caught.push(e===sentinel)}try{lookup.__lookupSetter__('target')}catch(e){caught.push(e===sentinel)}var protoSubject=new Proxy(root,{getPrototypeOf:function(){throw sentinel}});try{protoSubject.__lookupGetter__('missing')}catch(e){caught.push(e===sentinel)}var coerced=0,badKey={toString:function(){coerced++;throw sentinel}};try{({}).__lookupGetter__(badKey)}catch(e){caught.push(e===sentinel)}var fixed=function(){};var holder={};holder.__defineGetter__('x',fixed);holder.__defineSetter__('x',fixed);var seen=holder.__lookupGetter__('x')===fixed&&holder.__lookupSetter__('x')===fixed;var defineTrap=new Proxy({},{defineProperty:throwing});try{defineTrap.__defineGetter__('attr',function(){})}catch(e){caught.push(e===sentinel)}JSON.stringify([caught,coerced,seen]);"#,
        r#"[[true,true,true,true,true],1,true]"#,
    );
}

#[test]
fn object_to_string_uses_tags_arguments_and_proxy_brands() {
    assert_tiers(
        r#"var t=Object.prototype.toString,log=[];log.push(t.call(function(){return arguments}()));log.push(t.call(new Proxy([],{})));log.push(t.call(new Proxy(function(){},{})));var m=new Map(),it=m[Symbol.iterator]();log.push(t.call(it));var p=new Promise(function(){});log.push(t.call(p));delete Promise.prototype[Symbol.toStringTag];log.push(t.call(p));log.push(t.call(Object(1)));log.push(t.call(new Number(1)));Number.prototype[Symbol.toStringTag]='test262';log.push(t.call(0));delete Number.prototype[Symbol.toStringTag];var s=new Set(),sit=s[Symbol.iterator]();log.push(t.call(sit));JSON.stringify(log);"#,
        r#"["[object Arguments]","[object Array]","[object Function]","[object Map Iterator]","[object Promise]","[object Object]","[object Number]","[object Number]","[object test262]","[object Set Iterator]"]"#,
    );
}
