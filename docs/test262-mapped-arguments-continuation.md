# Mapped arguments continuation

## Implementation

Sloppy functions with simple parameter lists link supplied argument indexes to
their parameter bindings using the existing traced live-binding cells. Duplicate
names map only their last formal occurrence; unsupplied indexes remain absent.
Strict functions, arrows, and functions with default, rest, or destructured
parameters do not create their own mapped indexes. Arrows inherit the enclosing
arguments binding.

Ordinary reads and property descriptors observe the current cell value. Indexed
assignment, including bytecode inline-cache hits, updates the parameter. A
successful deletion disconnects that index. Accessor conversion and making an
index nonwritable disconnect it; a nonwritable descriptor without a value
snapshots the current parameter. Failed deletion or descriptor changes preserve
the mapping. Receiver-sensitive Reflect.set preserves ordinary receiver rules.
Seal retains mapping; freeze snapshots and disconnects it.

Functions reading arguments can now execute in verified bytecode. The compiler
boxes their parameters and arguments binding before hoisting. Bare var arguments
preserves the seeded object, while parameters, lexicals, and function declarations
retain their shadowing behavior. Generator parameter environments initialize
before suspension and retain the same cells across resumes.

Existing heap tracing handles the shared binding cells. Escaped argument objects
retain parameter values; replacements release old values, and unreachable
arguments/parameter cycles are collected. No guest property can expose a binding
cell or forge a parameter map.

The rules follow the [arguments exotic object algorithms](https://tc39.es/ecma262/multipage/ordinary-and-exotic-objects-behaviours.html#sec-arguments-exotic-objects).

## Validation

Twenty focused tests cover AST and prepared execution, including an explicit
verified-bytecode function and hot indexed writes. The neighboring async-generator,
generator stress, lifecycle, function declaration, and lexical-scope suites pass
72 tests. Strict workspace Clippy, formatting, and all 79 selected bytecode tests
pass. The existing unrelated super-destructuring parity failure is excluded
from the bytecode selection; this is not a full Cargo-suite claim.

The release Test262 selection passes **13,621 / 13,661** variants, with
**31 gains and zero losses**. Its 13,661-variant selection covers
arguments objects, property descriptors, seal/freeze, generators and async
generators, Proxy, and the preceding control-flow selection. The baseline passes
13,590 variants. All **43 mapped**, **8 unmapped**, and **1,442 for-of**
variants pass. The arguments-object directory passes 458 / 460; its two
remaining failures concern legacy callee.caller behavior. The expanded
selection retains forty failures and has no skips, timeouts, crashes, or
harness errors. Reports, comparison, worker identity, and the source patch
are archived under [mapped arguments evidence](../tools/test262/evidence/mapped-arguments-2026-10-18/).

The latest completed full report measures 101,728 / 102,956 on
607d086; focused gains are not added to that full score.
