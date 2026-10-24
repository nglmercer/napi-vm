use napi_vm::{Interpreter, Lexer, Parser, Value};
fn both(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    let compiled = Interpreter::with_builtins().execute(&program).unwrap();
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let ast = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    for value in [compiled, ast] {
        assert!(
            matches!(value, Value::String(ref s) if s == expected),
            "{value:?}"
        );
    }
}
#[test]
fn numeric_sort_and_copy_keep_special_values_and_source() {
    both(
        r#"
        var a = new Float64Array(512);
        for (var i = 0; i < a.length; i++) a[i] = (i * 137) % 511 - 255;
        a[0] = NaN; a[1] = -0; a[2] = 0; a[3] = Infinity; a[4] = -Infinity;
        var b = a.toSorted();
        var unchanged = Number.isNaN(a[0]) && Object.is(a[1], -0);
        var identity = a.sort() === a;
        var same = true, ordered = true, negativeZeroFirst = false;
        for (var i = 0; i < a.length; i++) {
            if (!Object.is(a[i], b[i])) same = false;
            if (i && a[i-1] > a[i]) ordered = false;
            if (i && Object.is(a[i-1], -0) && Object.is(a[i], 0)) negativeZeroFirst = true;
        }
        JSON.stringify([unchanged,identity,same,ordered,negativeZeroFirst,
            a[0] === -Infinity,a[510] === Infinity,Number.isNaN(a[511])]);
    "#,
        "[true,true,true,true,true,true,true,true]",
    );
}
#[test]
fn custom_comparators_and_bigints_keep_their_dispatch() {
    both(
        r#"
        var a = new Int32Array(256);
        for (var i = 0; i < a.length; i++) a[i] = i;
        var calls = 0;
        a.sort(function(a,b) {calls++; return b-a;});
        var stable = new Float64Array(256);
        for (var i = 0; i < stable.length; i++) stable[i] = i;
        stable.sort(function() {return NaN;});
        var big = new BigInt64Array(256);
        for (var i = 0; i < big.length; i++) big[i] = BigInt(255-i);
        big.sort();
        JSON.stringify([a[0],a[255],calls > 0,stable[0],stable[255],
            big[0] === 0n,big[255] === 255n]);
    "#,
        "[255,0,true,0,255,true,true]",
    );
}
