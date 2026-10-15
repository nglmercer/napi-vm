use napi_vm::{Interpreter, Lexer, Parser, TurnBudget, Value};

fn assert_tiers(source: &str, expected: &str, collect: bool) {
    for bytecode in [true, false] {
        let mut vm = Interpreter::with_builtins();
        if bytecode {
            let prepared = Interpreter::compile(source).unwrap();
            assert_eq!(
                prepared.tier(),
                napi_vm::interpreter::ExecutionTier::Bytecode
            );
            vm.execute(&prepared).unwrap();
        } else {
            let statements = Parser::new(Lexer::new(source).tokenize())
                .parse_program()
                .unwrap();
            vm.run_program_body(&statements).unwrap();
        }
        if collect {
            vm.collect_cycles();
            vm.eval_source("release(8);").unwrap();
        }
        vm.poll_event_loop(TurnBudget::jobs(10_000)).unwrap();
        let result = vm.eval_source("JSON.stringify(result);").unwrap();
        assert!(
            matches!(&result,Value::String(s) if s==expected),
            "bytecode={bytecode}: {result:?}"
        );
    }
}

#[test]
fn combinators_interleave_iteration_resolve_and_then_and_close_abruptly() {
    assert_tiers(
        r#"var result,log=[],iterator={},source={};source[Symbol.iterator]=function(){log.push('iterator');return iterator};var n=0;iterator.next=function(){log.push('next');return {done:n++>0,value:7}};iterator.return=function(){log.push('close');return {}};function C(executor){log.push('construct');executor(function(v){log.push('resolved')},function(e){log.push(e.message)})}Object.defineProperty(C,'resolve',{get:function(){log.push('get resolve');return function(v){log.push('resolve '+v);return {then:function(){log.push('then');throw new Error('stop')}}}}});var output=Promise.all.call(C,source);result=[output instanceof C,log];"#,
        r#"[true,["construct","get resolve","iterator","next","resolve 7","then","close","stop"]]"#,
        false,
    );
}

#[test]
fn synchronous_thenables_cannot_complete_combinators_early_or_repeat_slots() {
    assert_tiers(
        r#"var result,calls=0;function C(executor){executor(function(v){calls++;result=[v,calls]},function(e){throw e})}C.resolve=function(v){return {then:function(yes,no){yes(v);yes(99);no('late')}}};Promise.allSettled.call(C,[1,2]);"#,
        r#"[[{"status":"fulfilled","value":1},{"status":"fulfilled","value":2}],1]"#,
        false,
    );
}

#[test]
fn then_species_capabilities_survive_collection_with_their_closures() {
    assert_tiers(
        r#"var result,release;var p=new Promise(function(r){release=r});function C(executor){var token={x:42};executor(function(v){result=token.x+v},function(e){throw e})}var holder={};holder[Symbol.species]=C;p.constructor=holder;p.then(function(v){return v+1});p.constructor=undefined;holder=null;C=null;"#,
        "51",
        true,
    );
}

#[test]
fn finally_waits_for_returned_thenables_and_preserves_or_replaces_settlement() {
    assert_tiers(
        r#"var result=[],log=[];Promise.resolve(4).finally(function(){log.push('finally');return {then:function(resolve){log.push('thenable');resolve()}}}).then(function(v){result.push(v);result.push(log.join(','))});Promise.reject('original').finally(function(){return Promise.reject('replacement')}).catch(function(e){result.push(e)});"#,
        r#"["replacement",4,"finally,thenable"]"#,
        false,
    );
}

#[test]
fn keyed_combinators_and_try_preserve_symbols_null_prototypes_and_arguments() {
    assert_tiers(
        r#"var result=[],symbol=Symbol('key'),source={__proto__:null};source.a=Promise.resolve(1);source[symbol]=2;Promise.allKeyed(source).then(function(v){result.push([Object.getPrototypeOf(v)===null,v.a,v[symbol]])});Promise.try(function(a,b){return a+b},3,4).then(function(v){result.push(v)});var d=Promise.withResolvers();d.promise.then(function(v){result.push(v)});d.resolve(8);"#,
        r#"[7,8,[true,1,2]]"#,
        false,
    );
}

#[test]
fn from_async_waits_sequentially_and_awaits_iterator_cleanup() {
    assert_tiers(
        r#"var result,log=[],source={};source[Symbol.asyncIterator]=function(){var n=0;return {next:function(){log.push('next');return Promise.resolve({done:false,value:++n})},return:function(){log.push('close');return Promise.resolve().then(function(){log.push('closed');return {}})}}};var sentinel={};Array.fromAsync(source,function(v){log.push('map '+v);if(v===2)throw sentinel;return Promise.resolve(v*2)}).catch(function(e){result=[e===sentinel,log]});"#,
        r#"[true,["next","map 1","next","map 2","close","closed"]]"#,
        false,
    );
}

#[test]
fn from_async_distinguishes_async_values_from_sync_values_and_checks_lengths() {
    assert_tiers(
        r#"var result=[],thenable={then:function(resolve){resolve(9)}},source={};source[Symbol.asyncIterator]=function(){var n=0;return {next:function(){return {done:n++>0,value:thenable}}}};Array.fromAsync(source).then(function(v){result.push(v[0]===thenable)});Array.fromAsync([thenable]).then(function(v){result.push(v[0])});Array.fromAsync.call({}, {length:4294967296}).catch(function(e){result.push(e instanceof RangeError)});"#,
        r#"[true,true,9]"#,
        false,
    );
}

#[test]
fn async_disposal_moves_and_waits_in_reverse_order_while_nesting_errors() {
    assert_tiers(
        r#"var result,log=[],one={},two={},s=new AsyncDisposableStack();s.adopt(7,function(v){log.push(v);return Promise.reject(one)});s.defer(function(){log.push('last');return Promise.resolve().then(function(){log.push('awaited');throw two})});var moved=s.move();var promise=moved.disposeAsync();log.push('returned');promise.catch(function(e){result=[s.disposed,moved.disposed,log,e instanceof SuppressedError,e.error===one,e.suppressed===two,AsyncDisposableStack.prototype[Symbol.asyncDispose]===AsyncDisposableStack.prototype.disposeAsync]});"#,
        r#"[true,true,["last","returned","awaited",7],true,true,true,true]"#,
        false,
    );
}

#[test]
fn async_disposal_sync_fallback_ignores_return_and_rejects_wrong_brand() {
    assert_tiers(
        r#"var result=[],s=new AsyncDisposableStack(),v={};v[Symbol.dispose]=function(){return {then:function(){throw new Error('must not await sync return')}}};s.use(v);s.disposeAsync().then(function(){result.push('disposed')});AsyncDisposableStack.prototype.disposeAsync.call(new DisposableStack()).catch(function(e){result.push(e instanceof TypeError)});"#,
        r#"[true,"disposed"]"#,
        false,
    );
}
