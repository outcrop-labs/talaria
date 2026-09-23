// PERSISTED PREFERENCES, with the two storage facts encoded once.
//
// Every key here is a per-person or per-board setting the server cannot see —
// a collapsed rail, a column order, the theme, which tab you were on. They all
// face the same two facts, and each call site used to encode them by hand:
//
//   1. THE STORE IS NOT ALWAYS READABLE. Private mode and "block site data"
//      throw on ACCESS, not only on write, so an unguarded `localStorage` read
//      is a crash in exactly the browser where the setting matters least.
//   2. WHAT COMES BACK IS WHATEVER WAS STORED. A value written by an older
//      release, or by hand in a debugger, must degrade to the caller's fallback
//      — never to a malformed object, and never to a throw.
//
// The parse contract mirrors view-memory.ts, for the same reason: `parse` is a
// required argument returning null for anything it does not recognise, because
// a stored value that does not typecheck has to answer "nothing usable was
// stored". That is also why the fallback is required rather than optional —
// what "nothing stored" MEANS is a decision each key's owner makes (a closed
// rail, an on pref, the default columns), and it is not this module's to guess.
//
// The parse SHAPE differs per key and this module never inspects the value:
//
//   • JSON keys — readStored/writeStored. Arrays, sort states, group maps.
//   • the value itself, not JSON — readText/writeText. A theme id, an agent or
//     chat id, a timestamp, a width. These keys predate JSON and keep their
//     bytes: routing them through JSON would both change what is stored and
//     lose every value already written by an older release.
//   • a boolean flag, spelled '1'/'0' — readFlag/writeFlag.

// The store is reached through `window` rather than the bare `localStorage`
// global for two reasons: the SSR server has no `window` at all, which is an
// undeclared identifier the `catch` already handles, and that is the spelling
// view-memory.ts uses for the session-scoped half of the same idea. Both
// functions absorb the access itself, so "no store" and "the store refused"
// are the one answer — nothing to read, nothing to write.

/** A stored value, or null when there is none to read — absent, unreadable, or
 *  storage that refuses access. The in-memory fallbacks live at the call sites;
 *  this only ever reports what the store said. */
export function readText(key: string): string | null {
  try {
    return window.localStorage.getItem(key)
  } catch {
    return null
  }
}

/** Write, or remove when `value` is null — "no preference" is spelled by the
 *  absence of the key rather than by a sentinel (see useStickyAgent's unpin). */
export function writeText(key: string, value: string | null): void {
  try {
    if (value === null) window.localStorage.removeItem(key)
    else window.localStorage.setItem(key, value)
  } catch {
    /* no window (SSR) / private mode: the preference simply does not persist */
  }
}

/** A JSON-shaped key: `parse` sees the parsed value (typed as unknown, because
 *  that is what came out of storage) and returns null for anything it does not
 *  recognise. Malformed JSON, a parse that throws, and a null all answer
 *  `fallback`. */
export function readStored<T>(key: string, parse: (value: unknown) => T | null, fallback: T): T {
  // Outside the try on purpose: nothing was stored, so nothing can be malformed
  // — the fallback is the answer, not a degradation.
  const raw = readText(key)
  if (raw === null) return fallback
  try {
    return parse(JSON.parse(raw)) ?? fallback
  } catch {
    return fallback
  }
}

/** Store a value in its JSON shape. */
export function writeStored(key: string, value: unknown): void {
  writeText(key, JSON.stringify(value))
}

/** A '1'/'0' flag. Anything else stored under the key — including the absent
 *  key and the unreadable store — answers `fallback`. */
export function readFlag(key: string, fallback = false): boolean {
  const raw = readText(key)
  return raw === null ? fallback : raw === '1'
}

export function writeFlag(key: string, value: boolean): void {
  writeText(key, value ? '1' : '0')
}