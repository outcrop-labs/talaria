// When the daily brief opens, and where that answer comes from.
//
// THE RULE IS "TWO HOURS BEFORE NORMAL WORKING HOURS", which is a product
// decision with a clock attached, so it is stated as a workday start and a lead
// rather than as a fire hour. Those are two different facts: an org that moves
// to an 08:00 start wants its brief at 06:00 without anyone recomputing, and an
// org that wants more (or less) reading time before the day starts is changing
// the LEAD, not the start. Storing only the answer would have made both of
// those edits look identical, and the second one silently wrong.
//
// THE ZONE IS PER-PERSON: `users.timezone` when the account set one, this
// config's zone (TZ env → UTC) otherwise. The seam the old "ORG-WIDE,
// DELIBERATELY" note here left open — `zoneFor` was the single function that
// would change, and it did: it now resolves the person's stored zone against
// the workspace default, and `server/digest.ts` does the same through its own
// `recipientZone`.
import { getSetting, setSetting } from './audit'
import { localMoment } from './tz'

export interface BriefConfig {
  /** Local hour (0-23) the workday is considered to start. */
  workdayStartHour: number
  /** How many hours BEFORE that the brief opens. The product rule is 2. */
  leadHours: number
  /** IANA zone the hours are read in. */
  timeZone: string
  /** Minutes between sweeps of an open brief. The floor on "how stale can the
   *  brief be", and the ceiling on how much work following the day costs — a
   *  sweep is four scoped queries per person with an open brief. */
  sweepMinutes: number
}

export const BRIEF_CONFIG_KEY = 'brief_config'

const DEFAULTS: BriefConfig = {
  workdayStartHour: 9,
  leadHours: 2,
  timeZone: process.env.TZ || 'UTC',
  // Five minutes. The brief is the surface a person leaves open all day, and a
  // slower sweep is directly visible as "it did not know yet". Realtime nudges
  // (server/realtime.ts) shorten the observed latency further for the events
  // that publish; this is the floor for everything that does not.
  sweepMinutes: 5,
}

const hourIn = (value: unknown, fallback: number): number =>
  typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 23 ? value : fallback

export async function briefConfig(): Promise<BriefConfig> {
  const stored = await getSetting<Partial<BriefConfig>>(BRIEF_CONFIG_KEY, {})
  const positive = (v: unknown, fallback: number): number => (typeof v === 'number' && v > 0 ? v : fallback)
  return {
    workdayStartHour: hourIn(stored.workdayStartHour, DEFAULTS.workdayStartHour),
    // Clamped to the day it is subtracted from. A lead of 30 would otherwise
    // wrap `fireHour` into a negative and open every brief at a nonsense hour;
    // the config surface is admin-editable and this is the value most likely to
    // be typed wrong.
    leadHours: Math.min(23, positive(stored.leadHours, DEFAULTS.leadHours)),
    timeZone: typeof stored.timeZone === 'string' && stored.timeZone.trim() ? stored.timeZone.trim() : DEFAULTS.timeZone,
    sweepMinutes: Math.max(1, positive(stored.sweepMinutes, DEFAULTS.sweepMinutes)),
  }
}

export async function setBriefConfig(patch: Partial<BriefConfig>): Promise<BriefConfig> {
  const next = { ...(await briefConfig()), ...patch }
  await setSetting(BRIEF_CONFIG_KEY, next)
  return briefConfig()
}

/** The local hour a brief opens. Wraps rather than clamps: a 01:00 start with a
 *  2h lead means the brief opens at 23:00 the PREVIOUS evening, which is the
 *  literally correct reading of "two hours before you start" for a night shift
 *  and the only one that does not silently move someone's brief to midnight. */
export function fireHour(config: BriefConfig): number {
  return (config.workdayStartHour - config.leadHours + 24) % 24
}

