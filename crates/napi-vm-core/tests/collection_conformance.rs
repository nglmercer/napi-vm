use napi_vm_core::{Interpreter, Value};

fn check(source: &str) {
    let mut vm = Interpreter::with_builtins();
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "{source}: {result:?}"
    );
}

#[test]
fn constructors_require_new_and_methods_are_not_constructors() {
    check(
        "var failed=0; for (var C of [Map,Set,WeakMap,WeakSet]) { try { C(); } catch(e) { if(e.name==='TypeError') failed++; } } failed===4;",
    );
    check(
        "var failed=false; try { new WeakMap.prototype.set({},1); } catch(e) { failed=e.name==='TypeError'; } failed;",
    );
    check(
        "var failed=false; try { Reflect.construct(function(){}, [], WeakMap.prototype.set); } catch(e) { failed=e.name==='TypeError'; } failed;",
    );
}

#[test]
fn adder_is_read_once_before_iteration_and_overrides_are_called() {
    check(
        "var calls=0; var reads=0; var original=WeakMap.prototype.set; Object.defineProperty(WeakMap.prototype,'set',{configurable:true,get:function(){ reads++; return function(k,v){calls++; return original.call(this,k,v);}; }}); var key={}; var map=new WeakMap([[key,42]]); calls===1 && reads===1 && map.get(key)===42;",
    );
    check(
        "var reason={}; var got=false; Object.defineProperty(WeakMap.prototype,'set',{get:function(){throw reason;}}); new WeakMap(null); try { new WeakMap([]); } catch(e) {got=e===reason;} got;",
    );
}

#[test]
fn entries_are_inserted_incrementally_and_iterator_getters_are_observed() {
    check(
        r#"
      var order = '';
      var iterable = {};
      Object.defineProperty(iterable, Symbol.iterator, {
        get: function() {
          order += 'i';
          return function() {
            return {
              next: function() {
                order += 'n';
                if (order.length > 3) return {done: true};
                return {done: false, get value() { order += 'v'; return [{}, 1]; }};
              }
            };
          };
        }
      });
      var old = WeakMap.prototype.set;
      WeakMap.prototype.set = function(k, v) { order += 's'; return old.call(this, k, v); };
      new WeakMap(iterable);
      order === 'invsn';
    "#,
    );
}

#[test]
fn invalid_entries_close_the_iterator_and_preserve_the_original_throw() {
    check(
        "var closed=0; var reason={}; var iterable={ [Symbol.iterator]:function(){return {next:function(){return {done:false,value:0};},return:function(){closed++;throw reason;}};}}; var type=false; try {new WeakMap(iterable);}catch(e){type=e.name==='TypeError';} type && closed===1;",
    );
    check(
        "var closed=0; var reason={}; var iterable={ [Symbol.iterator]:function(){return {next:function(){return {done:false,value:[{},1]};},return:function(){closed++;throw 2;}};}}; WeakMap.prototype.set=function(){throw reason;}; var same=false; try {new WeakMap(iterable);}catch(e){same=e===reason;} same && closed===1;",
    );
    check(
        "var reason={};var closed=0;var iterable={ [Symbol.iterator]:function(){return {next:function(){return {done:false,get value(){throw reason;}};},return:function(){closed++;}};}};var same=false;try{new WeakMap(iterable);}catch(e){same=e===reason;}same && closed===0;",
    );
}

#[test]
fn method_metadata_and_iterator_aliases_match_the_standard() {
    check(
        "var d=Object.getOwnPropertyDescriptor(WeakMap.prototype.set,'length'); d.value===2 && !d.writable && !d.enumerable && d.configurable && WeakMap.prototype.set.name==='set' && Object.getPrototypeOf(WeakMap)===Function.prototype && Object.getPrototypeOf(WeakMap.prototype.set)===Function.prototype;",
    );
    check(
        "Set.prototype.keys===Set.prototype.values && Set.prototype.values===Set.prototype[Symbol.iterator] && Map.prototype.entries===Map.prototype[Symbol.iterator] && WeakMap.prototype[Symbol.toStringTag]==='WeakMap';",
    );
}

