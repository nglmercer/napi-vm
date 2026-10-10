use napi_vm_core::{Interpreter, Value};

fn truth(vm: &mut Interpreter, source: &str) {
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "{source}: {result:?}"
    );
}

#[test]
fn legacy_match_state_tracks_captures_context_and_input_aliases() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "RegExp.input===''&&RegExp.$1===''&&RegExp.lastMatch==='';",
    );
    truth(
        &mut vm,
        "/(a)(b)/.exec('xabz');RegExp.input==='xabz'&&RegExp.$_==='xabz'&&RegExp.$1==='a'&&RegExp.$2==='b'&&RegExp.$9===''&&RegExp.lastMatch==='ab'&&RegExp['$&']==='ab'&&RegExp.lastParen==='b'&&RegExp['$+']==='b'&&RegExp.leftContext==='x'&&RegExp['$`']==='x'&&RegExp.rightContext==='z'&&RegExp[\"$'\"]==='z';",
    );
    truth(
        &mut vm,
        "/missing/.exec('other');RegExp.input==='xabz'&&RegExp.lastMatch==='ab';",
    );
    truth(
        &mut vm,
        "var calls=0;RegExp.$_={toString(){calls++;return 'changed';}};RegExp.input==='changed'&&calls===1&&RegExp.lastMatch==='ab';",
    );
}

#[test]
fn legacy_accessors_validate_receivers_before_coercion() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var caught=0;for(var key of ['input','lastMatch','lastParen','leftContext','rightContext','$1','$9']){try{Reflect.get(RegExp,key,{});}catch(e){if(e instanceof TypeError)caught++;}}var calls=0;try{Reflect.set(RegExp,'input',{toString(){calls++;return 'bad';}},{});}catch(e){if(e instanceof TypeError)caught++;}caught===8&&calls===0;",
    );
    truth(
        &mut vm,
        "class Derived extends RegExp{}var caught;try{Derived.lastMatch;}catch(e){caught=e;}caught instanceof TypeError;",
    );
}

#[test]
fn legacy_accessors_have_real_descriptors_and_intrinsic_identity() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var input=Object.getOwnPropertyDescriptor(RegExp,'input');var match=Object.getOwnPropertyDescriptor(RegExp,'lastMatch');typeof input.get==='function'&&typeof input.set==='function'&&input.get.length===0&&input.set.length===1&&!input.enumerable&&input.configurable&&typeof match.get==='function'&&match.set===undefined&&!match.enumerable&&match.configurable;",
    );
    truth(
        &mut vm,
        "var Original=RegExp;/a/.test('a');RegExp=function Replacement(){};Reflect.get(Original,'lastMatch',Original)==='a'&&Reflect.get(Original.prototype,'source',Original.prototype)==='(?:)';",
    );
}

#[test]
fn legacy_state_is_shared_by_matching_helpers() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "'abc'.replace(/(b)/,'x');RegExp.$1==='b'&&RegExp.leftContext==='a'&&RegExp.rightContext==='c';",
    );
    truth(
        &mut vm,
        "'a1b2'.match(/([0-9])/g);RegExp.$1==='2'&&RegExp.lastMatch==='2';",
    );
    truth(
        &mut vm,
        "'abc'.search(/(b)/);RegExp.$1==='b'&&RegExp.input==='abc';",
    );
    truth(
        &mut vm,
        "'axc'.split(/(x)/);RegExp.$1==='x'&&RegExp.leftContext==='a'&&RegExp.rightContext==='c';",
    );
}

#[test]
fn disabled_legacy_matches_invalidate_and_normal_matches_restore_state() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "class Derived extends RegExp{}new Derived('(a)').exec('a');var caught;try{RegExp.$1;}catch(e){caught=e;}caught instanceof TypeError;",
    );
    truth(
        &mut vm,
        "RegExp.input='recovered';var caught;try{RegExp.lastMatch;}catch(e){caught=e;}RegExp.input==='recovered'&&caught instanceof TypeError;",
    );
    truth(&mut vm, "/(b)/.exec('b');RegExp.$1==='b';");
}

#[test]
fn foreign_legacy_accessors_keep_their_realm_after_collection() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child.eval_source("/(foreign)/.exec('foreign');").unwrap();
    for (name, source) in [
        ("ForeignRegExp", "RegExp"),
        ("ForeignTypeError", "TypeError"),
        ("foreignExec", "RegExp.prototype.exec"),
        ("foreignPattern", "/child/"),
    ] {
        vm.set_global_checked(name, child.eval_source(source).unwrap())
            .unwrap();
    }
    drop(child);
    truth(
        &mut vm,
        "/(parent)/.exec('parent');RegExp.$1==='parent'&&ForeignRegExp.$1==='foreign';",
    );
    truth(
        &mut vm,
        "var caught;try{Reflect.get(ForeignRegExp,'lastMatch',RegExp);}catch(e){caught=e;}caught instanceof ForeignTypeError&&!(caught instanceof TypeError);",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    truth(
        &mut vm,
        "foreignExec.call(foreignPattern,'child');ForeignRegExp.lastMatch==='child'&&RegExp.lastMatch==='parent';",
    );
    truth(
        &mut vm,
        "RegExp.prototype.exec.call(foreignPattern,'child');ForeignRegExp.lastMatch==='child'&&RegExp.lastMatch==='parent';",
    );
}
