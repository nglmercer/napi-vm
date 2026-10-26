# Float16Array support — 2026-10-26

5,505 / 5,519 variants pass: four gains and zero losses against the matching
complete-report projection. Both sort-negative-nan.js and toString.js pass
in script/strict modes. All fourteen remaining variants were already timeouts.
The selection includes all TypedArray, DataView, Atomics, ArrayBuffer, selected
integrity APIs, SpiderMonkey TypedArray, and TypedArray subclass tests.

Float16Array uses the existing binary16 conversion helpers for two-byte
storage, shared views, generic methods, and cross-realm construction. Native
bridges recognize the Float16 type and Node-API tag 11. Validation passed 390
core tests, five worker tests, all-target Clippy, the root all-target check,
and formatting. Source identity, reports, and comparison are retained here.
These focused gains do not replace the latest full score, 102,783 / 102,956.
