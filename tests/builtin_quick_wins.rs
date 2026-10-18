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
fn builtin_function_metadata_and_attributes() {
    assert_tiers(
        "var groups=[[Number,['isNaN','isFinite','isInteger','isSafeInteger','parseInt','parseFloat'],[1,1,1,1,2,1]],[Number.prototype,['toString','valueOf','toFixed','toPrecision'],[1,0,1,1]],[Array,['isArray','from','of'],[1,1,0]],[Array.prototype,['at','with','includes','findLast','findLastIndex','slice','splice'],[1,2,1,1,1,2,2]],[String,['raw','fromCharCode','fromCodePoint'],[1,1,1]],[String.prototype,['at','padStart','padEnd','isWellFormed','toWellFormed','split','slice'],[1,1,1,0,0,2,2]],[JSON,['parse','stringify'],[2,3]]];var ok=true;for(var g=0;g<groups.length;g++){var target=groups[g][0];for(var i=0;i<groups[g][1].length;i++){var name=groups[g][1][i];var f=target[name];var d=Object.getOwnPropertyDescriptor(target,name);var l=Object.getOwnPropertyDescriptor(f,'length');var n=Object.getOwnPropertyDescriptor(f,'name');ok=ok&&f.name===name&&f.length===groups[g][2][i]&&d.writable&&!d.enumerable&&d.configurable&&!l.writable&&!l.enumerable&&l.configurable&&!n.writable&&!n.enumerable&&n.configurable&&Object.getPrototypeOf(f)===Function.prototype;}}JSON.stringify([ok,Number.parseInt===parseInt,Number.parseFloat===parseFloat,Array.prototype.values===Array.prototype[Symbol.iterator]]);",
        "[true,true,true,true]",
    );
}

#[test]
fn object_is_prototype_of_returns_false_for_primitive_arguments_before_receiver_coercion() {
    assert_tiers(
        "var falseResults=[];var throws=[];for(var value of [null,undefined]){falseResults.push(Object.prototype.isPrototypeOf.call(value,1));try{Object.prototype.isPrototypeOf.call(value,{}) ;throws.push(false)}catch(error){throws.push(error instanceof TypeError)}}JSON.stringify([falseResults,throws]);",
        "[[false,false],[true,true]]",
    );
}

#[test]
fn for_in_boxes_primitive_string_iterables() {
    // This loop shape uses the AST fallback, so assert its semantics directly
    // instead of requiring the bytecode tier used by most tests in this file.
    let source = "var key;for(key in '9'){}JSON.stringify(key);";
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == "\"0\""),
        "AST: {result:?}"
    );
}

#[test]
fn shadow_realm_evaluates_and_wraps_callables_at_the_realm_boundary() {
    assert_tiers(
        "var realm=new ShadowRealm();var twice=realm.evaluate('(value)=>value*2');var bad=false;try{twice({})}catch(error){bad=error instanceof TypeError}var thrown=false;try{realm.evaluate('()=>{throw 1}')()}catch(error){thrown=error instanceof TypeError}var objectResult=false;try{realm.evaluate('globalThis')}catch(error){objectResult=error instanceof TypeError}JSON.stringify([realm.evaluate('40+2'),twice(21),bad,thrown,objectResult,ShadowRealm.prototype.evaluate.length,ShadowRealm.prototype.importValue.length,Object.prototype.toString.call(realm)]);",
        "[42,42,true,true,true,1,2,\"[object ShadowRealm]\"]",
    );
}

#[test]
fn number_constructor_coerces_objects_and_accepts_bigints() {
    assert_tiers(
        "var log='';var obj={[Symbol.toPrimitive]:function(hint){log+=hint;return '12'}};var a=Number(obj);var b=new Number(obj);var bad=false;try{Number(Symbol())}catch(e){bad=e instanceof TypeError}JSON.stringify([a,b.valueOf(),log,Number(3n),Number(Object(4n)),bad,Number(),Number.isNaN(Number(undefined))]);",
        "[12,12,\"numbernumber\",3,4,true,0,true]",
    );
}

#[test]
fn number_methods_validate_receivers_before_arguments() {
    assert_tiers(
        "var names=['toString','valueOf','toFixed','toPrecision'];var bad=0;var calls=0;var arg={valueOf:function(){calls++;return 2}};for(var i=0;i<names.length;i++){try{Number.prototype[names[i]].call({},arg)}catch(e){if(e instanceof TypeError)bad++}}JSON.stringify([bad,calls,Number.prototype.valueOf.call(new Number(7))]);",
        "[4,0,7]",
    );
}

