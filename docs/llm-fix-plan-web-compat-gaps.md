# LLM fix plan: web-compat gaps blocking guest DOM libraries

> For an LLM coding agent. Goal: fix the verified spec gaps below so real-world
> pure-JS libraries (linkedom, `entities`, `css-select`) can execute as guest
> code. Each issue has runnable repros, exact fix pointers, and acceptance
> criteria. Fix in priority order; verify each issue before moving on.
>
> Line numbers were accurate at writing time but the tree has concurrent
> uncommitted work — **re-grep for the cited symbols** instead of trusting
> line numbers.

## 0. Repo context

- `napi-vm`: sandboxed JS interpreter written in Rust, exposed to Node via
  N-API (`index.js` + `napi-vm.*.node`). Guest builtins live in `src/builtins/`,
  parser in `src/parser/`, interpreter in `src/interpreter/`.
- **After any Rust change you MUST rebuild the binding** before Bun tests see
  it: `bun run build` (`npx napi build --platform --release --js index.js`).
- Test/verify commands:
  - `bun test tests/` — full JS suite (must stay green except pre-existing
    failures; note them, don't fix unrelated ones).
  - `cargo test --release` — Rust suite.
  - Gates (CI): `cargo fmt --all -- --check`,
    `cargo clippy --all-targets --all-features -- -D warnings`,
    `npx tsc --noEmit`.
- Quick repro harness (run from repo root, no files needed):
  `bun -e 'import { Vm } from "./index.js"; const vm = new Vm(); console.log(vm.run("EXPR;")); vm.dispose();'`
- Do NOT weaken the sandbox: these are pure-semantics fixes, no new host reach.
- Add regression tests: extend `tests/builtins.test.js` /
  `tests/array-methods.test.js`, or add `tests/web-compat.test.js`. Add Rust
  unit tests next to existing parser/builtin tests where natural.

## 1. P0 — `fromIndex` / position arguments ignored everywhere

**Impact:** breaks `entities` startup (its encode-trie parser passes a cursor to
`indexOf`; the cursor never advances, a table grows unboundedly, guest dies
with `RangeError: Maximum array length exceeded`). Also silently corrupts any
`lastIndexOf(x, from)` logic (e.g. linkedom's `removeSubsets`).

**Verified broken (actual → want):**

| Expression | Got | Want |
|---|---|---|
| `[1,2,1,2].indexOf(2, 2)` | `1` | `3` |
| `[1,2,1].lastIndexOf(1, 1)` | `2` | `1` |
| `[1,2,3].includes(1, 1)` | `true` | `false` |
| `"abcabc".indexOf("b", 2)` | `1` | `4` |
| `"abcabc".lastIndexOf("b", 3)` | `4` | `1` |
| `"abc".includes("b", 2)` | `true` | `false` |
| `"abc".startsWith("b", 1)` | `false` | `true` |
| `"abc".endsWith("b", 2)` | `false` | `true` |
| `new Uint8Array([1,2,1]).indexOf(1, 1)` | `0` | `2` |

**Fix pointers:**

- `src/builtins/string.rs`: `string_index_of` (~line 385, only reads
  `a.first()`, uses `s.find` from 0), `string_last_index_of`,
  `string_includes`, `string_starts_with`, `string_ends_with`. Audit every
  `string_*` function that takes a position/length argument.
- `src/builtins/array.rs`: `array_index_of` (~line 238 dispatch),
  `array_last_index_of` (~line 402, loops full range), `array_includes`.
  Check `copyWithin`/`fill` too (same argument-shape family).
- `src/builtins/typedarray.rs`: `typed_index_of` (delegated via
  `typed_delegate_method!` ~line 1109) and siblings (`lastIndexOf`,
  `includes` if present).

**Spec notes:** implement ECMA-262 conversions: `ToIntegerOrInfinity` on the
position, clamp negatives (`max(len + n, 0)` for indexOf/includes;
`lastIndexOf` clamps to `len - 1`, negative → `-1`), `undefined` → default
(`0`, or `len` for `lastIndexOf`/`endsWith`).

**Acceptance:** all 9 repros above return the `Want` column; add Bun tests for
each including negative/omitted/out-of-range positions; full suites green.

## 2. P0 — `Set` / `Map` / `WeakMap` / `WeakSet` have no `.prototype`

**Impact:** breaks `class DOMTokenList extends Set`, `const { add } =
Set.prototype`, and any `Xxx.prototype.method` reference (pervasive in real
libraries). Verified: `typeof Set.prototype` → `"undefined"` (same for `Map`,
`WeakMap`, `WeakSet`); every other builtin (`Array`, `Object`, `Promise`,
`String`, `Date`, `RegExp`, `Error`, `Function`, typed arrays, …) correctly
exposes `.prototype`.

**Repro:**

```js
typeof Set.prototype;              // "undefined", want "object"
const { add } = Set.prototype;     // throws, want a function
```

**Fix pointers:**

- `src/builtins/collections.rs` — wire `.prototype` the way `Array` does
  (`src/builtins/array.rs` ~line 90: `a.set_prop("prototype".into(),
  prototype)`), following the same pattern as `object.rs` (~line 81),
  `function.rs` (~line 116), `error.rs` (~line 52).
- Native instances already work (`new Set().add(1)` → size `1`), and
  `Object.getPrototypeOf(new Set()).add` is already a function — only the
  constructor's `.prototype` link is missing. Reuse the existing hidden proto
  object if possible; do not fork method implementations.

**Also verify (same fix area):** `class X extends Set { constructor() {
super(); } }` + `new X()` + inherited `.add/.has/.size` work, i.e. native
brand checks accept subclass instances. Same for `Map`. If subclassing is
broken beyond `.prototype`, fix it and note it in the final summary.

**Acceptance:** `typeof` for all four is `"object"`; destructure + `.call`
works; subclass smoke test passes; regression tests added.

## 3. P1 — computed keys in destructuring patterns are syntax errors

**Impact:** linkedom (and much modern code) uses `let {[NEXT]: next, [END]:
end} = x` (~31 occurrences). Parser rejects with `` expected `RBrace`, found
`LBracket` ``.

**Repro:** `vm.validateModule("const {[k]: v} = o;")` → invalid; want valid.
Also cover: arrow params `({[k]: v}) => v`, function params, `for (const {[k]:
v} of xs)`, nested patterns, defaults `({[k]: v = 1} = o)`, mixed
`({[P]: a, plain, ...rest} = o)`.

**Fix pointers:**

- `src/parser/stmt.rs`, `pattern()` object-pattern branch (~line 214): `let
  key = self.ident()?` only accepts identifiers. Add a `Token::LBracket`
  arm that parses a full key expression, plus string/numeric literal keys
  (same gap, same line).
- `Pattern` AST in `src/parser/ast.rs` stores `(String, Option<Pattern>)`
  pairs — computed keys need a representation change (e.g. key expression
  variant). Update ALL consumers: evaluation (`src/interpreter/`, search for
  destructuring evaluation), `stmts_reference` in `ast.rs`, and the
  `src/lang/` (LSP) walkers that match on `Pattern` (compile will point at
  them).
- `pattern()` is shared by declarations, arrow params (`primary.rs` ~line
  445), function params (`stmt.rs` ~line 576), and loop heads (`stmt.rs`
  ~line 385) — one fix covers all; verify each with a parse+eval test.

**Acceptance:** all repro shapes parse AND evaluate correctly
(`const {[\"a\"+\"b\"]: v} = {ab: 42}; v;` → `42`); existing suites green.

## 4. P1 — computed (and literal) class member names are syntax errors

**Impact:** `[Symbol.iterator]() {}`, `get[PRIVATE]() {}`, `set[K](v) {}`
rejected with `` unexpected token `LBracket` ``. Same file family as issue 3.

**Repro:** `vm.validateModule("class A { [Symbol.iterator]() {} }")` →
invalid; want valid. Also: computed getters/setters, computed fields,
`static [K]() {}`, `async [K]() {}`, `*[K]() {}`, string names
`class A { \"m\"() {} }`, numeric names `class A { 0() {} }`.

**Fix pointers:**

- `src/parser/compound.rs`, class-body member-name `match` (~lines 96–139):
  handles `Identifier`/keywords/`#private` only. Add `Token::LBracket`
  (parse key expression, expect `RBracket`), `Token::String`, and numeric
  literal arms. Note `static`/`async`/`get`/`set` modifiers are already
  parsed before the name — they compose with the new arms.
- `ClassMember::{Method, Getter, Setter, Field}` in `src/parser/ast.rs` hold
  `name: String` — computed keys need an AST variant (key expression).
  Update evaluation: keys evaluate once, in definition order, at class
  definition evaluation (after heritage is resolved, per spec
  ClassDefinitionEvaluation). Update `stmts_reference` and `src/lang/`
  walkers that match on `ClassMember`.
- Private `#` members already work — do not regress them.

**Acceptance:** all repro shapes parse and behave (`class A { [\"m\"]() {
return 1; } } new A().m();` → `1`; `[Symbol.iterator]` makes instances
iterable); regression tests added.

## 5. P2 — `Object.defineProperty` wrongly rejects redefining non-configurable props

**Impact:** breaks Babel-transpiled classes (the standard fallback for issues
3–4): `_createClass` ends with `Object.defineProperty(Ctor, "prototype", {
writable: false })`, which throws `TypeError: Cannot redefine property:
prototype`. Per spec this call is legal (writable `true`→`false` with same
value on a non-configurable property).

**Repro:**

```js
function C() {}
Object.defineProperty(C, "prototype", { writable: false }); // throws, want C
C.prototype; // still the original prototype object
```

**Fix pointers:**

- `src/builtins/object.rs` ~lines 957–959: `if existing && !configurable →
  throw` fires before any attribute comparison. Replace with the nuanced
  ValidateAndApplyPropertyDescriptor check the array path already has
  (~lines 1188–1228): allow when configurable, or when only narrowing
  `writable: true`→`false` with same value and unchanged
  enumerable/configurable/getter/setter; reject the rest with the same error
  type/message shape. Unify the two paths if feasible without behavior drift.
- Check `Object.defineProperties` and `Reflect.defineProperty` share the same
  helper (fix once if so; verify both).

**Acceptance:** repro passes; a Babel-ie11-compiled `class A { m() { return
1; } }` (helpers `_createClass`/`_defineProperties`/`_classCallCheck`
inlined) defines and `new A().m()` → `1`; negative cases still throw
(changing value of non-writable+non-configurable, flipping enumerable,
widening writable `false`→`true`).

## 6. P3 (optional, end-to-end) — prove linkedom loads as guest code

Only after issues 1–5. Do this in scratch dirs, NEVER in the repo tree:

1. `mkdir /tmp/domtest && cd /tmp/domtest && npm install linkedom`.
2. `cssom` is CJS-only and `linkedom/commonjs/canvas.cjs` needs native
   `canvas`: stub them. Replace `node_modules/cssom` with
   `{"name":"cssom","version":"0.0.0-stub","type":"module","exports":{".":"./index.js"}}`
   + `export function parse() { throw new Error("stub"); }`, and point
   `esm/html/canvas-element.js`'s `canvas.cjs` import at an ESM port of
   `commonjs/canvas-shim.cjs`.
3. Load with the repo's `GuestPackageLoader` (`rootDir: "/tmp/domtest"`,
   `compilerMode: "swc"` + a Babel preset-env `modules: false` compiler —
   see prior probe scripts if present on this machine at
   `~/domprobe-scratch/domtest/`).
4. Expect: `import { parseHTML } from "linkedom"; const { document } =
   parseHTML("<div id=x>hi</div>"); document.querySelector("#x").textContent;`
   → `"hi"`.

If it still fails, bisect with per-module imports in fresh `Vm`s (import each
canonical `/npm/…` id; deps-first order finds the culprit in one pass) and
append the new gap to this plan. Do NOT vendor linkedom or the stubs into the
repo.

## 7. P3 follow-ups — gaps found while proving linkedom (all fixed)

Bisecting the §6 proof surfaced four more spec gaps. Each is fixed, covered
in `tests/web-compat.test.js`, and verified by the green proof
(`RESULT "hi!|1"` for the §6 query plus `querySelectorAll` count).

### 7a. Object destructuring read only own data slots

`const { document } = parseHTML(...)` yielded `undefined` while
`r.document` worked: `destructure`'s `Pattern::Object` arm
(`src/interpreter/call.rs`) scanned the own-props vec, bypassing getters
(leaked the getter function), the prototype chain, and proxies (all
`undefined`). Destructuring `null`/`undefined` also silently yielded
`undefined` instead of throwing.

Fix: named keys read via `get_prop_value` (the normal member path);
`{ ...rest }` copies own *enumerable* keys via Get (getters run,
non-enumerables excluded); nullish bases throw
`TypeError: Cannot destructure properties of …`.

### 7b. Same-type loose equality never matched (`null == null` → `false`)

`leq` (`src/interpreter/ops.rs`) had no `(Null, Null)` /
`(Undefined, Undefined)` arms, so css-select's
`selector.namespace != null` misfired and tag selectors threw
`Namespaced tag names are not yet supported`. Same-reference objects also
compared `false`.

Fix: when both operands share a discriminant, `leq` delegates to
`strict_equals` (Abstract Equality Comparison step 1); mixed-type
coercion arms unchanged.

### 7c. Function bodies leaked the last statement's value as the result

`call_this` (`src/interpreter/call.rs`) returned `run_program_body`'s
`Ok(value)` on fall-off-the-end, so `new Tokenizer(...)` returned the
inner `EntityDecoder` (the constructor's trailing assignment) and
htmlparser2 tokenized nothing. Concise arrows were already wrapped in
`Return`, and `super()` discards the value — both unaffected.

Fix: fall-off-the-end yields `undefined`; only explicit `return`
(`VmErr::Ret`) produces a value. Class construction then keeps the
instance unless the constructor explicitly returns an object.

### 7d. `Array.prototype.push` / `pop` were not generic

Both returned `undefined` for non-`Array` receivers, so
`class NodeList extends Array` (a plain object in this VM) never
accumulated `querySelectorAll` matches (`length` stayed `undefined`).

Fix: generic paths per ECMA-262 (`LengthOfArrayLike` via Get/`ToNumber`,
clamped to `MAX_ARRAY_LEN`; writes via `assign_member`, removal via
`delete_member`; `TypeError` on nullish receivers). Real arrays keep the
fast path.

## Explicitly out of scope

- **happy-dom**: imports Node builtins (`fs`, `vm`, `child_process`,
  `http/https`, `crypto`, …) plus `ws`. It cannot run as guest code. Do not
  chase it; linkedom is the target.
- Host-bridging live DOM objects is architecturally impossible (marshal layer
  drops functions, rejects cycles — see `src/bindings/marshal.rs`,
  `from_napi_d`). The DOM must execute as guest JS.
- Do not fix unrelated failing tests; report pre-existing failures as-is.
- `Generator`/`AsyncFunction` constructor globals intentionally absent (matches
  V8) — not a bug.
