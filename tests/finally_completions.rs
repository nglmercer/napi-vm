use napi_vm::interpreter::ExecutionTier;
use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(prepared.tier(), ExecutionTier::Bytecode);
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "bytecode: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "AST: {result:?}"
    );
}

#[test]
fn nested_catches_preserve_the_original_throw() {
    both(
        "var result;try{try{throw 'original';}finally{try{throw 'inner';}catch(e){}}}catch(e){result=e;}result",
        "original",
    );
}

#[test]
fn nested_finally_pads_preserve_a_return() {
    both(
        "function f(){try{return 'original';}finally{try{try{throw 'inner';}finally{try{throw 'caught';}catch(e){}}}catch(e){}}}f()",
        "original",
    );
}

#[test]
fn breaking_from_a_finally_discards_only_its_pending_completion() {
    both(
        "var result;try{try{throw 'outer';}finally{label:{try{throw 'inner';}finally{break label;}}}}catch(e){result=e;}result",
        "outer",
    );
}

#[test]
fn continuing_from_a_finally_discards_its_pending_completion() {
    both(
        "var result;try{try{throw 'outer';}finally{for(var i=0;i<2;i++){try{throw 'inner';}finally{continue;}}}}catch(e){result=e;}result",
        "outer",
    );
}

#[test]
fn inline_finally_errors_do_not_run_cleanup_twice() {
    both(
        "var count=0,result;try{while(true){try{break;}finally{count++;throw 'cleanup';}}}catch(e){result=e;}count+','+result",
        "1,cleanup",
    );
}

#[test]
fn nested_normal_finally_keeps_the_outer_pending_completion() {
    both(
        "var result,log='';try{try{throw 'outer';}finally{try{log+='a';}finally{log+='b';}}}catch(e){result=e;}log+','+result",
        "ab,outer",
    );
}

#[test]
fn local_loop_exits_inside_cleanup_preserve_the_pending_throw() {
    both(
        "var result;try{try{throw 'outer';}finally{for(var i=0;i<3;i++){if(i===0)continue;break;}}}catch(e){result=e;}result",
        "outer",
    );
}

#[test]
fn cleanup_return_and_throw_replace_earlier_completions() {
    both(
        "function f(){try{throw 'old';}finally{try{throw 'caught';}catch(e){}return 'new';}}function g(){try{return 'old';}finally{throw 'new';}}var result;try{g();}catch(e){result=e;}f()+','+result",
        "new,new",
    );
}

#[test]
fn break_cleanup_can_continue_an_outer_loop_without_stale_handlers() {
    both(
        "var log='';outer:for(var i=0;i<3;i++){while(true){try{break;}finally{log+=i;continue outer;}}}try{throw 'done';}catch(e){log+=e;}log",
        "012done",
    );
}

#[test]
fn iterator_cleanup_nested_in_finally_restores_the_original_throw() {
    both(
        "var result,closed=0;var iterator={next:function(){return {value:1,done:false};},return:function(){closed++;return {};}};var iterable={};iterable[Symbol.iterator]=function(){return iterator;};try{try{throw 'outer';}finally{try{for(var x of iterable){throw 'inner';}}catch(e){}}}catch(e){result=e;}closed+','+result",
        "1,outer",
    );
}

#[test]
fn iterator_close_errors_on_break_are_not_closed_again() {
    both(
        "var count=0,result;var iterator={next:function(){return {value:1,done:false};},return:function(){count++;throw 'close';}};var iterable={};iterable[Symbol.iterator]=function(){return iterator;};try{for(var x of iterable){break;}}catch(e){result=e;}count+','+result",
        "1,close",
    );
}