#[test]
fn number_formatting_handles_defaults_ranges_and_argument_coercion() {
    assert_tiers(
        "var calls=0;var radix={valueOf:function(){calls++;return 2.9}};var bad=0;for(var i=0;i<4;i++){try{(1).toFixed([-1,101,Infinity,-Infinity][i])}catch(e){if(e instanceof RangeError)bad++}}JSON.stringify([(5).toString(undefined),(5).toString(radix),(2.5).toFixed(undefined),(2.5).toFixed(2.9),(-0).toFixed(1),(Infinity).toFixed(2),(-Infinity).toPrecision(0),(1).toPrecision(undefined),calls,bad]);",
        "[\"5\",\"101\",\"3\",\"2.50\",\"0.0\",\"Infinity\",\"-Infinity\",\"1\",1,4]",
    );
}

#[test]
fn numeric_parsers_use_string_hooks_and_ecmascript_whitespace() {
    assert_tiers(
        "var log='';var x={[Symbol.toPrimitive]:function(hint){log+=hint;return '\\ufeff 12.5x'}};JSON.stringify([parseInt(x),parseFloat(x),log,Number.isNaN(parseInt('\\u00851')),Number.isNaN(parseFloat('\\u00851')),Object.is(parseInt('-0'),-0)]);",
        "[12,12.5,\"stringstring\",true,true,true]",
    );
}

#[test]
fn string_wellformedness_replaces_only_lone_surrogates() {
    assert_tiers(
        "var values=['','abc','\\ud800','\\udc00','\\ud800\\udc00','a\\ud800b\\udc00c','\\ud800\\ud800\\udc00'];var result=[];for(var i=0;i<values.length;i++){result.push(values[i].isWellFormed(),values[i].toWellFormed())}JSON.stringify(result);",
        "[true,\"\",true,\"abc\",false,\"�\",false,\"�\",true,\"𐀀\",false,\"a�b�c\",false,\"�𐀀\"]",
    );
}

#[test]
fn string_receiver_coercion_precedes_length_and_filler() {
    assert_tiers(
        "var log='';var receiver={toString:function(){log+='s';return 'abc'}};var length={valueOf:function(){log+='l';return 5}};var filler={toString:function(){log+='f';return 'xy'}};var a=String.prototype.padStart.call(receiver,length,filler);var b=String.prototype.padEnd.call(receiver,1,filler);JSON.stringify([a,b,log,String.prototype.isWellFormed.call({toString:function(){return '\\ud800'}})]);",
        "[\"xyabc\",\"abc\",\"slfs\",false]",
    );
}

#[test]
fn string_methods_propagate_receiver_errors() {
    assert_tiers(
        "var bad=0;var methods=['isWellFormed','toWellFormed','padStart','padEnd'];for(var i=0;i<methods.length;i++){try{String.prototype[methods[i]].call(null,4)}catch(e){if(e instanceof TypeError)bad++}try{String.prototype[methods[i]].call(Symbol(),4)}catch(e){if(e instanceof TypeError)bad++}}JSON.stringify(bad);",
        "8",
    );
}

#[test]
fn array_at_is_generic_and_coerces_in_order() {
    assert_tiers(
        "var log='';var obj={get length(){log+='l';return 3},get 1(){log+='g';return 7}};var index={valueOf:function(){log+='i';return -2}};var result=Array.prototype.at.call(obj,index);var a=[,];Array.prototype[0]=9;var hole=a.at(0);delete Array.prototype[0];var bad=false;try{a.at(Symbol())}catch(e){bad=e instanceof TypeError}JSON.stringify([result,log,hole,bad,Array.prototype.at.call('abc',-1)]);",
        "[7,\"lig\",9,true,\"c\"]",
    );
}

#[test]
fn array_with_copies_holes_without_getting_replaced_index_or_species() {
    assert_tiers(
        "var arr=[,2,3];Object.defineProperty(arr,'1',{get:function(){throw 7}});Object.defineProperty(arr,'constructor',{get:function(){throw 8}});var result=arr.with(-2,9);var bad=0;try{[].with(0,1)}catch(e){if(e instanceof RangeError)bad++}try{arr.with(Infinity,1)}catch(e){if(e instanceof RangeError)bad++}JSON.stringify([result,0 in result,arr.length,bad,[1].with(-0.5,2)]);",
        "[[null,9,3],true,3,2,[2]]",
    );
}

#[test]
fn array_reads_use_safe_integer_lengths_without_allocating() {
    assert_tiers(
        "var obj={length:9007199254740991,9007199254740990:'last'};var index=Array.prototype.findLastIndex.call(obj,function(v,i){return true});JSON.stringify([Array.prototype.at.call(obj,-1),Array.prototype.includes.call(obj,'last',9007199254740990),Array.prototype.findLast.call(obj,function(){return true}),index]);",
        "[\"last\",true,\"last\",9007199254740990]",
    );
}

