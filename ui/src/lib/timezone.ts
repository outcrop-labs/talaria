// Time-zone helpers shared by the browser (Settings picker, silent adoption)
// and the server (profile-PUT validation). Pure `lib/` code on purpose: the
// import invariant forbids browser modules from value-importing `@/server`,
// and this is the one piece of zone logic both sides need.
//
// The validation idiom is the codebase's established one — ask Intl to
// RESOLVE the name rather than pattern-matching it — the same check
// `readerZone` in server/daily-brief.ts applies to the brief's `?tz=` param.
// A name that parses is an IANA zone the runtime actually knows.

/** Can this runtime resolve `tz` as an IANA zone? The only authority that
 *  matters: a name is valid exactly where it formats. */
export function isValidTimeZone(tz: string): boolean {
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: tz })
    return true
  } catch {
    return false
  }
}

/** The browser's own zone, or null when the environment won't say. This is
 *  the autodetect signal — `resolvedOptions()` is the supported way to ask,
 *  and it can throw on old ICUs, so callers get null instead of a crash. */
export function detectedZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone ?? null
  } catch {
    return null
  }
}

/** Every zone this runtime knows, for the Settings picker. Browsers without
 *  `supportedValuesOf` (older Safari) get an empty list and the picker
 *  degrades to workspace-default + stored + detected rather than a free-text
 *  box — the picker should not offer a way to type a zone it can't check. */
export function supportedTimeZones(): string[] {
  try {
    const zones = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('timeZone')
    if (!Array.isArray(zones) || zones.length === 0) return []
    // Some runtimes resolve 'UTC' but leave it off the supported list — a
    // headless box with no TZ set detects UTC and then can't pick it. The
    // one zone every runtime honors is always on the menu. The list arrives
    // alphabetical; keep it that way.
    return zones.includes('UTC') ? zones : [...zones, 'UTC'].sort()
  } catch {
    return []
  }
}

/** The wall clock in `zone` right now ("9:42 AM") — the live preview beside
 *  each picker row, so choosing a zone shows what it means. Empty string on
 *  an unresolvable zone; the preview is a nicety, never an error surface. */
export function zoneNowLabel(zone: string): string {
  try {
    return new Intl.DateTimeFormat(undefined, { timeZone: zone, hour: 'numeric', minute: '2-digit' }).format(new Date())
  } catch {
    return ''
  }
}
