use napi_vm_core::{Interpreter, Lexer, Parser, Value};
fn both(source: &str) {
    for ast in [true, false] {
        let mut vm = Interpreter::with_builtins();
        let result = if ast {
            let mut p = Parser::new(Lexer::new(source).tokenize());
            vm.run_program_body(&p.parse())
        } else {
            let p = Interpreter::compile(source).unwrap();
            assert_eq!(p.tier(), napi_vm_core::interpreter::ExecutionTier::Bytecode);
            vm.execute(&p)
        };
        assert!(
            matches!(result, Ok(Value::Bool(true))),
            "AST={ast}: {result:?}"
        );
    }
}
#[test]
fn tag_application_precedes_constructor_invocation() {
    both(
        "var target,template;function C(x){this.value=x;target=new.target;}function tag(p){template=p;return C;}var a=new tag`first`;var first=template[0]==='first' && a instanceof C && a.value===undefined && target===C;var b=new tag`second`(42);first && b instanceof C && b.value===42 && template[0]==='second' && target===C",
    );
}
#[test]
fn tagged_constructor_member_access_and_tag_receivers_follow_member_grammar() {
    both(
        "var receiver;function C(x){this.value=x;}var object={tag(p){receiver=this;return {Inner:C};}};var a=new object.tag`member`.Inner(7);receiver===object && a instanceof C && a.value===7",
    );
}
#[test]
fn repeated_tags_are_evaluated_before_new() {
    both(
        "var order='';function C(){order+='C';}function tag(p){order+='A';return function(q){order+='B';return C;};}var x=new tag`first``second`;order==='ABC' && x instanceof C",
    );
}
#[test]
fn cache_hits_still_evaluate_tag_getters_and_substitutions() {
    both(
        "var seen=[],reads=0,sub=0;var object={get tag(){reads++;return function(parts,value){seen.push([parts,value]);};}};for(var i=0;i<3;i++)object.tag`x${++sub}`;reads===3 && sub===3 && seen[0][0]===seen[1][0] && seen[1][0]===seen[2][0] && seen[0][1]===1 && seen[1][1]===2 && seen[2][1]===3",
    );
}
