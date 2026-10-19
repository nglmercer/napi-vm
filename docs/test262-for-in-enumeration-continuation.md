# Descriptor-aware for-in enumeration

## Implementation

AST and bytecode share a private enumeration cursor. Each object snapshots its
own keys when reached, then checks descriptors one at a time as execution
advances. Deleted properties are skipped. A missing descriptor does not shadow
an inherited property; an existing nonenumerable property does. Symbols never
cause descriptor queries, and enumeration never reads property values or calls
Proxy has traps.

Prototype traversal happens after the current object's keys are exhausted and
uses the Proxy-aware internal prototype operation. Breaking early avoids later
descriptor and prototype queries. Prototype traversal and skipped keys consume
instruction fuel and enforce the existing prototype-depth limit.

Bytecode holds cursors in a private frame stack. Loop exits release the crossed
cursors, while exception handlers restore their recorded cursor depth. An inner
catch preserves the enclosing loop's cursor. Continue preserves its target
loop's cursor, including when finally cleanup intervenes. Node-API's separate
own-key helper remains available only in host builds; its caller handles
prototype traversal itself.

## Validation

Eleven new integration tests require verified bytecode and also execute AST.
They cover deletion, changing enumerability, nonenumerable shadowing, inherited
keys, symbol filtering, Proxy trap order, early break, descriptor errors,
nullish and primitive sources, and nested cleanup across catches and labels.
The five selected integration suites pass 52 tests; all 79 selected bytecode
tests pass with the existing unrelated super-destructuring parity failure
excluded. Strict workspace Clippy and formatting pass. Strict Clippy also passes with the Node-API-host feature enabled.

The Test262 selection covers thirteen control-flow directories, object and
typeof expressions, and Proxy builtins. The release worker passes **6,498 / 6,515** variants,
up from 6,496 / 6,515, with **2 gains and zero losses**.
All 206 selected for-in variants pass. The selection retains seventeen
failures, with zero skips, timeouts, crashes, or harness errors.
Reports, comparisons, worker identity, and the source patch are archived under
[for-in evidence](../tools/test262/evidence/for-in-enumeration-2026-10-18/). The full score measures 101,889 / 102,956 on 997b476; this feature is included in that measured full snapshot.

Latest full-run update (2026-10-19): the frozen a528b02 snapshot completed at
101,895 / 102,956 (98.9695%), with 951 failures and 110 timeouts. It gains
61 passes and loses 55 versus 997b476; all losses are measured timeouts.
Earlier full-run status statements above describe the historical stage.
See [archived full evidence](../tools/test262/evidence/full-empty-accessors-2026-10-19/).
