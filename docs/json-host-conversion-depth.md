# Host JSON conversion depth

Host JSON input conversion now uses an explicit work stack instead of recursive
Rust calls. The depth check can therefore return its RangeError without first
exhausting a small host thread stack. The existing maximum depth (512), array
length, object property count, and string length checks remain unchanged.
Traversal and property-key validation retain their original order.

The work stack visits values and keys and then assembles each completed array
or object from its child values. Completed nested values retain their original
property order and ordinary object attributes.

The formerly crashing excessive-depth regression passes. A second regression
uses a 128 KiB thread stack, accepts a mixed array/object input exactly at the
depth limit, checks its leaf value, and rejects input one level beyond the limit.
All seven conversion tests and strict workspace Clippy pass. The complete
core-library gate passes **368 tests with zero exclusions**.

This change concerns host JSON input conversion. Guest JSON builtins and host
JSON output conversion retain their existing implementations. It does not
change the reported Test262 score.
