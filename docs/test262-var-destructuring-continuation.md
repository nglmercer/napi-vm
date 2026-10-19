# Var destructuring reference order continuation

Ordinary var destructuring now uses prepared assignment references, matching
var destructuring loop bindings. Var names have already been hoisted; they
must resolve their target before reading a source property or applying its
default initializer. Previously this path used lexical initialization, which
resolved the target only when it wrote the value.

Regression tests compare prepared execution with raw AST execution. They check
computed source-key conversion before target resolution, target resolution
before a getter/default, retained with-object references after a source getter
deletes the property, abrupt target lookup before getter execution, lexical
binding isolation inside with, and array target resolution before iterator
steps.

The frozen baseline worker is the class-layout release
c3d28437e3b2bbcf8d14eab430c828099fc1a9b099d93113e51edfa742c0d82c,
whose implementation is archived in the class property layout evidence. It
passes 527 / 528 variants across language/destructuring,
language/statements/variable, and language/statements/with. The sole failure is
the Test262 keyed-destructuring var-binding lookup-order case. The fresh release passes **528 / 528**, gaining **one pass with zero losses**.
There are zero skips, timeouts, crashes, or harness errors. The reports,
comparison, worker identity, and implementation patch are archived in
[var destructuring evidence](../tools/test262/evidence/var-destructuring-2026-10-18/).
The latest completed full score remains 101,889 / 102,956 on 997b476; this
continuation is outside that measured snapshot.

All 17 destructuring integration tests and all 368 core-library tests pass,
with no exclusions. Strict workspace Clippy and formatting pass. The five
new regressions run both prepared and raw AST execution; with statements retain
their existing prepared AST fallback.

Latest full-run update (2026-10-19): the frozen a528b02 snapshot completed at
101,895 / 102,956 (98.9695%), with 951 failures and 110 timeouts. It gains
61 passes and loses 55 versus 997b476; all losses are measured timeouts.
Earlier full-run status statements above describe the historical stage.
See [archived full evidence](../tools/test262/evidence/full-empty-accessors-2026-10-19/).
