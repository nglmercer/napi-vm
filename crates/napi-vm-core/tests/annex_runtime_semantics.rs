use napi_vm_core::{Interpreter, Value, parser::parse_cached};

fn assert_both(source: &str) {
    let prepared = Interpreter::with_builtins().eval_source(source);
    assert!(
        matches!(prepared, Ok(Value::Bool(true))),
        "{source}: {prepared:?}"
    );
    let ast =
        Interpreter::with_builtins().run_program_body(&parse_cached(source).expect("valid source"));
    assert!(matches!(ast, Ok(Value::Bool(true))), "{source}: {ast:?}");
}

#[test]
fn dynamic_function_parameters_do_not_gain_a_leading_line_terminator() {
    for constructor in [
        "Function",
        "(async function(){}).constructor",
        "(function*(){}).constructor",
        "(async function*(){}).constructor",
    ] {
        assert_both(&format!(
            "var C={constructor},caught=false;try{{C('-->','')}}catch(e){{caught=e instanceof SyntaxError}}C('\\n-->','');C('<!--','');C('-->');caught;"
        ));
    }
}

#[test]
fn var_iteration_assigns_the_catch_parameter_and_preserves_the_outer_var() {
    for head in ["in {key:0}", "of ['key']", "of ['first','key']"] {
        for nested in [false, true] {
            let loop_body = format!("for(var err {head}){{during=err}}after=err;");
            let loop_body = if nested {
                format!("{{{loop_body}}}")
            } else {
                loop_body
            };
            let setup = format!(
                "var err='outer',before,during,after;try{{throw 'exception'}}catch(err){{before=err;{loop_body}}}"
            );
            let condition = "before==='exception'&&during==='key'&&after==='key'&&err==='outer'";
            assert_both(&format!("{setup}{condition};"));
            assert_both(&format!(
                "function test(){{{setup}return {condition}}}test();"
            ));
        }
    }
}

#[test]
fn captured_var_iteration_heads_share_the_hoisted_binding() {
    assert_both(
        "function test(){var read;{for(var x of [1,2]){read=()=>x}}return x===2&&read()===2}test();",
    );
    assert_both(
        "var read,err='outer';try{throw 0}catch(err){read=()=>err;for(var err of [1,2]){}}read()===2&&err==='outer';",
    );
}
