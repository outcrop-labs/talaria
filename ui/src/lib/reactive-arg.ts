// An argument that may be passed eagerly or lazily.
//
// Every hook in this app takes its parameters as GETTERS when it needs to
// re-run reactively (`useBoards(() => appId)`), and callers that have nothing
// reactive to say then write a thunk anyway. So the hooks accept both, and the
// "is it a function?" test that resolves them was declared locally in sixteen
// files — `MaybeGetter<T>` sixteen times, plus two one-off spellings of the
// same resolution (`resolveModel`, `resolveValue`).
//
// Two consequences of the copies, both real: the hooks that forgot to accept the
// eager form were a papercut at every call site, and `type MaybeGetter<T>` in a
// file that already has a `T` in scope is how the sixteenth copy got a
// different name.

/** An argument that may be given eagerly (`'x'`) or lazily (`() => x`). */
export type MaybeGetter<T> = T | (() => T)

/** The value behind a `MaybeGetter`. */
export const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)
