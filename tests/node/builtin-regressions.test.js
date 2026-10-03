const test = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { runInNewContext } = require("node:vm");
const path = require("node:path");
const { runCode } = require("../../index.js");

// Compare observable values and guest exception types against a separate Node
// realm. Each run gets fresh globals so mutations cannot contaminate a case.
function matchesNode(expression) {
  const source = `(() => {
    try { return String(${expression}); }
    catch (error) { return 'error:' + error.name; }
  })()`;
  assert.equal(runCode(source), runInNewContext(source), expression);
}

test("invalid parseInt radices cannot abort the host process", () => {
  execFileSync(process.execPath, ["-e", `
    const assert = require('node:assert/strict');
    const { runCode } = require('./index.js');
    for (const radix of [1, 37, -1, 100, 2147483647, 4294967295]) {
      assert.equal(runCode('Number.isNaN(parseInt("12", ' + radix + '))'), 'true');
    }
  `], { cwd: path.resolve(__dirname, "../.."), timeout: 15000, stdio: "pipe" });
});

test("recursive calls and constructors stop before exhausting the host stack", () => {
  execFileSync(process.execPath, ["-e", `
    const assert = require('node:assert/strict');
    const { Vm } = require('./index.js');
    for (const source of [
      'function f(){return f();} try{f();}catch(e){e.name;}',
      'function F(){new F();} try{new F();}catch(e){e.name;}',
    ]) {
      const vm = new Vm();
      assert.equal(vm.run(source), 'RangeError');
      assert.equal(vm.run('40 + 2'), '42');
    }
  `], { cwd: path.resolve(__dirname, "../.."), timeout: 15000, stdio: "pipe" });
});

test("fractional lastIndexOf positions cannot abort the host process", () => {
  execFileSync(process.execPath, ["-e", `
    const assert = require('node:assert/strict');
    const { runCode } = require('./index.js');
    assert.equal(runCode('[1,2,3].lastIndexOf(1, -0.5)'), '0');
    assert.equal(runCode('new Uint8Array([1,2,3]).lastIndexOf(1, -0.5)'), '0');
  `], { cwd: path.resolve(__dirname, "../.."), timeout: 15000, stdio: "pipe" });
});

