# Harness error classification

The Test262 worker now recognizes thrown Test262Error instances by the identity
of the harness constructor's prototype, captured before test execution. The
harness deliberately supplies no name property, so the previous name/message
classification reported these exceptions as EngineError.

Reporting follows existing prototype links without invoking guest getters,
Symbol.hasInstance, or stringification. An unrelated constructor with the same
name does not qualify. The captured prototype also remains independent of
subsequent global constructor replacement.

The regression covers scripts, modules, default-export expressions, async DONE
errors, and an inherited harness-error prototype. It fails before the change
and passes after it. This corrects measurement rather than implementing a new
ECMAScript feature. All 19 worker integration tests, strict workspace Clippy, and formatting pass.
The release comparison passes **2,621 / 2,637** selected variants, with **two
corrected passes and zero losses**. See the [archived evidence](../tools/test262/evidence/harness-error-2026-10-20/summary.json). The running full snapshot ffc10c7 excludes this
worker correction; its measured outcomes will be retained as produced.
