// Classifying a navigation failure into words a person can act on.
//
// The store (nav-failure.svelte.ts) owns the state; this module owns the
// only part worth unit testing — turning whatever the platform threw into
// one honest sentence. Kept pure (no runes, no router import) because the
// vitest config deliberately has no Svelte plugin: every module under test
// is server-side or pure.

/** The failure the banner renders: what was attempted, what to tell the
 *  person, and when it happened (a retry refreshes it). */
export interface NavigationFailure {
  href: string
  message: string
  at: number
}

// The lazy `import()` of a route chunk fails with different words per engine:
//   Chrome:  "Failed to fetch dynamically imported module: <url>"
//   Firefox: "error loading dynamically imported module <url>"
//   WebKit:  "Importing a module script failed."
// The chunk case is the common one after a deploy (the hashed filename no
// longer exists) and the one where reloading is the guaranteed recovery —
// the browser caches the failed import in the document's module map, so a
// retry may keep failing until a fresh document. Plain network failures
// usually clear on their own; the rest gets the honest generic.
export function describeNavigationFailure(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error ?? '')
  const s = raw.toLowerCase()
  if (
    s.includes('dynamically imported module') ||
    s.includes('importing a module script') ||
    s.includes('module script failed')
  ) {
    return 'The code for that page failed to load — a new version may have been deployed, or the connection dropped. Reloading fetches the current version.'
  }
  if (s.includes('failed to fetch') || s.includes('networkerror') || s.includes('load failed')) {
    return 'The network request failed. Check the connection, then try again.'
  }
  return 'The page failed to open. Try again; reloading recovers it if this keeps happening.'
}

// A Try again that failed AGAIN: the retry path is empirically dead for chunk
// loads in engines that cache the rejected import in the document's module
// map (verified on Chrome — poison3.mjs: with the network restored, the same
// URL re-imports straight from the cached failure). The second message stops
// offering the same click and points at Reload, which is the one recovery
// that always works: a fresh document re-fetches everything, including the
// current deploy's chunk names.
export function stillFailingMessage(error: unknown): string {
  return 'Still failing after another try — reloading is the reliable fix. ' + describeNavigationFailure(error)
}
