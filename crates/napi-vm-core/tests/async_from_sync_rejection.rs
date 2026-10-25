use napi_vm_core::{Interpreter, Value, parser::parse_cached};

#[test]
fn delegated_throw_closes_on_rejection_and_preserves_the_original_reason() {
    for poisoned in [false, true] {
        for done in [false, true] {
            for prepared in [false, true] {
                let value = if poisoned {
                    "Object.defineProperty(Promise.resolve(1),'constructor',{get(){throw marker}})"
                } else {
                    "Promise.reject(marker)"
                };
                let source = format!(
                    "var marker={{}},other={{}},closed=0,caught,finished=false;
                     var iterable={{[Symbol.iterator](){{return this}},next(){{return {{value:1,done:false}}}},
                     throw(){{return {{value:{value},done:{done}}}}},return(){{closed++;throw other}}}};
                     async function* delegate(){{yield* iterable}}
                     async function run(){{var g=delegate();await g.next();try{{await g.throw(other)}}catch(e){{caught=e}}finished=true}}
                     run();"
                );
                let mut vm = Interpreter::with_builtins();
                if prepared {
                    vm.eval_source(&source).unwrap();
                } else {
                    vm.run_program_body(&parse_cached(&source).unwrap())
                        .unwrap();
                    vm.drain_jobs().unwrap();
                }
                let expected_closes = usize::from(!done);
                let result = vm.eval_source(&format!(
                    "finished&&caught===marker&&closed==={expected_closes};"
                ));
                assert!(
                    matches!(result, Ok(Value::Bool(true))),
                    "poisoned={poisoned}, done={done}, prepared={prepared}: {result:?}"
                );
            }
        }
    }
}

#[test]
fn delegated_return_does_not_close_again_when_its_value_rejects() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(
        "var marker={},calls=0,caught,finished=false;
         var iterable={[Symbol.iterator](){return this},next(){return {value:1,done:false}},
         return(){calls++;return {value:Promise.reject(marker),done:false}}};
         async function* delegate(){yield* iterable}
         async function run(){var g=delegate();await g.next();try{await g.return()}catch(e){caught=e}finished=true}
         run();",
    ).unwrap();
    assert!(matches!(
        vm.eval_source("finished&&caught===marker&&calls===1;"),
        Ok(Value::Bool(true))
    ));
}
