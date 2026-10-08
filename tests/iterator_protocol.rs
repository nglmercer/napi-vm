use napi_vm::Interpreter;

fn evaluate(source: &str) -> String {
    let mut vm = Interpreter::with_builtins();
    let value = vm.eval_source(source).expect("iterator evaluation");
    vm.vs(&value).expect("result string")
}

#[test]
fn spread_observes_next_done_and_value_getters_in_order() {
    assert_eq!(
        evaluate(
            r#"
            let log = [], calls = 0;
            let source = {
                [Symbol.iterator]() {
                    return {
                        get next() {
                            log.push('next');
                            return function() {
                                let count = ++calls;
                                return {
                                    get done() { log.push('done:' + count); return count === 2; },
                                    get value() { log.push('value:' + count); return 42; }
                                };
                            };
                        }
                    };
                }
            };
            let values = [...source];
            values.join(',') + '|' + log.join(',');
            "#,
        ),
        "42|next,done:1,value:1,done:2"
    );
}

#[test]
fn missing_done_keeps_spread_iterating_and_missing_value_is_undefined() {
    assert_eq!(
        evaluate(
            r#"
            let calls = 0;
            let source = {
                [Symbol.iterator]() {
                    return { next() { return ++calls === 2 ? {done: true} : {}; } };
                }
            };
            let values = [...source];
            values.length + ':' + (values[0] === undefined) + ':' + calls;
            "#,
        ),
        "1:true:2"
    );
}

#[test]
fn spread_rejects_non_object_results_and_missing_next() {
    assert_eq!(
        evaluate(
            r#"
            let failures = 0;
            for (let source of [
                {[Symbol.iterator]() { return {next() {return 1;}}; }},
                {[Symbol.iterator]() { return {}; }}
            ]) {
                try { [...source]; } catch (error) { if (error instanceof TypeError) failures++; }
            }
            failures;
            "#,
        ),
        "2"
    );
}

#[test]
fn call_spread_uses_the_same_iterator_protocol() {
    assert_eq!(
        evaluate(
            r#"
            let calls = 0;
            let source = {
                [Symbol.iterator]() {
                    return {next() {return ++calls === 2 ? {done: true} : {value: 42}; }};
                }
            };
            function take(value) { return value; }
            take(...source);
            "#,
        ),
        "42"
    );
}

#[test]
fn array_spread_observes_an_overridden_iterator() {
    assert_eq!(
        evaluate(
            "let source = [1, 2]; source[Symbol.iterator] = function*() { yield 42; }; [...source].join(',');",
        ),
        "42"
    );
}

#[test]
fn call_spread_observes_an_overridden_array_iterator() {
    assert_eq!(
        evaluate(
            "let source = [1, 2]; source[Symbol.iterator] = function*() { yield 42; }; function take(value) { return value; } take(...source);",
        ),
        "42"
    );
}