/** The person's zone: their stored preference when they set one, the
 *  workspace's otherwise. Pure over the VALUE (not the userId) on purpose —
 *  callers that hold only an id fetch it first, `runBriefPass` carries it in
 *  the same query as the user row, and the digest loop stays sync. A blank
 *  stored value counts as unset, so a hand-emptied row follows the workspace
 *  rather than throwing at every format call. */
export function zoneFor(stored: string | null | undefined, config: { timeZone: string }): string {
  return stored && stored.trim() ? stored : config.timeZone
}

/** Next moment a brief opens in `zone`, from `at`. Rendered by the surface
 *  while it is waiting, so "no brief yet" can say WHEN instead of just NO. */
export function nextBriefAt(config: BriefConfig, zone: string, at: Date): Date {
  const hour = fireHour(config)
  // Walked forward an hour at a time rather than computed, because arithmetic
  // on a wall clock is wrong across a DST boundary — the day a zone springs
  // forward has no 02:00, and the day it falls back has two. Stepping and
  // re-reading the local hour is the version that survives both.
  const step = new Date(at.getTime())
  step.setUTCMinutes(0, 0, 0)
  for (let i = 0; i <= 48; i++) {
    step.setUTCHours(step.getUTCHours() + 1)
    if (localHour(zone, step) === hour) return step
  }
  return step
}

/** Local hour in a zone. An unknown zone falls back to UTC rather than throwing
 *  — a typo in one settings row must not stop every brief in the workspace,
 *  which is the exact failure the scheduler exists to prevent. */
export function localHour(zone: string, at: Date): number {
  return localMoment(zone, at).hour
}

// Re-exported from the leaf so this module's importers (daily-brief.ts, its
// test) keep their imports. It used to be a deliberate byte-for-byte duplicate
// of digest.ts's copy — see server/tz.ts for why sharing it needed a leaf.
export { localMoment } from './tz'

/** Which workday a brief opened at `at` is FOR, and whether it is due yet.
 *
 *  ONE FUNCTION FOR BOTH ANSWERS, because they are the same question asked
 *  twice and the two ways of getting them wrong compound. `due` is a WINDOW,
 *  not an equality: the scheduler ticks every few minutes and can miss one — a
 *  deploy, a lease held by an instance that died, a slow pass — and an
 *  `hour === fireHour` test would mean one missed tick costs somebody their
 *  entire brief for the day. This says "the hour has come and the workday has
 *  not turned over", so the next tick after any gap still opens it.
 *
 *  `date` is the workday the brief COVERS, which is not always the local date
 *  it opens on. With an early enough start the lead wraps the fire hour into
 *  the previous evening (a 01:00 start, 2h lead → the brief opens at 23:00),
 *  and a brief written on Monday evening for Tuesday's work belongs to Tuesday.
 *  Getting this wrong does not merely mislabel the document — `brief_date` is
 *  half the unique key, so an evening brief filed under the wrong day would be
 *  re-opened, as a second brief, when the day it was written for arrived. */
export function briefWindow(config: BriefConfig, zone: string, at: Date): { due: boolean; date: string } {
  const { hour, date } = localMoment(zone, at)
  const fire = fireHour(config)
  const wrapped = fire > config.workdayStartHour
  if (!wrapped) return { due: hour >= fire, date }
  // Wrapped: the window is [fire, midnight) ∪ [midnight, workdayStart), and the
  // first half is the evening BEFORE the day it is for.
  if (hour >= fire) return { due: true, date: shiftDate(date, 1) }
  return { due: hour < config.workdayStartHour, date }
}

/** Calendar-date arithmetic on a `YYYY-MM-DD` string. Done in UTC on a parsed
 *  date rather than on the zone, because the input is already a LOCAL date and
 *  "the day after Monday" is a calendar fact with no clock in it — running it
 *  through a zone again is what introduces a DST bug rather than avoiding one. */
function shiftDate(date: string, days: number): string {
  const at = new Date(`${date}T00:00:00Z`)
  at.setUTCDate(at.getUTCDate() + days)
  return at.toISOString().slice(0, 10)
}