const groups = {
  "function constructors propagate errors and accept omitted rest arguments": [
    '(() => { function F(){throw new Error("boom");} try{new F();}catch(e){return e.message;} })()',
    '(() => { function F(){try {throw 1;} catch(e){throw new Error("boom");}} try{new F();}catch(e){return e.message;} })()',
    '(() => { function F(){return {x:3};} return new F().x; })()',
    '(() => { function F(){this.x=3;return 1;} return new F().x; })()',
    '(() => { function F(a,b,...rest){this.n=rest.length;} return new F().n; })()',
    '(() => { function f(a,b,...rest){return rest.length;} return f(); })()',
  ],
  "parseInt handles prefixes, wrapping radices, large integers and signed zero": [
    'parseInt("0xff", 16)', 'parseInt("-0Xf", 16)',
    'parseInt("12", 4294967306)', 'parseInt("12", -4294967286)',
    'parseInt("12", Infinity)', 'parseInt("12", NaN)',
    'parseInt("123456789012345678901234567890") === 1.2345678901234568e29',
    'parseInt("9".repeat(400))', 'Object.is(parseInt("-0"), -0)',
    'parseInt("11", {valueOf() { return 2; }})',
    'parseInt("12", Symbol())', 'parseInt("12", 10n)',
  ],
  "parseFloat accepts the longest valid decimal prefix": [
    'parseFloat(".5")', 'parseFloat("-.5rest")',
    'parseFloat("1.2.3")', 'parseFloat("1e+2rest")',
    'parseFloat("1e-2")', 'parseFloat("1e")', 'parseFloat("1e+")',
    'parseFloat("Infinityrest")', 'parseFloat("-Infinity")',
    'parseFloat("+Infinity")', 'parseFloat(".")', 'parseFloat("0x10")',
    'Object.is(parseFloat("-0"), -0)',
  ],
  "Math preserves signed zero and floating point boundaries": [
    'Object.is(Math.round(-0.1), -0)', 'Object.is(Math.round(-0.5), -0)',
    'Object.is(Math.round(-0), -0)', 'Math.round(0.49999999999999994)',
    'Math.round(4503599627370497)', 'Math.round(-1.5)',
    'Object.is(Math.sign(-0), -0)', 'Object.is(Math.min(0, -0), -0)',
    'Object.is(Math.max(-0, 0), 0)',
    'Number.isFinite(Math.hypot(1e308, 1e308))',
    'Math.hypot(1e-300, 1e-300) > 1e-300',
    'Math.hypot(Infinity, NaN)', 'Math.hypot(NaN, Infinity)', 'Math.hypot()',
    'Math.floor({valueOf() {return 2.7;}})', 'Math.abs(1n)', 'Math.abs(Symbol())',
  ],
  "bitwise operators wrap numbers modulo 2 to the power of 32": [
    '4294967295 | 0', '4294967296 | 0', '-4294967297 | 0',
    '4294967295 >>> 0', 'Infinity | 0', '-Infinity | 0',
    '4294967297 & 3', '4294967297 ^ 3', '~4294967296',
    '1 << 4294967296', '16 >> 4294967297', '16 >>> 4294967297',
    '"4294967295" | 0', '"4294967295" >>> 0',
  ],
  "array indices truncate before applying relative offsets": [
    '[1,2,3].lastIndexOf(1, -0.5)', '[1,2,3].lastIndexOf(3, -1.5)',
    '[1,2,3].at(NaN)', '[1,2,3].at(-0.5)', '[1,2,3].at(-1.5)',
    '[1,2,3].at(-Infinity)', '[1,2,3].slice(1, undefined).join()',
    '[1,2,3].splice().join()', '[1,2,3].splice(1, Infinity).join()',
    '[1,2,3].splice(-1.5, 1).join()', '[1,2,3].fill(0, -1.5).join()',
    'JSON.stringify([1,[2,[3]]].flat(0.5))',
    'JSON.stringify([1,[2,[3]]].flat(1.5))',
    'JSON.stringify([1,[2,[3]]].flat(undefined))',
  ],
  "typed array indices and bounds cannot overflow": [
    'new Uint8Array([1,2,3]).at(-0.5)', 'new Uint8Array([1,2,3]).at(-1.5)',
    'new Uint8Array([1,2,3]).slice(-1.5).join()',
    'new Uint8Array([1,2,3]).fill(0, -1.5).join()',
    '(() => {const a=new Uint8Array(2);a.set([1],NaN);return a.join();})()',
    '(() => {const a=new Uint8Array(2);a.set([1],-0.5);return a.join();})()',
    'new Uint8Array(2).set([1], 1e30)',
    'new DataView(new ArrayBuffer(2),1,Infinity)',
  ],
  "array callbacks validate even when there are no elements": [
    ...["map", "filter", "forEach", "some", "every", "find", "findIndex",
      "findLast", "findLastIndex", "flatMap"].map(method => `[].${method}(null)`),
    '[].reduce(null, 1)', '[].reduceRight(null, 1)', '[].sort(null)',
    '[1].sort(123)', 'Array.from([], null)', 'Array.from([], 1)',
  ],
  "array callbacks honor thisArg and read elements during iteration": [
    '[1].map(function(x){return x + this.n;}, {n:2}).join()',
    '[1,2].filter(function(x){return x > this.n;}, {n:1}).join()',
    '[1].some(function(x){return x === this.n;}, {n:1})',
    '[1].every(function(x){return x === this.n;}, {n:1})',
    '[1].find(function(x){return x === this.n;}, {n:1})',
    '[1].findIndex(function(x){return x === this.n;}, {n:1})',
    '[1].findLast(function(x){return x === this.n;}, {n:1})',
    '[1].findLastIndex(function(x){return x === this.n;}, {n:1})',
    '[1].flatMap(function(x){return [x + this.n];}, {n:2}).join()',
    'Array.from([1], function(x){return x + this.n;}, {n:2}).join()',
    '(() => { const out=[]; [1].forEach(function(x){out.push(x + this.n);}, {n:2}); return out.join(); })()',
    '(() => { const a=[1,2,3]; return a.map((x,i)=>{a[1]=9; return x;}).join(); })()',
    '(() => { const a=[1,2]; return a.map((x,i)=>{delete a[1]; return x;}).join(); })()',
    '(() => { const a=[1,,3]; return a.map((x,i)=>{a[1]=2; return x;}).join(); })()',
    '(() => { const a=[1,2]; return a.map((x,i)=>{a.push(3); return x;}).join(); })()',
    '(() => { const a=[1,2,3]; return a.reduce((s,x,i)=>{a[2]=9; return s+x;},0); })()',
    '(() => { const a=[1,2,3]; return a.reduceRight((s,x,i)=>{a[0]=9; return s+x;},0); })()',
    'Array.prototype.map.call({0:3,1:4,length:2}, x=>x*2).join()',
    'Array.prototype.map.call(null, x=>x)',
  ],
  "array iterators observe mutations and remain exhausted": [
    ...["keys", "values", "entries"].map(method => `(() => {
      const a=[1,2]; const it=a.${method}(); it.next(); a[1]=9; a.push(3);
      return JSON.stringify([it.next(), it.next(), it.next()]);
    })()`),
    ...["keys", "values", "entries"].map(method => `(() => {
      const a=[1]; const it=a.${method}(); it.next(); it.next(); a.push(2);
      return it.next().done;
    })()`),
  ],
  "Array.from maps while reading its source and closes failed iterators": [
    'Array.from(1).length', 'Array.from(true).length',
    '(() => {const a=[1,2]; return Array.from(a,(x,i)=>{a[1]=9; return x;}).join();})()',
    '(() => {const a={0:1,1:2,length:2}; return Array.from(a,(x,i)=>{a[1]=9; return x;}).join();})()',
    '(() => {const a=[1,2]; a[Symbol.iterator]=function*(){yield 9;}; return Array.from(a).join();})()',
    '(() => {const seen=[];const source={[Symbol.iterator](){let i=0;return{next(){if(i===2)return{done:true};seen.push("next");return{value:++i,done:false};}};}};Array.from(source,x=>{seen.push("map");return x;});return seen.join();})()',
    '(() => {let closed=false;const source={[Symbol.iterator](){return{next(){return{value:1,done:false};},return(){closed=true;return{done:true};}};}};try{Array.from(source,()=>{throw new Error("x");});}catch(e){}return closed;})()',
    '(() => {const source={[Symbol.iterator](){return{next(){return{value:1,done:false};},return(){throw new Error("close");}};}};try{Array.from(source,()=>{throw new Error("map");});}catch(e){return e.message;}})()',
    '(() => {let closed=false;function* g(){try{yield 1;}finally{closed=true;}}try{Array.from(g(),()=>{throw new Error("x");});}catch(e){}return closed;})()',
  ],
  "string methods normalize positions and omitted arguments": [
    '"abc".indexOf("a", -1)', '"abc".includes("a", -1)',
    '"abc".indexOf()', '"abc".includes()', '"abc".startsWith()',
    '"abc".endsWith()', '"abc".lastIndexOf()',
    '"abc".at(NaN)', '"abc".at(-0.5)', '"abc".at(-1.5)',
    '"abc".slice(1, undefined)', '"abc".substring(1, undefined)',
    '"abc".charAt(-1)', '"abc".charCodeAt(-1)',
    '"abc".charAt(-0.5)', '"abc".endsWith("bc", undefined)',
    '"abc".repeat(-1)', '"abc".repeat(-0.5)', '"".repeat(Infinity)',
    '"a,b".split(",", undefined).join("|")',
    '"a,b".split(",", -1).join("|")',
    '"a1b1c".split(1).join("|")', '"anullb".split(null).join("|")',
    '"abc".split(undefined, 0).length', '"a,b".split(/,/, -1).join("|")',
    '"abc".codePointAt(NaN)', '"abc".codePointAt(-0.5)',
  ],
};

for (const [name, expressions] of Object.entries(groups)) {
  test(name, () => {
    for (const expression of expressions) matchesNode(expression);
  });
}

test('Array.from generator side effects follow the platform generator backend', () => {
  // Cargo scopes corosensei to supported targets. Windows ARM64 uses the
  // documented buffered fallback: the body runs fully on its first next().
  // Ordinary iterators above must still interleave reads and mapping everywhere.
  const buffered = process.platform === 'win32' && process.arch === 'arm64';
  const cases = [
    ['(() => {const seen=[]; function* g(){seen.push("next");yield 1;seen.push("next");yield 2;} Array.from(g(),x=>{seen.push("map");return x;});return seen.join();})()', 'next,next,map,map'],
    ['(() => {const seen=[];function* g(){try{seen.push("next");yield 1;seen.push("next");yield 2;}finally{seen.push("close");}}try{Array.from(g(),()=>{seen.push("map");throw new Error("x");});}catch(e){}return seen.join();})()', 'next,next,close,map'],
  ];
  for (const [expression, bufferedResult] of cases) {
    if (buffered) assert.equal(runCode(expression), bufferedResult, expression);
    else matchesNode(expression);
  }
});
