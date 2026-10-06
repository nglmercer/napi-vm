// Tests for the browser (`wasm32`) build.
//
// The main suite runs the *native* addon, so nothing else here exercises the
// browser path — and it has broken silently before, because the two targets
// compile different code. These load the built WASM module directly under
// Node, which is close enough to the browser for everything the VM does.
//
// Run `npm run playground:build` first; the suite skips itself if the package
// is not built.

import { test, before, describe } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const pkg = join(root, "playground", "pkg");
const built = existsSync(join(pkg, "napi_vm_bg.wasm"));

let vm;
let timersAvailable = false;

before(async () => {
  if (!built) return;
  const module = await import(join(pkg, "napi_vm.js"));
  module.initSync({ module: readFileSync(join(pkg, "napi_vm_bg.wasm")) });
  vm = new module.WasmVm();
  try { vm.enable_timers(); timersAvailable = true; } catch {}
});

/// Run one program and return its value, asserting that it succeeded.
function run(source) {
  const result = vm.run(source);
  assert.ok(result.ok, `expected success, got: ${result.error}`);
  return result.value;
}

describe("browser build", { skip: built ? false : "playground/pkg is not built" }, () => {
  test("UTF-16 host callbacks preserve surrogate strings and keys", () => {
    assert.equal(run("'" + '\ud800' + "'.charCodeAt(0)"), '55296');
    vm.expose_function('\ud800', value => value);
    assert.equal(run("globalThis['\\ud800'](42)"), '42');
    assert.match(vm.get_global('\ud800'), /Function/);
    assert.throws(() => vm.register_module('bad', "export default '" + '\ud800' + "';"), /valid Unicode/);
    vm.expose_function('utfEcho', value => value);
    assert.equal(run("utfEcho('\\ud800').charCodeAt(0)"), '55296');
    assert.equal(run("utfEcho({'\\ud800':'\\udc00'})['\\ud800'].charCodeAt(0)"), '56320');
    assert.equal(run("'😀'.length"), '2');
    vm.expose_function('utfThrow', () => { throw new Error('\ud800'); });
    assert.equal(run("try { utfThrow(); } catch(e) { e.message.charCodeAt(0); }"), '55296');
  });

  test("await assimilates thenables and module namespaces reject mutation", () => {
    assert.equal(run('await { then(resolve) { resolve(42); } };'), '42');
    vm.register_module('linked-order', 'export const z = 3; export default 1;');
    assert.equal(run('import * as linked from "linked-order"; Object.keys(linked).join();'), 'default,z');
    assert.equal(run('Reflect.set(linked, "z", 9);'), 'false');
    assert.equal(run('linked.z;'), '3');
    assert.equal(run('var weakTarget = {}; var weakRef = new WeakRef(weakTarget); weakRef.deref() === weakTarget;'), 'true');
  });

  test("numeric builtin regressions also work in the browser target", () => {
    assert.equal(run('Number.isNaN(parseInt("12", 1));'), "true");
    assert.equal(run('parseInt("0xff", 16);'), "255");
    assert.equal(run('parseFloat(".5rest");'), "0.5");
    assert.equal(run('parseFloat("1e+2rest");'), "100");
    assert.equal(run('Object.is(Math.round(-0.1), -0);'), "true");
    assert.equal(run('4294967295 >>> 0;'), "4294967295");
  });

  test("array callbacks and iterators read current elements", () => {
    assert.equal(run('[1].map(function(x) { return x + this.n; }, {n:2}).join();'), "3");
    assert.equal(run('(() => { const a=[1,2]; return a.map(x => { a[1]=9; return x; }).join(); })();'), "1,9");
    assert.equal(run('(() => { const a=[1]; const it=a.values(); it.next(); a.push(2); return it.next().value; })();'), "2");
    assert.equal(run('[1,2,3].slice(1, undefined).join();'), "2,3");
    assert.equal(run('"abc".indexOf("a", -1);'), "0");
  });

  test("function constructors propagate guest exceptions", () => {
    assert.equal(run('(() => { function F() { throw new Error("boom"); } try { new F(); } catch (e) { return e.message; } })();'), "boom");
    assert.equal(run('(() => { function F(a,b,...rest) { this.n=rest.length; } return new F().n; })();'), "0");
  });

  test("generators yield their values", () => {
    assert.equal(run("function* g() { yield 1; yield 2; } [...g()].join();"), "1,2");
    assert.equal(run("function* g() { yield 1; } g().next().value;"), "1");
  });

  test("a generator reports done and its return value", () => {
    assert.equal(
      run("function* g() { yield 1; return 9; } const it = g(); it.next(); it.next().value;"),
      "9",
    );
    assert.equal(
      run("function* g() { yield 1; } const it = g(); it.next(); String(it.next().done);"),
      "true",
    );
  });

  test("generators work in loops and comprehensions", () => {
    assert.equal(run("function* g() { for (let i = 0; i < 3; i++) yield i; } [...g()].join();"), "0,1,2");
    assert.equal(run("function* g() { yield 1; } let t = 0; for (const v of g()) t += v; t;"), "1");
    assert.equal(run("function* g() { yield 1; yield 2; } Array.from(g()).join();"), "1,2");
  });

  test("yield* delegates", () => {
    assert.equal(
      run(
        "function* inner() { yield 1; yield 2; } function* outer() { yield 0; yield* inner(); yield 3; } [...outer()].join();",
      ),
      "0,1,2,3",
    );
  });

  test("an unbounded generator is a catchable error, not a hang", () => {
    // The browser target cannot suspend a body, so it runs to completion under
    // a cap rather than streaming. The cap must be reachable and catchable.
    assert.equal(
      run(
        "function* f() { let a = 0, b = 1; while (true) { yield a; const n = a + b; a = b; b = n; } } try { [...f()]; 'no'; } catch (e) { 'caught'; }",
      ),
      "caught",
    );
  });

  test("the language features the native build has are present", () => {
    assert.equal(run("const s = new Set([1, 2]); s.size;"), "2");
    assert.equal(run("/ab+/.test('abbb');"), "true");
    assert.equal(run("(2n ** 64n).toString();"), "18446744073709551616");
    assert.equal(run("new Date(0).toISOString();"), "1970-01-01T00:00:00.000Z");
    assert.equal(run("new Uint8Array([1, 2, 3]).join();"), "1,2,3");
    assert.equal(run("const o = { a: 1 }; delete o.a; JSON.stringify(o);"), "{}");
    assert.equal(run("class A { #v = 3; get v() { return this.#v; } } new A().v;"), "3");
    assert.equal(run("let a = 0; a ||= 5; a;"), "5");
    assert.equal(run("new Map([['a', 1]]).get('a');"), "1");
    assert.equal(run("'a-b'.replace(/-/, '+');"), "a+b");
  });

  test("promise reactions run as microtasks", () => {
    assert.equal(
      run("let o = []; o.push('a'); Promise.resolve().then(() => o.push('c')); o.push('b'); await 0; o.join('');"),
      "abc",
    );
  });
  test("virtual clock polling resumes checkpoints without blocking", (t) => {
    if (!timersAvailable) { t.skip("runtime feature is not compiled"); return; }
    vm.set_clock("virtual");
    run("var scheduled=[];setTimeout(()=>{scheduled.push('timer');queueMicrotask(()=>scheduled.push('micro'));},10);");
    assert.equal(vm.poll_event_loop(10).executedJobs,0);
    assert.equal(vm.poll_event_loop(0).nextDeadline,10);
    vm.advance_clock(10);
    assert.equal(vm.poll_event_loop(1).checkpointPending,true);
    assert.equal(vm.run("scheduled.push('intruder')").ok,false);
    assert.equal(vm.poll_event_loop(1).executedJobs,1);
    assert.equal(run("scheduled.join(',')"),"timer,micro");
    assert.throws(()=>vm.advance_clock(-1));
    vm.set_clock("legacy");
  });
  test("real-time clock polling leaves future timers pending", async(t)=>{
    if (!timersAvailable) { t.skip("runtime feature is not compiled"); return; }
    vm.set_clock("real-time");
    run("var fired=false;setTimeout(()=>{fired=true},20)");
    const turn=vm.poll_event_loop(10);
    assert.equal(turn.executedJobs,0);assert.equal(turn.runnable,false);assert.ok(turn.nextDeadline>=20);
    await new Promise(r=>setTimeout(r,25));
    assert.equal(vm.poll_event_loop(1).executedJobs,1);
    assert.equal(run("fired"),"true");
    vm.set_clock("legacy");
  });

});
