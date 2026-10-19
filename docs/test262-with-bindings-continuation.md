# With and Proxy binding continuation

Identifier references in `with` environments now resolve once and retain their
object base through reads, assignments, updates, calls, optional calls, and
tags. `HasBinding` performs the Proxy `has` and `Symbol.unscopables` checks;
`GetBindingValue` then performs its own existence check and property read. A
binding removed by an unscopables getter does not fall through to an outer
binding. Sloppy assignment can recreate a deleted property, while strict reads
still throw.

Calls preserve the with object as the receiver, including verified bytecode,
`globalThis`, optional calls, template tags, and user-defined functions named
`eval`. Assignment RHS evaluation cannot redirect the captured base. Proxy
assignment follows the required `set`, descriptor, and defineProperty sequence.

The release selection passes **14,466 / 14,502** variants, gaining **6 passes
with zero losses**. It includes the earlier control-flow, arguments, generator,
Proxy, eval, call/apply, and descriptor selections. Twelve focused integration
tests and 79 selected bytecode tests pass; strict Clippy, formatting, realm,
tail-call, lexical, mapped-arguments, for-in, and finally suites pass. The
known unrelated super-destructuring parity failure remains excluded.

Evidence is archived under [`with-bindings-2026-10-18`](../tools/test262/evidence/with-bindings-2026-10-18/).
The completed full snapshot remains 101,728 / 102,956 on source 607d086;
with-binding changes are outside that full run.