#[test]
fn reflect_construct_uses_array_like_arguments_and_new_target() {
    check(
        "function Target(x){this.x=x;} function Other(){} var instance=Reflect.construct(Target,{0:42,length:1},Other); instance.x===42 && Object.getPrototypeOf(instance)===Other.prototype;",
    );
    check(
        "function Other(){} var map=Reflect.construct(WeakMap,[],Other); Object.getPrototypeOf(map)===Other.prototype;",
    );
}

#[test]
fn native_methods_remain_valid_accessors_with_the_same_identity() {
    check(
        "var map=new WeakMap(); Object.defineProperty(map,'peek',{get:WeakMap.prototype.get}); map.peek===undefined && Object.getOwnPropertyDescriptor(map,'peek').get===WeakMap.prototype.get;",
    );
}

#[test]
fn date_extremes_are_clipped_without_overflow_and_iso_rejects_invalid_dates() {
    check(
        "Number.isNaN(Date.UTC(Infinity)) && Number.isNaN(Date.UTC(2000,0,1,Infinity)) && Number.isNaN(Date.UTC()) && Number.isNaN(new Date(1e100).getTime());",
    );
    check(
        "var errors=0; for(var year of [Infinity,-Infinity]) {try{new Date(year,0).toISOString();}catch(e){if(e.name==='RangeError')errors++;}} errors===2;",
    );
    check(
        "Date.UTC(2000,-1,1)===Date.UTC(1999,11,1) && Date.UTC(2000,12,1)===Date.UTC(2001,0,1) && new Date(NaN).toJSON()===null;",
    );
    check(
        "new Date(8640000000000000).toISOString()==='+275760-09-13T00:00:00.000Z' && new Date(-8640000000000000).toISOString()==='-271821-04-20T00:00:00.000Z';",
    );
}

#[test]
fn long_character_repeats_are_iterative_and_backtrack_at_unicode_boundaries() {
    check(
        "/^a+$/.test('a'.repeat(20000)) && /^a+ab$/.test('a'.repeat(20000)+'b') && /^a+?b$/.test('a'.repeat(20000)+'b');",
    );
    check(
        "/^(?:😀)+😀b$/u.test('😀'.repeat(20000)+'b') && /^a{2,4}a$/.exec('aaaaa')[0]==='aaaaa';",
    );
    check(
        "var caught=false; try{ /^(ab)+$/.test('ab'.repeat(20000)); }catch(e){caught=e.name==='RangeError';}caught;",
    );
}

#[test]
fn collection_brands_are_not_shared_or_forgeable() {
    check(
        "var failures=0; for(var wrong of [new Map(),new Set(),new WeakSet(),{}]) {try{WeakMap.prototype.get.call(wrong,{});}catch(e){if(e.name==='TypeError')failures++;}} failures===4;",
    );
    check(
        "var fake={'__symbol_collection__':'WeakMap','__symbol_entries__':[]};var caught=false;try{WeakMap.prototype.get.call(fake,{});}catch(e){caught=e.name==='TypeError';}caught;",
    );
}

#[test]
fn upsert_preserves_existing_undefined_and_rechecks_callback_mutations() {
    check(
        "var map=new WeakMap();var key={};map.set(key,undefined);var calls=0;map.getOrInsertComputed(key,function(){calls++;return 42;})===undefined && calls===0 && map.getOrInsert(key,1)===undefined;",
    );
    check(
        "var map=new WeakMap();var key={};var value=map.getOrInsertComputed(key,function(k){map.set(k,1);return 42;});value===42 && map.get(key)===42;",
    );
    check(
        "var map=new Map();var calls=0;map.getOrInsertComputed(-0,function(k){calls++;return 1/k;})===Infinity && calls===1 && map.get(0)===Infinity;",
    );
}

#[test]
fn constructors_accept_keyword_member_names() {
    check("var holder={delete:function(){this.value=42;}}; new holder.delete().value===42;");
}

#[test]
fn date_arithmetic_uses_ieee_evaluation_order_for_cancelling_components() {
    check(
        "Date.UTC(1970,0,1,80063993375,29,1,-288230376151711740)===29312 && Date.UTC(1970,0,213503982336,0,0,0,-18446744073709552000)===34447360;",
    );
}
