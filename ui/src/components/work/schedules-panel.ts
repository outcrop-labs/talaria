// The pure half of SchedulesPanel: a schedule row's dot and its one line of
// "what happens next". Both read the TONE rather than the raw fields, because
// the live half of a schedule is null whenever the agent's container could not
// be asked (lib/schedules) — and a row that folded that into "paused" or
// "error" would report a healthy nightly job as broken every time its agent
// rolled.
import type { DotStatus } from '@/components/ui/chip'
import { fmtNext } from '@/components/fleet/agent-crons'
import { scheduleTone, type ScheduleRow, type ScheduleTone } from '@/lib/schedules'

/** Tone → the house health dot. `paused` takes the grey, which is what the
 *  fleet's own rows use for a stopped job (`jobDot`); `unknown` takes the
 *  gold, because "we could not ask the agent" is worth a look — the job is
 *  fine, the agent probably is not running — without being a failure. */
export const TONE_DOT: Record<ScheduleTone, DotStatus> = {
  ok: 'ok',
  paused: 'idle',
  error: 'danger',
  unknown: 'warn',
}

/** The dot's tooltip: the tone in words, since colour alone is not a label. */
export const TONE_TITLE: Record<ScheduleTone, string> = {
  ok: 'scheduled',
  paused: 'paused',
  error: 'the last run failed',
  unknown: 'the agent could not be asked',
}

/** The row's right-hand line. An unreadable live half has no next run to
 *  name and is not "paused" either, so it says what is actually true; a
 *  paused job says so instead of naming a time it will not keep; everything
 *  else names the time. Empty when a live, enabled job reports no next run —
 *  the fleet rows show nothing there too, rather than inventing a word. */
export function nextRunLabel(
  row: Pick<ScheduleRow, 'enabled' | 'state' | 'lastStatus' | 'nextRunAt'>,
): string {
  const tone = scheduleTone(row)
  if (tone === 'unknown') return 'waiting on the agent'
  if (tone === 'paused') return 'paused'
  return row.nextRunAt ? `next ${fmtNext(row.nextRunAt)}` : ''
}
