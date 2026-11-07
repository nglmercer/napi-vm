# Retained ownership validation

This historical capture records commands, logs, build inputs, and machine
context for the implementation campaign identified by these JSON files.

Start with [current-results.json](current-results.json),
[current-build-metadata.json](current-build-metadata.json), and
[machine-context.json](machine-context.json). [miri-results.json](miri-results.json)
and [remote-gate.json](remote-gate.json) describe their separate validation
boundaries. The `prior-head/` data belongs to the explicitly recorded older
source; do not merge those outcomes with the current-build entries.

A successful historical check applies to that command, binary, source, and
platform. It is not proof that every later revision or target passed.
See [current validation guidance](../../validation.md) and
[architecture/ownership](../../architecture.md#ownership-and-collection).