#[test]
fn property_is_enumerable_coerces_symbol_keys_before_receivers() {
    assert_tiers(
        "var symbol=Symbol();var obj={[symbol]:1};var log='';var key={[Symbol.toPrimitive]:function(hint){log+=hint;return symbol}};var result=obj.propertyIsEnumerable(key);try{Object.prototype.propertyIsEnumerable.call(null,key)}catch(e){log+=e instanceof TypeError}JSON.stringify([result,log]);",
        "[true,\"stringstringtrue\"]",
    );
}

#[test]
fn json_parse_uses_tostring_and_reports_syntax_errors() {
    assert_tiers(
        "var log='';var x={toString:function(){log+='s';return '{\"x\":1}'}};var value=JSON.parse(x);var bad=0;var inputs=['','01','-01','1.','1e','1e+','[1,]','{\"a\":}','undefined'];for(var i=0;i<inputs.length;i++){try{JSON.parse(inputs[i])}catch(e){if(e instanceof SyntaxError)bad++}}var symbol=false;try{JSON.parse(Symbol())}catch(e){symbol=e instanceof TypeError}JSON.stringify([value,log,JSON.parse(true),JSON.parse(null),JSON.parse(123),bad,symbol]);",
        "[{\"x\":1},\"s\",true,null,123,9,true]",
    );
}

#[test]
fn json_tag_and_numeric_grammar_preserve_special_values() {
    assert_tiers(
        "var d=Object.getOwnPropertyDescriptor(JSON,Symbol.toStringTag);JSON.stringify([Object.prototype.toString.call(JSON),d.writable,d.enumerable,d.configurable,Object.is(JSON.parse('-0'),-0),JSON.parse('1e400')===Infinity,JSON.parse('1.25e2'),JSON.parse('1e-400')]);",
        "[\"[object JSON]\",false,false,true,true,true,125,0]",
    );
}

#[test]
fn collection_species_getters_and_receiver_validation() {
    assert_tiers(
        "var names=[Map,Set];var bad=0;var ok=true;for(var i=0;i<names.length;i++){var C=names[i];var d=Object.getOwnPropertyDescriptor(C,Symbol.species);ok=ok&&d.get.name==='get [Symbol.species]'&&d.get.length===0&&!d.enumerable&&d.configurable&&d.get.call(3)===3&&C[Symbol.species]===C;try{new C().forEach(0)}catch(e){if(e instanceof TypeError)bad++}}var size=Object.getOwnPropertyDescriptor(Map.prototype,'size').get;try{size.call(new Set())}catch(e){if(e instanceof TypeError)bad++}try{size.call(new WeakMap())}catch(e){if(e instanceof TypeError)bad++}JSON.stringify([ok,bad,size.call(new Map([[1,2]]))]);",
        "[true,4,1]",
    );
}

#[test]
fn date_metadata_receiver_checks_and_default_primitive_hint() {
    assert_tiers(
        "var log='';var value={[Symbol.toPrimitive]:function(hint){log+=hint;return '1970-01-01T00:00:00.000Z'}};var d=new Date(value);var bad=false;try{Date.prototype.getTimezoneOffset.call({})}catch(e){bad=e instanceof TypeError}var tag=Object.getOwnPropertyDescriptor(Date.prototype,Symbol.toPrimitive);JSON.stringify([d.getTime(),log,Date.name,Date.length,tag.writable,Number.isNaN(new Date(NaN).getTimezoneOffset()),bad]);",
        "[0,\"default\",\"Date\",7,false,true,true]",
    );
}

#[test]
fn date_formats_and_own_output_roundtrips() {
    assert_tiers(
        "var d=new Date(0);var negative=new Date(-62198755200000);JSON.stringify([d.toDateString(),d.toTimeString(),d.toUTCString(),Date.parse(d.toString()),Date.parse(d.toUTCString()),new Date(NaN).toDateString(),negative.toDateString(),new Date('-000001-01-01T00:00:00.000Z').getUTCFullYear(),Date.prototype.toGMTString===Date.prototype.toUTCString]);",
        "[\"Thu Jan 01 1970\",\"00:00:00 GMT+0000 (Coordinated Universal Time)\",\"Thu, 01 Jan 1970 00:00:00 GMT\",0,0,\"Invalid Date\",\"Fri Jan 01 -0001\",-1,true]",
    );
}

