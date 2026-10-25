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
fn exact_function_and_arrow_text_preserves_comments_and_spelling() {
    assert_both(
        r#"
        function /*between*/ named ( a ) { /*body*/ return a; }
        var arrow = ( x ) /*arrow*/ => x + 1;
        var block = async x => { return x; };
        var gen = function* /*star*/ g(){ yield 1; };
        named.toString() === 'function /*between*/ named ( a ) { /*body*/ return a; }' &&
        arrow.toString() === '( x ) /*arrow*/ => x + 1' &&
        block.toString() === 'async x => { return x; }' &&
        gen.toString() === 'function* /*star*/ g(){ yield 1; }';
    "#,
    );
}

#[test]
fn object_members_keep_original_computed_names_and_accessors() {
    assert_both(
        r#"
        var o = { /*leading*/ [ { a(){} }.a ]( ) { }, get x () { return 1; }, set x (v) { }, async m(){ } };
        var k = Object.keys(o)[0]; var d = Object.getOwnPropertyDescriptor(o, 'x');
        o[k].toString() === '[ { a(){} }.a ]( ) { }' &&
        d.get.toString() === 'get x () { return 1; }' &&
        d.set.toString() === 'set x (v) { }' && o.m.toString() === 'async m(){ }';
    "#,
    );
}

#[test]
fn classes_expose_source_during_static_initialization() {
    assert_both(
        r#"
        var text; class C { static { text = this.toString(); } static /*gap*/ m () {} get x(){ return 1; } }
        text === 'class C { static { text = this.toString(); } static /*gap*/ m () {} get x(){ return 1; } }' &&
        C.m.toString() === 'm () {}' &&
        Object.getOwnPropertyDescriptor(C.prototype,'x').get.toString() === 'get x(){ return 1; }';
    "#,
    );
}

#[test]
fn dynamic_functions_and_eval_preserve_source() {
    assert_both(
        r#"
        var f = Function('a /*p*/', 'return a;');
        var g = eval('(function /*eval*/ x () { })');
        var h = (0,eval)('(x /*p*/ => x)');
        f.toString() === 'function anonymous(a /*p*/\n) {\nreturn a;\n}' &&
        g.toString() === 'function /*eval*/ x () { }' && h.toString() === 'x /*p*/ => x';
    "#,
    );
}

#[test]
fn native_bound_and_proxy_strings_do_not_read_guest_names() {
    assert_both(
        r#"
        function f(){} var b = f.bind(null); var p = new Proxy(f,{});
        Object.defineProperty(f,'name',{get(){throw 1}});
        Object.defineProperty(Array,'name',{get(){throw 2}});
        f.toString() === 'function f(){}' &&
        b.toString() === 'function () { [native code] }' &&
        p.toString() === 'function () { [native code] }' &&
        Array.toString() === 'function Array() { [native code] }';
    "#,
    );
}

#[test]
fn source_offsets_handle_astral_characters_templates_and_surrogates() {
    assert_both(
        r#"
        var prefix = '😀'; var template = `${function /*inner*/ x(){}}`;
        var f = Function('return "\uD800\uFDD0😀";');
        template === 'function /*inner*/ x(){}' &&
        f.toString() === 'function anonymous(\n) {\nreturn "\uD800\uFDD0😀";\n}';
    "#,
    );
}

#[test]
fn nested_functions_defaults_async_parameters_and_private_members_keep_source() {
    assert_both(
        r#"
        async function /*a*/ decl (x=1) { return x; }
        var a = async ( x = 2 ) => x;
        var b = ((function (x=3) { return function /*nested*/ n () { }; }));
        class C { #m () {} expose(){ return this.#m; } }
        decl.toString() === 'async function /*a*/ decl (x=1) { return x; }' &&
        a.toString() === 'async ( x = 2 ) => x' &&
        b.toString() === 'function (x=3) { return function /*nested*/ n () { }; }' &&
        b() .toString() === 'function /*nested*/ n () { }' &&
        new C().expose().toString() === '#m () {}';
    "#,
    );
}

#[test]
fn callable_string_conversion_honors_guest_hooks() {
    assert_both(
        r#"
        function f(){} f[Symbol.toPrimitive]=function(hint){return hint};
        var first=`${f}`;
        delete f[Symbol.toPrimitive]; f.toString=function(){return 'custom'};
        first==='string' && `${f}`==='custom' && String(f)==='custom';
    "#,
    );
}

#[test]
fn native_symbol_getters_keep_display_names_after_descriptor_installation() {
    assert_both(
        r#"
        Object.getOwnPropertyDescriptor(Array,Symbol.species).get.toString() ===
            'function get [Symbol.species]() { [native code] }' &&
        Object.getOwnPropertyDescriptor(RegExp,Symbol.species).get.toString() ===
            'function get [Symbol.species]() { [native code] }';
    "#,
    );
}

#[test]
fn source_preserves_carriage_return_sequences() {
    assert_both("function\r\n/*a*/\rf( )\r{}; f.toString() === 'function\\r\\n/*a*/\\rf( )\\r{}'");
}
