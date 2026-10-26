# Primitive conversion and array length follow-up — 2026-10-26

The pinned neighboring selection passes **18,247 / 18,247** variants, gaining
**three passes and zero losses** against the latest complete-report projection.
The gains are both staging Array ToLength variants and the staging object
ToPrimitive variant. The standard six-variant timing slice passes. The latest
complete score remains **102,839 / 102,956** until another full run confirms
these focused gains.

Template interpolations use abstract ToString, including Symbol errors, and
perform conversion before evaluating the next interpolation in both tiers.
Date default conversion uses ordinary numeric preference when its exotic
primitive hook is absent. Array.from passes the full ToLength result to its
constructor before iterating, preserving constructor side effects and errors.
The shared result creation path used by Array.fromAsync retains the logical
length too. Existing allocation and loop bounds remain enforced.

Validation passed **412 core tests**, five worker tests, core all-target Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. Source and worker hashes were revalidated after the
broad run. The first candidate passed both Array variants but exposed a further
Date conversion failure in object ToPrimitive; its focused report and check log
are retained. The second candidate passes all three. Reports, projection,
comparison, timing, source diff, identity, and check logs are archived.
