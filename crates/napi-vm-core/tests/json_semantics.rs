use napi_vm_core::{Interpreter, Value};

fn truth(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source), Ok(Value::Bool(true))),
        "{source}"
    );
}

#[test]
fn stringify_invokes_to_json_then_replacer_with_keys_and_holders() {
    truth(
        r#"
        var log=[]; var input={a:{toJSON(key){log.push('json:'+key);return 2;}},b:3};
        var text=JSON.stringify(input,function(key,value){
            log.push('replace:'+key);
            if(key==='a' && this!==input)throw new Error('holder');
            return key==='b'?undefined:value;
        });
        text==='{"a":2}' && log.join('|')==='replace:|json:a|replace:a|replace:b';
    "#,
    );
}

#[test]
fn stringify_replacer_runs_for_undefined_and_bigint_roots() {
    truth(
        r#"
        JSON.stringify(undefined,function(key,value){return key===''?42:value;})==='42' &&
        JSON.stringify(1n,function(key,value){return typeof value==='bigint'?Number(value):value;})==='1' &&
        JSON.stringify(Symbol())===undefined && JSON.stringify(function(){})===undefined;
    "#,
    );
}

#[test]
fn stringify_property_lists_keep_order_and_read_inherited_properties() {
    truth(
        r#"
        var base={inherited:4};var value=Object.create(base);value.a=1;value.b=2;
        var keys=['b',new String('a'),'b','inherited',{},Symbol()];
        JSON.stringify(value,keys)==='{"b":2,"a":1,"inherited":4}' &&
        JSON.stringify([1,undefined,function(){},Symbol()],[])==='[1,null,null,null]';
    "#,
    );
}

#[test]
fn stringify_observes_proxy_operations_without_unwrapping_the_target() {
    truth(
        r#"
        var log=[];var p=new Proxy({a:1,b:2},{
            get(target,key,receiver){log.push('get:'+String(key));return Reflect.get(target,key,receiver);},
            ownKeys(){log.push('keys');return ['b','a'];},
            getOwnPropertyDescriptor(target,key){log.push('desc:'+key);return Reflect.getOwnPropertyDescriptor(target,key);}
        });
        JSON.stringify(p)==='{"b":2,"a":1}' &&
        log.join('|')==='get:toJSON|keys|desc:b|desc:a|get:b|get:a';
    "#,
    );
}

#[test]
fn stringify_snapshots_object_keys_but_reads_array_holes_and_live_values() {
    truth(
        r#"
        var value={get a(){delete this.b;this.c=3;return 1;},b:2};
        var array=[1,2,3];delete array[1];Object.setPrototypeOf(array,{1:2});
        JSON.stringify(value)==='{"a":1}' && JSON.stringify(array)==='[1,2,3]';
    "#,
    );
}

#[test]
fn stringify_handles_boxed_values_spacing_and_utf16_keys() {
    truth(
        r#"
        var n=new Number(7);n.valueOf=function(){return 8;};
        var s=new String('x');s.toString=function(){return 'y';};
        var value={n:n,s:s,b:new Boolean(false),small:1e-7,zero:-0};
        JSON.stringify(value)==='{"n":8,"s":"y","b":false,"small":1e-7,"zero":0}' &&
        JSON.stringify({a:1},null,new String(' '))==='{\n "a": 1\n}' &&
        JSON.stringify({'\uD800':1},['\uD800'])==='{"\\ud800":1}';
    "#,
    );
}

#[test]
fn stringify_preserves_exotic_own_properties_and_detects_cycles() {
    truth(
        r#"
        var a=new Uint8Array([1,2]);a.extra=3;
        var shared={x:1};var cycle={};cycle.self=cycle;var threw=false;
        try{JSON.stringify(cycle);}catch(e){threw=e instanceof TypeError;}
        JSON.stringify(a)==='{"0":1,"1":2,"extra":3}' &&
        JSON.stringify([shared,shared])==='[{"x":1},{"x":1}]' && threw;
    "#,
    );
}

#[test]
fn borrowed_stringify_retains_error_and_root_holder_realms_after_gc() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    vm.set_global_checked("stringify", child.eval_source("JSON.stringify").unwrap())
        .unwrap();
    drop(child);
    assert!(vm.collect_cycles().skipped.is_none());
    let source = r#"
        var objectPrototype=other.Object.prototype;
        other.Object=function replaced(){};var correct=false;
        stringify(1,function(key,value){if(key==='')correct=Object.getPrototypeOf(this)===objectPrototype;return value;});
        var revocable=Proxy.revocable([],{});revocable.revoke();var failures=0;
        try{stringify({},revocable.proxy);}catch(e){if(e instanceof other.TypeError)failures++;}
        try{stringify(revocable.proxy);}catch(e){if(e instanceof other.TypeError)failures++;}
        try{stringify(1n);}catch(e){if(e instanceof other.TypeError)failures++;}
        correct && failures===3;
    "#;
    assert!(matches!(vm.eval_source(source), Ok(Value::Bool(true))));
}

#[test]
fn stringify_keeps_the_depth_limit_as_a_catchable_error() {
    truth(
        r#"
        var root={};var cursor=root;
        for(var i=0;i<514;i++){cursor.next={};cursor=cursor.next;}
        var bounded=false;try{JSON.stringify(root);}catch(e){bounded=e instanceof RangeError;}
        bounded;
    "#,
    );
}

#[test]
fn stringify_array_replacer_and_space_coercion_use_observable_operations() {
    truth(
        r#"
        var log=[];var keys=new Proxy(['b','a','b'],{
            get(t,k,r){log.push(String(k));return Reflect.get(t,k,r);}
        });
        var space=new String(' ');space.toString=function(){log.push('space');return ' ';};
        var text=JSON.stringify({a:1,b:2},keys,space);
        text==='{\n "b": 2,\n "a": 1\n}' && log.join('|')==='length|0|1|2|space';
    "#,
    );
}
