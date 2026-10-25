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
fn lookbehind_quantifiers_and_captures_run_from_right_to_left() {
    for source in [
        "var m=/(?<=([ab]+)([bc]+))$/.exec('abc');m[1]==='a'&&m[2]==='bc';",
        "var m=/(?<=([ab]+?)([bc]+?))$/.exec('abc');m[1]==='b'&&m[2]==='c';",
        r"var m=/(?<=(\w){3})def/d.exec('abcdef');m[1]==='a'&&m.indices[1][0]===0&&m.indices[1][1]===1;",
        "var m=/(?<=(a|aa))b/.exec('aab');m[1]==='a';",
    ] {
        assert_both(source);
    }
}

#[test]
fn backward_backreferences_observe_groups_to_their_right() {
    for source in [
        r"var m=/(?<=\1(a+))b/.exec('aaaab');m[1]==='aa';",
        r"var m=/(?<=(a+)\1)b/.exec('aaaab');m[1]==='aaaa';",
        r"var m=/(?<=\k<part>(?<part>a+))b/d.exec('aaaab');m.groups.part==='aa'&&m.indices.groups.part[0]===2&&m.indices.groups.part[1]===4;",
        r"var m=/(?<=a(.\2)b(\1)).{4}/.exec('aabcacbc');m[0]==='cacb'&&m[1]==='a'&&m[2]==='';",
    ] {
        assert_both(source);
    }
}

#[test]
fn nested_assertions_restore_the_enclosing_direction() {
    for source in [
        "var m=/(?<=a(?=(b))b)c/.exec('abc');m[0]==='c'&&m[1]==='b';",
        "var m=/a(?=(?<=a)(b))/.exec('ab');m[0]==='a'&&m[1]==='b';",
        "var m=/(?<!(a))b/.exec('cb');m[0]==='b'&&m[1]===undefined;",
        "var m=/(?<=(a))c|b/.exec('ab');m[0]==='b'&&m[1]===undefined;",
    ] {
        assert_both(source);
    }
}

#[test]
fn backward_matching_preserves_unicode_boundaries_and_string_set_lengths() {
    for source in [
        "var m=/(?<=(.))x/du.exec('𝄞x');m[1]==='𝄞'&&m.indices[1][0]===0&&m.indices[1][1]===2;",
        "var m=/(?<=(.))x/d.exec('𝄞x');m[1].charCodeAt(0)===0xdd1e&&m.indices[1][0]===1&&m.indices[1][1]===2;",
        r"var m=/(?<=([\q{ab|b}]+))c/v.exec('ababc');m[1]==='abab';",
        r"var m=/(?<=([\q{ab|b}]+?))c/v.exec('ababc');m[1]==='ab';",
        r"var m=/([\q{a|ab}]+?)/v.exec('abab');m[1]==='ab';",
        r"var m=/([\q{ab|a}]+?)b/v.exec('ab');m[1]==='a';",
    ] {
        assert_both(source);
    }
}

#[test]
fn unicode_backreferences_compare_code_points_and_do_not_split_surrogate_pairs() {
    for source in [
        r"/foo(.+)bar\1/u.exec('foo\uD834bar\uD834\uDC00')===null;",
        r"var m=/foo(.+)bar\1/u.exec('foo\uD834bar\uD834A');m[1]==='\uD834'&&m[0]==='foo\uD834bar\uD834';",
        r"var m=/(\u{10400})\1/iu.exec('\u{10400}\u{10428}');m[0].length===4;",
        r"var m=/(?<=\1(\u{10400}))x/iu.exec('\u{10400}\u{10428}x');m[0]==='x'&&m.index===4;",
        r"/(?<=\1(\uDC00))x/u.exec('\uD834\uDC00\uDC00x')===null;",
    ] {
        assert_both(source);
    }
}
