# Namespace descriptor TDZ continuation

Module namespace GetOwnProperty queries must read the live export binding.
Previously the shared descriptor path read the slot directly and returned a
descriptor even when that binding was still uninitialized. The namespace path
now reads the export through the interpreter before returning the descriptor.
Ordinary descriptor flags and symbol-property behavior are retained.

Two regressions observe a self-importing module before its exports initialize.
Descriptor queries, hasOwn/hasOwnProperty, keys, propertyIsEnumerable, for-in,
and ordinary writes through a namespace receiver raise ReferenceError where
required. Key-only operations and symbol descriptors remain available; direct
namespace writes still reject with TypeError or false. Descriptor values and
flags are checked again after initialization.

All 22 module integration tests and 369 core-library tests pass without
exclusions. Strict workspace Clippy passes. The frozen baseline passes
7,881 / 7,916 variants across module code, Object, Reflect, and for-in; the final release passes **7,887 / 7,916**, gaining **six passes with zero losses**. The completed full suite freezes
ffc10c7 and excludes this continuation.

The first focused run found six gains and a deletion regression. Namespace
Delete now uses key presence directly, without calling GetOwnProperty or
reading the export binding. Strict deletion rejects existing exports with
TypeError, reflection returns false, and missing string keys delete successfully.
Symbol deletion retains the ordinary descriptor path. The final regressions
cover these cases before initialization. The deferred-import selection remains **109 / 109**, with zero losses.

Evidence, including the preliminary regression and final correction, is archived
in [namespace evidence](../tools/test262/evidence/namespace-tdz-2026-10-20/summary.json).
