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
fn callbacks_box_receivers_and_snapshot_length() {
    assert_tiers(
        "var out=[];Array.prototype.forEach.call('ab',function(v,i,o){out.push(v,i,typeof o,o.valueOf())});var log='';var obj={get length(){log+='l';return 1},0:3};var bad=false;try{Array.prototype.every.call(obj,null)}catch(e){bad=e instanceof TypeError}var a=[1,2,3];var seen=[];a.forEach(function(v,i){seen.push(v);if(i===0){delete a[1];a.push(4)}});JSON.stringify([out,log,bad,seen]);",
        "[[\"a\",0,\"object\",\"ab\",\"b\",1,\"object\",\"ab\"],\"l\",true,[1,3]]",
    );
    assert_tiers(
        "var names=['every','some','forEach','reduce','reduceRight'];var n=0;for(var i=0;i<names.length;i++){try{Array.prototype[names[i]].call(null,function(){})}catch(e){if(e instanceof TypeError)n++}}JSON.stringify([n,Array.prototype.reduce.call('abc',function(a,b){return a+b}),Array.prototype.reduceRight.call('abc',function(a,b){return a+b}),Array.prototype.reduce.call([],function(){},undefined)===undefined]);",
        "[5,\"abc\",\"cba\",true]",
    );
}
#[test]
fn callbacks_use_logical_lengths_and_inherited_elements() {
    assert_tiers(
        "var o={length:Infinity,0:1};var log=[];var every=Array.prototype.every.call(o,function(v,i){log.push(i);return false});var some=Array.prototype.some.call(o,function(v,i){log.push(i);return true});var last=-1;try{Array.prototype.reduceRight.call({length:Infinity,9007199254740990:7},function(a,v,i){last=i;throw 'stop'},0)}catch(e){}var proto={1:4};var a=Object.create(proto);a.length=3;a[2]=5;JSON.stringify([every,some,log,last,Array.prototype.reduce.call(a,function(a,b){return a+b},0)]);",
        "[false,true,[0,0],9007199254740990,9]",
    );
}
#[test]
fn fill_coerces_before_writing_and_throws_on_rejected_writes() {
    assert_tiers(
        "var log='';var o={length:4};var a={valueOf:function(){log+='s';return -3}};var b={valueOf:function(){log+='e';return -1}};var same=Array.prototype.fill.call(o,7,a,b)===o;var p={length:3};Object.defineProperty(p,'1',{value:0,writable:false});var bad=false;try{Array.prototype.fill.call(p,9)}catch(e){bad=e instanceof TypeError}var high={length:9007199254740991};Array.prototype.fill.call(high,8,-1);JSON.stringify([o,log,same,bad,p[0],p[1],Object.hasOwn(p,2),high[9007199254740990],Object.freeze([]).fill(1).length]);",
        "[{\"1\":7,\"2\":7,\"length\":4},\"se\",true,true,9,0,false,8,0]",
    );
}
#[test]
fn reverse_handles_generic_holes_inheritance_and_partial_failure() {
    assert_tiers(
        "var o={length:4,0:'a',2:'c'};var same=Array.prototype.reverse.call(o)===o;var p={length:3};Object.defineProperty(p,'2',{value:8,configurable:false});var bad=false;try{Array.prototype.reverse.call(p)}catch(e){bad=e instanceof TypeError}var proto={0:'x'};var q=Object.create(proto);q.length=2;Array.prototype.reverse.call(q);JSON.stringify([o,same,bad,p[0],p[2],q[1],Object.hasOwn(q,0),Array.prototype.reverse.call({length:0}).length]);",
        "[{\"1\":\"c\",\"3\":\"a\",\"length\":4},true,true,8,8,\"x\",false,0]",
    );
}
#[test]
fn to_sorted_is_dense_stable_and_does_not_use_species() {
    assert_tiers(
        "var bad=false;try{[2,1].sort(function(){return 1n})}catch(e){bad=e instanceof TypeError}var calls=0;var a=[1,2].sort(function(){return {valueOf:function(){calls++;return 1}}});JSON.stringify([bad,calls,a]);",
        "[true,1,[2,1]]",
    );
    assert_tiers(
        "var a=[3,,undefined,1];Object.defineProperty(a,'constructor',{get:function(){throw Error('species')}});var called=0;var b=a.toSorted(function(x,y){if(x===undefined||y===undefined)throw Error('undefined');called++;return x-y});var x=[{k:1,n:'a'},{k:0,n:'b'},{k:1,n:'c'}].toSorted(function(x,y){return x.k-y.k});JSON.stringify([b,Object.hasOwn(b,2),Object.hasOwn(b,3),a[0],x[0].n+x[1].n+x[2].n,called>0,Array.prototype.toSorted.name,Array.prototype.toSorted.length]);",
        "[[1,3,null,null],true,true,3,\"bac\",true,\"toSorted\",1]",
    );
    assert_tiers(
        "var calls=0;var a=[2,1].toSorted(function(){return {valueOf:function(){calls++;return -1}}});var bad=0;try{[2,1].toSorted(function(){return 1n})}catch(e){if(e instanceof TypeError)bad++}var lengthCalls=0;try{Array.prototype.toSorted.call({get length(){lengthCalls++;return 0}},null)}catch(e){if(e instanceof TypeError)bad++}JSON.stringify([a,calls,bad,lengthCalls,Array.prototype.toSorted.call({length:3,0:'b',2:'a'})]);",
        "[[2,1],1,2,0,[\"a\",\"b\",null]]",
    );
}

#[test]
fn reverse_observes_has_get_delete_and_set_order() {
    assert_tiers(
        "var log='';var target={0:'a',length:2};var p=new Proxy(target,{get:function(t,k){log+='g'+k+';';return t[k]},has:function(t,k){log+='h'+k+';';return k in t},deleteProperty:function(t,k){log+='d'+k+';';return delete t[k]},set:function(t,k,v){log+='s'+k+';';t[k]=v;return true}});Array.prototype.reverse.call(p);JSON.stringify([log,target[1],Object.hasOwn(target,0),['\\uffff','\\ud800'].toSorted()]);",
        "[\"glength;h0;g0;h1;d0;s1;\",\"a\",false,[\"\\ud800\",\"￿\"]]",
    );
}

#[test]
fn generic_callback_iteration_respects_execution_limits() {
    for method in ["every", "some", "forEach", "reduce", "reduceRight"] {
        let source = format!(
            "Array.prototype.{method}.call({{length:Infinity}},function(){{return true}},0);"
        );
        let prepared = Interpreter::compile(&source).unwrap();
        assert_eq!(
            prepared.tier(),
            napi_vm::interpreter::ExecutionTier::Bytecode
        );
        let mut bytecode = Interpreter::with_builtins();
        bytecode.set_loop_budget(3);
        assert!(
            bytecode
                .execute(&prepared)
                .unwrap_err()
                .to_string()
                .contains("loop")
        );
        let statements = Parser::new(Lexer::new(&source).tokenize())
            .parse_program()
            .unwrap();
        let mut ast = Interpreter::with_builtins();
        ast.set_loop_budget(3);
        assert!(
            ast.run_program_body(&statements)
                .unwrap_err()
                .to_string()
                .contains("loop")
        );
    }
}
