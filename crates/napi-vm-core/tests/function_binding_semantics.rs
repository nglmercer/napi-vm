use napi_vm_core::{Interpreter, Value, parser::parse_cached};

fn assert_both(source: &str) {
    let prepared = Interpreter::with_builtins().eval_source(source);
    assert!(
        matches!(prepared, Ok(Value::Bool(true))),
        "prepared: {prepared:?}\n{source}"
    );
    let ast = Interpreter::with_builtins().run_program_body(&parse_cached(source).unwrap());
    assert!(
        matches!(ast, Ok(Value::Bool(true))),
        "AST: {ast:?}\n{source}"
    );
}

#[test]
fn default_parameters_infer_names_only_for_anonymous_definitions() {
    assert_both(
        r#"
        function f(a=function(){},b=()=>{},c=class {},d=function named(){}, e=(function(){})){
            return a.name==='a'&&b.name==='b'&&c.name==='c'&&d.name==='named'&&e.name==='e';
        }
        function g(a=class {static nameSeen=this.name}){return a.nameSeen==='a'}
        f()&&g();
    "#,
    );
}

#[test]
fn annex_b_for_in_initializers_have_inferred_names_even_without_iterations() {
    assert_both(
        r#"
        for(var f = function(){} in {}){}
        for(var a = ()=>{} in {}){}
        for(var c = class {} in {}){}
        for(var named = function n(){} in {}){}
        f.name==='f'&&a.name==='a'&&c.name==='c'&&named.name==='n';
    "#,
    );
}

#[test]
fn computed_fields_after_block_arrows_remain_separate_fields() {
    assert_both(
        r#"
        class C {
            ['first'] = ()=>{}
            ['second'] = async ()=>{}
            [3] = ()=>{}
            ['fourth'] = async ()=>{};
        }
        var c=new C();
        c.first.name==='first'&&c.second.name==='second'&&c[3].name==='3'&&c.fourth.name==='fourth'&&
        c.first.toString()==='()=>{}'&&c.second.toString()==='async ()=>{}';
    "#,
    );
}

#[test]
fn parenthesized_arrow_values_can_still_be_called_and_indexed() {
    assert_both(
        r#"
        var f=()=>42;
        (()=>42)()===42 && (()=>{}).length===0 && (()=>{})['name']==='';
    "#,
    );
}

#[test]
fn deleting_inherited_function_properties_does_not_resurrect_defaults() {
    assert_both(
        r#"
        function f(a){} delete f.length; delete f.name;
        var inherited=f.length===0&&f.name==='';
        delete Function.prototype.length; delete Function.prototype.name;
        var absent=f.length===undefined&&f.name===undefined&&
            !('length' in f)&&!('name' in f);
        Function.prototype.length=7;
        inherited&&absent&&f.length===7;
    "#,
    );
}

#[test]
fn parenthesized_logical_assignment_targets_do_not_infer_names() {
    assert_both(
        r#"
        var a,b=false,c=true,d;
        (a) ??= function(){}; (b) ||= ()=>{}; (c) &&= class {};
        d ??= ()=>{};
        a.name===''&&b.name===''&&c.name===''&&d.name==='d';
    "#,
    );
}

#[test]
fn method_modifiers_cannot_introduce_data_properties() {
    assert_both(
        r#"
        var bad=['({async async:0})','({async ["x"]:0})','({*x:0})','({get x:0})','({set x:0})'];
        var rejected=bad.every(code=>{
            try{Function(code);return false}catch(e){if(!(e instanceof SyntaxError))return false}
            try{eval(code);return false}catch(e){return e instanceof SyntaxError}
        });
        var good={async:1,get:2,set:3,async m(){},get x(){return 4}};
        rejected&&good.async===1&&good.get===2&&good.set===3&&good.x===4;
    "#,
    );
}

#[test]
fn exponentiation_requires_parentheses_around_await_on_the_left() {
    assert_both(
        r#"
        var A=(async function(){}).constructor;
        var bad=false;try{A('return await a ** 0')}catch(e){bad=e instanceof SyntaxError}
        var f=A('a','return (await a) ** 0');
        var g=A('a','return a ** await 0');
        bad&&typeof f==='function'&&typeof g==='function';
    "#,
    );
}

#[test]
fn host_callbacks_also_respect_deleted_function_prototype_properties() {
    let source = "delete host.length; delete host.name; delete Function.prototype.length; delete Function.prototype.name; delete Function.prototype.call; host.length===undefined&&host.name===undefined&&host.call===undefined;";
    for ast in [false, true] {
        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .set_global_checked("host", Value::host_function("host", 123))
            .unwrap();
        let result = if ast {
            interpreter.run_program_body(&parse_cached(source).unwrap())
        } else {
            interpreter.eval_source(source)
        };
        assert!(
            matches!(result, Ok(Value::Bool(true))),
            "AST={ast}: {result:?}"
        );
    }
}

#[test]
fn primitive_references_preserve_receivers_through_proxy_prototypes() {
    assert_both(
        r#"
        var key=Symbol('key'),ok=true;
        for (var value of [12,false,'text',Symbol('value'),1n]) {
            let current=value,prototype=Object.getPrototypeOf(value);
            var proxy=new Proxy({get nested(){'use strict';return this===current}}, {
                get(target,property,receiver){
                    if(property==='probe'||property===key)return receiver===current;
                    return Reflect.get(target,property,receiver);
                }
            });
            Object.setPrototypeOf(prototype,proxy);
            ok=ok&&value.probe&&value['probe']&&value[key]&&value.nested;
        }
        ok&&'text'.length===4&&'text'[0]==='t';
    "#,
    );
}
