# Capture names use Unicode identifier escapes

Capture names and named backreferences decode Unicode identifier escapes even
when a pattern lacks u or v flags. Capture-name decoding now uses strict Unicode
escape rules independently of ordinary pattern escapes. Braced code points and
surrogate-pair names work in legacy patterns, while malformed identifier escapes
remain syntax errors. Ordinary legacy identity escapes remain supported.

This corrects the two regressions retained in the original String/regexp release
comparison. The new valid-name regression fails before the correction, while the
invalid-name control passes before and after. Both tests, six String tests, six
legacy regexp tests, all 369 core tests, strict workspace Clippy, and formatting
pass. The debug worker passes **2,445 / 2,445** selected String and previously
regressed capture-name variants, with no other outcomes.

See the [evidence](../tools/test262/evidence/regexp-group-name-escapes-2026-10-20/).
A detached-checkout optimized rerun is pending; the original release report is
retained unchanged and this debug result is not added to the full score.