#[test]
fn reflect_metadata_and_get_prototype_validate_primitive_targets() {
    assert_tiers(
        "var bad=0;var values=[undefined,null,1,'a',true,1n,Symbol()];for(var i=0;i<values.length;i++){try{Reflect.getPrototypeOf(values[i])}catch(e){if(e instanceof TypeError)bad++}}var d=Object.getOwnPropertyDescriptor(Reflect,Symbol.toStringTag);JSON.stringify([bad,Reflect.get.length,Object.prototype.toString.call(Reflect),d.writable,d.enumerable,d.configurable,Reflect.getPrototypeOf({})===Object.prototype]);",
        "[7,2,\"[object Reflect]\",false,false,true,true]",
    );
}

#[test]
fn tofixed_rounds_the_exact_binary_value_including_ties() {
    assert_tiers(
        "JSON.stringify([(2.5).toFixed(0),(-2.5).toFixed(0),(1.25).toFixed(1),(-1.25).toFixed(1),(2.55).toFixed(1),(0.125).toFixed(2),(0.0001).toFixed(2),(-0.0001).toFixed(2),(1000000000000000100).toFixed(0),(5e-324).toFixed(100)]);",
        "[\"3\",\"-3\",\"1.3\",\"-1.3\",\"2.5\",\"0.13\",\"0.00\",\"-0.00\",\"1000000000000000128\",\"0.0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\"]",
    );
}

#[test]
fn number_constructor_owns_standard_name_and_length() {
    assert_tiers(
        "var n=Object.getOwnPropertyDescriptor(Number,'name');var l=Object.getOwnPropertyDescriptor(Number,'length');JSON.stringify([n.value,n.writable,n.enumerable,n.configurable,l.value,l.writable,l.enumerable,l.configurable]);",
        "[\"Number\",false,false,true,1,false,false,true]",
    );
}

#[test]
fn json_duplicate_keys_replace_values_without_reordering_or_changing_prototypes() {
    assert_tiers(
        "var result=JSON.parse('{\"__proto__\":1,\"a\":2,\"__proto__\":3,\"a\":4}');JSON.stringify([result.__proto__,result.a,Object.keys(result),Object.getPrototypeOf(result)===Object.prototype]);",
        "[3,4,[\"__proto__\",\"a\"],true]",
    );
}

#[test]
fn logical_array_lengths_still_respect_execution_budgets() {
    for source in [
        "var limited=false;try{Array.prototype.includes.call({length:9007199254740991},1)}catch(e){limited=e instanceof RangeError}JSON.stringify(limited);",
        "var limited=false;try{Array.prototype.findLast.call({length:9007199254740991},function(){return false})}catch(e){limited=e instanceof RangeError}JSON.stringify(limited);",
    ] {
        let prepared = Interpreter::compile(source).unwrap();
        assert_eq!(
            prepared.tier(),
            napi_vm::interpreter::ExecutionTier::Bytecode
        );
        let statements = Parser::new(Lexer::new(source).tokenize())
            .parse_program()
            .unwrap();
        for bytecode in [true, false] {
            let mut vm = Interpreter::with_builtins();
            vm.set_loop_budget(3);
            let result = if bytecode {
                vm.execute(&prepared)
            } else {
                vm.run_program_body(&statements)
            }
            .unwrap();
            assert!(
                matches!(result, Value::String(ref text) if text == "true"),
                "bytecode={bytecode}: {result:?}"
            );
        }
    }
}

#[test]
fn global_is_finite_and_is_nan_use_to_number() {
    assert_tiers(
        "var abruptFinite=false;try{isFinite({valueOf:function(){throw new Error('stop')}})}catch(e){abruptFinite=e.message==='stop'}var abruptNaN=false;try{isNaN({toString:function(){throw new Error('stop')}})}catch(e){abruptNaN=e.message==='stop'}var prim={};prim[Symbol.toPrimitive]=function(){return 42};var symFinite=false;try{isFinite(Symbol())}catch(e){symFinite=e instanceof TypeError}var bigNaN=false;try{isNaN(1n)}catch(e){bigNaN=e instanceof TypeError}JSON.stringify([abruptFinite,abruptNaN,isFinite(prim),isNaN(prim),symFinite,bigNaN,isFinite(),isNaN(),isFinite('42'),isNaN('foo')]);",
        "[true,true,true,false,true,true,false,true,true,true]",
    );
}

#[test]
fn is_prototype_of_throws_for_nullish_receiver() {
    assert_tiers(
        "var errors=[];for(var receiver of [null,undefined]){try{Object.prototype.isPrototypeOf.call(receiver,{}) ;errors.push(false)}catch(error){errors.push(error instanceof TypeError)}}JSON.stringify([errors,Object.prototype.isPrototypeOf.call(5,{}),({}).isPrototypeOf(Object.create({}))]);",
        "[[true,true],false,false]",
    );
}
