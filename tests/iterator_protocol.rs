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

#[test]
fn body_throw_preserves_its_reason_when_getting_return_throws() {
    assert_eq!(
        evaluate(
            r#"
        let original = {}, secondary = {}, caught, closes = 0;
        let source = {};
        source[Symbol.iterator] = function() {return {
            next() {return {done: false, value: 1};},
            get return() {closes++; throw secondary;}
        };};
        try {for (let value of source) throw original;} catch(error) {caught = error;}
        (caught === original) + ':' + closes;
    "#
        ),
        "true:1"
    );
}

#[test]
fn body_throw_preserves_its_reason_when_return_is_non_callable() {
    assert_eq!(
        evaluate(
            r#"
        let original = {}, caught;
        let source = {};
        source[Symbol.iterator] = function() {return {
            next() {return {done: false, value: 1};}, return: 42
        };};
        try {for (let value of source) throw original;} catch(error) {caught = error;}
        caught === original;
    "#
        ),
        "true"
    );
}

#[test]
fn iterator_step_errors_do_not_close_the_iterator() {
    assert_eq!(
        evaluate(
            r#"
        let reason = {}, closes = 0, failures = 0;
        for (let kind of [0, 1, 2]) {
            let source = {};
            source[Symbol.iterator] = function() {return {
                next() {
                    if (kind === 0) throw reason;
                    return {
                        get done() {if (kind === 1) throw reason; return false;},
                        get value() {throw reason;}
                    };
                },
                return() {closes++; return {};}
            };};
            try {for (let value of source) {}} catch(error) {if (error === reason) failures++;}
        }
        failures + ':' + closes;
    "#
        ),
        "3:0"
    );
}

#[test]
fn close_error_replaces_a_return_completion() {
    assert_eq!(
        evaluate(
            r#"
        let reason = {}, caught;
        let source = {};
        source[Symbol.iterator] = function() {return {
            next() {return {done: false, value: 1};},
            get return() {throw reason;}
        };};
        function consume() {for (let value of source) return 42;}
        try {consume();} catch(error) {caught = error;}
        caught === reason;
    "#
        ),
        "true"
    );
}
