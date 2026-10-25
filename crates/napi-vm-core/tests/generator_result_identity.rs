use napi_vm_core::{Interpreter, Value, parser::parse_cached};

fn assert_both(source: &str) {
    let prepared = Interpreter::with_builtins().eval_source(source);
    assert!(
        matches!(prepared, Ok(Value::Bool(true))),
        "{source}: {prepared:?}"
    );
    let ast = Interpreter::with_builtins().run_program_body(&parse_cached(source).unwrap());
    assert!(matches!(ast, Ok(Value::Bool(true))), "{source}: {ast:?}");
}

#[test]
fn delegated_results_keep_identity_and_defer_value_reads_until_completion() {
    assert_both(
        "var complete=false,doneReads=0,valueReads=0;
         var step={get done(){doneReads++;return complete},get value(){valueReads++;return 42}};
         var delegate={[Symbol.iterator](){return this},next(){return step}};
         function* run(){return yield* delegate}
         var g=run(),first=g.next(),second=g.next();
         var untouched=first===step&&second===step&&doneReads===2&&valueReads===0;
         complete=true;var last=g.next();
         untouched&&last!==step&&last.done&&last.value===42&&doneReads===3&&valueReads===1;",
    );
}

#[test]
fn throw_and_return_forward_incomplete_results_without_reading_value() {
    for operation in ["throw", "return"] {
        assert_both(&format!(
            "var marker={{}},reads=0,received;
             var step={{done:false,get value(){{reads++;throw marker}}}};
             var delegate={{[Symbol.iterator](){{return this}},next(){{return step}},
                 {operation}(value){{received=value;return step}}}};
             function* run(){{yield* delegate}}
             var g=run(),first=g.next(),second=g.{operation}(marker);
             var untouched=first===step&&second===step&&received===marker&&reads===0;
             step.done=true;var caught;try{{g.next()}}catch(e){{caught=e}}
             untouched&&caught===marker&&reads===1&&g.next().done;"
        ));
    }
}

#[test]
fn nested_delegation_forwards_proxy_results_without_copying_properties() {
    assert_both(
        "var values=0,doneReads=0;
         var step=new Proxy({},{get(t,k){if(k==='done'){doneReads++;return false}if(k==='value'){values++;throw 99}}});
         var delegate={[Symbol.iterator](){return this},next(){return step}};
         function* inner(){yield* delegate}function* outer(){yield* inner()}
         var g=outer(),result=g.next();result===step&&doneReads===2&&values===0;",
    );
}

#[test]
fn ordinary_yields_after_delegation_still_create_result_objects() {
    assert_both(
        "var count=0,step={value:1};
         var delegate={[Symbol.iterator](){return this},next(){return count++===0?step:{done:true}}};
         function* run(){yield* delegate;yield step}
         var g=run(),first=g.next(),second=g.next();
         first===step&&second!==step&&second.value===step&&second.done===false&&g.next().done;",
    );
}

#[test]
fn abrupt_done_reads_complete_the_generator_without_touching_value() {
    assert_both(
        "var marker={},reads=0;
         var delegate={[Symbol.iterator](){return this},next(){return {get done(){throw marker},get value(){reads++;return 1}}}};
         function* run(){yield* delegate}
         var g=run(),caught;try{g.next()}catch(e){caught=e}
         caught===marker&&reads===0&&g.next().done;",
    );
}
