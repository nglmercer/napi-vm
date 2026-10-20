# TypedArray slice copy order

TypedArray slice now copies matching element types as raw bytes in forward
order. A species constructor can return a view overlapping the source buffer;
later reads must observe earlier writes rather than a pre-copy snapshot.
Different element types likewise read and convert each element as it is copied.
Copy loops retain execution-budget checks.

When a resizable source shrinks after start/end coercion, the copied count is
limited by its current length. Zero remaining bytes cause no buffer read, even
when the original start offset is beyond the resized buffer. The result still
has the length requested from the species constructor and its untouched entries
remain initialized by that constructor.

Two new regressions fail before the change and pass afterward. A raw NaN payload
control passes before and after, checking that matching types never round-trip
through a Number conversion. All 369 core tests and 13 related integration tests pass, together with strict
workspace Clippy and formatting. See the
[native evidence](../tools/test262/evidence/typed-array-slice-copy-2026-10-20/).
Release verification remains pending.
