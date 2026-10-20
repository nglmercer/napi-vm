# Async iterator disposal

The shared AsyncIteratorPrototype now has a writable, nonenumerable,
configurable Symbol.asyncDispose method named `[Symbol.asyncDispose]`, with
length zero. It obtains the receiver's return method and calls it with the
receiver and no arguments. Missing or null return methods fulfill with
undefined. Invalid methods, getter/call failures, and promise-resolution errors
reject the returned promise without throwing synchronously. Internal VM control
and teardown signals remain internal rather than becoming guest rejections.

A return value is awaited through the intrinsic PromiseResolve path. Internal
reactions discard the successful value and forward rejection; the implementation
does not read Promise.prototype.then or public Promise.resolve. Real async
generators run their finally cleanup before disposal completes.

This follows [AsyncIteratorPrototype asyncDispose](https://tc39.es/ecma262/multipage/control-abstraction-objects.html#sec-%asynciteratorprototype%-@@asyncdispose).

The initial three regressions fail before the change. Five focused tests now
cover pending cleanup, return receiver/argument count, getter/call/rejection and
promise-constructor errors, public then replacement, real generator cleanup,
missing return methods, invalid receivers, and await-using scope cleanup. All five tests, 369 core-library
tests without exclusions, strict workspace Clippy, and formatting pass.

The frozen baseline passes 1,770 / 1,788 selected variants across async iterator
prototypes, AsyncDisposableStack, using/await-using, and async generator
expressions. The final release passes **1,788 / 1,788**, gaining **18 passes with zero
losses**. All 26 AsyncIteratorPrototype, 208 AsyncDisposableStack, 188 await-using,
154 using, and 1,212 async generator expression variants pass. See the
[archived evidence](../tools/test262/evidence/async-iterator-disposal-2026-10-20/summary.json). The completed
full score remains 102,050 / 102,956 on source 75c6857 and excludes this change.
