# Public hash-named property continuation

Private class elements now use separate branded storage. The old shared
internal-key predicate still hid every public string beginning with #, so valid
public object properties and computed class fields disappeared from enumeration,
copying, and JSON output. The predicate now reserves only the existing symbol
and setter-companion slots; public hash-named strings remain ordinary keys.

Three regressions require bytecode and independently execute the AST. They
cover ordinary objects, object rest/spread/assign, for-in, JSON, array named
properties, and public/private class fields with matching names. All three pass.
The broad release selection passes **31,739 / 31,860** variants, gaining
**two passes with zero losses**. All 2,252 object-expression variants pass.
There are zero skips, timeouts, crashes, or harness errors. The remaining
121 failures include 38 parse failures and 83 runtime failures. Reports,
comparison, worker identity, and source patch are archived in
[public hash-key evidence](../tools/test262/evidence/public-hash-keys-2026-10-19/).
All 368 core-library tests and strict workspace Clippy pass. This change and
the later super receiver-order change are outside the latest completed full
snapshot on a528b02 (101,895 / 102,956).
