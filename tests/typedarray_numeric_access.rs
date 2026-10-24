use napi_vm::{Interpreter, Value};
fn check(source: &str) { let mut vm=Interpreter::with_builtins(); assert!(matches!(vm.eval_source(source).unwrap(),Value::Bool(true))); }
#[test]
fn numbers_and_string_keys_preserve_signed_zero_rules(){check(r#"
var a=new Int32Array(2);a[-0]=7;a[1]=9;
var invalid=[NaN,Infinity,-Infinity,-1,0.5,1e100];
var ok=a[0]===7 && a['-0']===undefined;
for(var k of invalid){a[k]=23;ok=ok && a[k]===undefined;}
ok && a[0]===7 && a[1]===9 && a['1']===9;
"#);}
#[test]
fn invalid_writes_still_coerce_values_and_foreign_receivers_are_ordinary(){check(r#"
var a=new Uint8Array(1),hits=0;
a[NaN]={valueOf(){hits++;return 4;}};
var r={}; Reflect.set(a,0,8,r);
var key={toString(){hits++;return '0';}};a[key]=5;
hits===2 && a[0]===5 && r[0]===8 && Reflect.get(a,0,{})===5;
"#);}
#[test]
fn coercion_can_detach_and_bigint_errors_are_preserved(){check(r#"
var a=new Uint8Array(1),key=0;
a[key]={valueOf(){a.buffer.transfer();return 4;}};
var b=new BigInt64Array(1);b[0]=3n;
var rejected=false;try{b[Infinity]=1;}catch(e){rejected=e instanceof TypeError;}
a[0]===undefined && b[0]===3n && rejected;
"#);}
#[test]
fn numeric_access_through_proxy_keeps_string_trap_keys(){check(r#"
var a=new Uint8Array([6]),log=[];
var p=new Proxy(a,{get(t,k){log.push(k);return Reflect.get(t,k,t);},set(t,k,v){log.push(k);return Reflect.set(t,k,v,t);}});
var x=p[-0];p[0]=8;x===6 && a[0]===8 && log.length===2 && log[0]==='0' && log[1]==='0';
"#);}
