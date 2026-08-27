// The one true `localMoment`. A leaf by necessity: it used to live in
// server/digest.ts, and server/daily-brief-config.ts carried a byte-for-byte
// duplicate of it — not out of carelessness but because digest.ts imports
// `zoneFor` from daily-brief-config, so config importing digest's copy back
// would have closed a cycle, and the brief's module graph would have gained
// the mail transport and digest's `registerJob` calls as baggage. A file that
// imports nothing but `Intl` is the shape that lets both callers share one
// implementation without either pulling the other in.

/** Local hour and calendar date in a zone.
 *
 *  Read through `Intl.DateTimeFormat` rather than by offset arithmetic:
 *  stepping a wall clock by UTC math is wrong across a DST boundary — the day
 *  a zone springs forward has no 02:00, and the day it falls back has two —
 *  and asking the zone itself what its wall clock says is the version that
 *  survives both. `nextBriefAt` (daily-brief-config) walks an hour at a time
 *  through this function for exactly that reason.
 *
 *  A bad zone falls back to UTC instead of throwing: a typo in one settings
 *  row must not take down a scheduled job for the whole workspace — the digest
 *  and the brief both read org- and per-person zones that nobody validates at
 *  rest, and "one bad row silences everyone's brief" is the failure mode both
 *  files exist to prevent. It says so and carries on. */
export function localMoment(timeZone: string, at: Date): { hour: number; date: string; zone: string } {
  const read = (zone: string): { hour: number; date: string; zone: string } => {
    const parts = new Intl.DateTimeFormat('en-CA', {
      timeZone: zone,
      hour12: false,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
    }).formatToParts(at)
    const get = (type: string): string => parts.find((p) => p.type === type)?.value ?? ''
    // 'en-CA' + hour12:false yields 24 for midnight in some ICU versions.
    return { hour: Number(get('hour')) % 24, date: `${get('year')}-${get('month')}-${get('day')}`, zone }
  }
  try {
    return read(timeZone)
  } catch {
    console.warn(`[tz] unknown time zone "${timeZone}" — falling back to UTC`)
    return read('UTC')
  }
}
