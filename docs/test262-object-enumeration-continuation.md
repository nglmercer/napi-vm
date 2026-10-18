# Object enumeration continuation

Historical implementation checkpoint. Its measured full-suite counts below
are superseded by [latest.json](../tools/test262/latest.json).

The full pinned corpus now passes 66,894 / 102,956 variants (64.9734%):
36,060 failures, two timeouts, and no skips, crashes, or harness errors.
Compared with the preceding object-copy full run, 68 variants newly pass and
no previously passing variant fails. Full conformance remains incomplete.

## Implemented behavior

Object.keys, Object.values, and Object.entries box primitive inputs, reject
nullish inputs, snapshot own keys, and check each string property's current
descriptor before reading its value. Symbols are excluded before descriptor
traps. Getter side effects therefore affect subsequent descriptor checks,
while newly added keys remain outside the snapshot. Ordinary objects use
numeric index ordering; Proxy objects retain their ownKeys trap ordering.

Object.getOwnPropertyNames uses the shared own-key machinery.
Object.getOwnPropertyDescriptors boxes inputs, preserves symbol properties,
omits properties removed during descriptor enumeration, and produces public
descriptor records without invoking property getters. Internal own-name
enumeration also handles boxed strings. Inherited for-in enumeration remains
unfinished.

## Verification and reproducibility

Eight new integration tests compare forced verified bytecode execution with
raw AST execution. The six array-elision and eleven object-copy integration
tests also pass, together with four engine-boundary tests, seventeen worker
tests, and twelve Python tooling tests. The minimal core suite passes 354
tests with the known native-addon sidecar fixture explicitly excluded.
Workspace Clippy with all targets/features and `-D warnings`, formatting,
and whitespace checks pass. This is not a passing workspace test result:
the previously observed five native-addon failures remain unresolved.

The focused Object/Reflect, for-in, object-expression, assignment-destructuring,
and dynamic-import selection passes 10,449 / 12,106 variants, gaining 58
passes with zero losses versus the matching projection of the prior full run.
The projection is a comparison artifact, not a separate baseline execution.

Full execution uses corpus revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, four workers, a five-second
per-variant timeout, no feature skips, and the existing worker budgets:
1,000,000 fuel, 100,000 loop iterations, call depth 128, and 10,000 jobs.
The frozen release worker SHA-256 is
`69b9657c517ca3d5d6a57fdc15364d1af4fa9706c2c0d8cf273380a85f817a11`.
Source is a working-tree snapshot based on
`9e4f017ee76ce538d9ac076fa180ac6d12ddd104`; the archived source patch SHA-256
is `37415b38ef91784f4dfb89d0ea39431f9152f2e9561bd19a1c4a78845de3a1da`.
The patch was frozen before this checkpoint's evidence-only documentation
updates.

Individual outcomes, full/focused comparisons, triage, logs, source patch,
configuration, and compressed-artifact hashes are retained in
[the evidence summary](../tools/test262/evidence/object-enumeration-continuation/summary.json).
`tools/test262/latest.json` records this actual full execution.

## Remaining work

The two timeouts are the script and strict variants of
`staging/sm/regress/regress-1507322-deep-weakmap.js`.
Large failing families include Temporal (13,268 variants across built-ins
and Intl), RegExp, classes, Iterator, Array, TypedArray, and Promise.
Environment, descriptor/accessor storage, Proxy/exotic, private-brand,
agent/shared-memory, realm, and declaration semantics still require work.
Foundation phases 1–3 and phase 4 bytecode coverage remain incomplete;
full-corpus AST/bytecode differential validation has not been measured.
