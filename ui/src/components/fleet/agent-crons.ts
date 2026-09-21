// Native Hermes crons — the jobs live and fire inside each agent's own
// scheduler; Talaria is the control surface. One panel reused by the admin
// agent modal, the member assistant modal, and the fleet-wide schedules view.
//
// Nobody should have to WRITE cron syntax: the schedule builder covers the
// real shapes (interval / daily / weekdays / weekly / monthly) and compiles
// to the cron expression underneath; "custom" exposes the raw string for the
// rest. Schedules DISPLAY in plain English everywhere.
//
// Components: CronsPanel.svelte (one agent) and FleetCronsTab.svelte
// (fleet-wide, admin) over ScheduleBuilder / CronRow / CronForm /
// CronListSkeleton; the schedule model, the helpers and the mutations
// (useCronMutations) live here.

import { confirm } from '@/components/ui/confirm.svelte'
import { delJson, errorMessage, postJson, putJson } from '@/lib/fetch-json'

export interface CronJob {
  id: string
  name: string
  prompt: string
  schedule: string
  enabled: boolean
  state: string
  nextRunAt: string | null
  lastRunAt: string | null
  lastStatus: string | null
  lastError: string | null
}

// ── Schedule model: friendly shapes ⇄ cron/shorthand strings ────────────────
export type SchedMode = 'interval' | 'daily' | 'weekdays' | 'weekly' | 'monthly' | 'custom'
export interface Sched {
  mode: SchedMode
  every: number
  unit: 'm' | 'h'
  time: string // "HH:MM", the agent's clock (UTC)
  dow: number // 0-6, Sunday=0
  dom: number // 1-31
  custom: string
}
export const DEFAULT_SCHED: Sched = { mode: 'daily', every: 30, unit: 'm', time: '09:00', dow: 1, dom: 1, custom: '' }
export const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']

function timeParts(t: string): { h: number; m: number } {
  const [h = 9, m = 0] = t.split(':').map((x) => Number(x))
  return { h: Math.min(23, Math.max(0, h || 0)), m: Math.min(59, Math.max(0, m || 0)) }
}

/** Compile the builder state to what Hermes stores. */
export function schedToString(s: Sched): string {
  const { h, m } = timeParts(s.time)
  switch (s.mode) {
    case 'interval':
      return `every ${Math.max(1, s.every)}${s.unit}`
    case 'daily':
      return `${m} ${h} * * *`
    case 'weekdays':
      return `${m} ${h} * * 1-5`
    case 'weekly':
      return `${m} ${h} * * ${s.dow}`
    case 'monthly':
      return `${m} ${h} ${Math.min(31, Math.max(1, s.dom))} * *`
    case 'custom':
      return s.custom.trim()
  }
}

/** Best-effort inverse — recognized shapes open in the builder, the rest as custom. */
export function parseSchedule(raw: string): Sched {
  const s = raw.trim()
  const interval = /^(?:every\s+)?(\d+)\s*(m|min|minutes?|h|hours?)$/i.exec(s)
  if (interval) {
    return { ...DEFAULT_SCHED, mode: 'interval', every: Number(interval[1]), unit: interval[2]!.startsWith('h') ? 'h' : 'm' }
  }
  const cron = /^(\d{1,2})\s+(\d{1,2})\s+(\S+)\s+(\S+)\s+(\S+)$/.exec(s)
  if (cron) {
    const [, m, h, dom, mon, dow] = cron
    const time = `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`
    if (dom === '*' && mon === '*' && dow === '*') return { ...DEFAULT_SCHED, mode: 'daily', time }
    if (dom === '*' && mon === '*' && dow === '1-5') return { ...DEFAULT_SCHED, mode: 'weekdays', time }
    if (dom === '*' && mon === '*' && /^\d$/.test(dow!)) return { ...DEFAULT_SCHED, mode: 'weekly', time, dow: Number(dow) }
    if (/^\d{1,2}$/.test(dom!) && mon === '*' && dow === '*') return { ...DEFAULT_SCHED, mode: 'monthly', time, dom: Number(dom) }
  }
  return { ...DEFAULT_SCHED, mode: 'custom', custom: s }
}

/** Plain-English schedule for display — the cron string stays in the tooltip. */
export function describeSchedule(raw: string): string {
  const s = parseSchedule(raw)
  const t = s.time
  switch (s.mode) {
    case 'interval':
      return s.unit === 'h' ? (s.every === 1 ? 'Every hour' : `Every ${s.every} hours`) : `Every ${s.every} minutes`
    case 'daily':
      return `Every day at ${t}`
    case 'weekdays':
      return `Weekdays at ${t}`
    case 'weekly':
      return `Every ${DAYS[s.dow]} at ${t}`
    case 'monthly':
      return `Monthly on day ${s.dom} at ${t}`
    case 'custom':
      return raw
  }
}

/** Next-run label — absolute short form ("Wed 9:00 AM"); relativeTime is past-oriented. */
export const fmtNext = (iso: string) =>
  new Date(iso).toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' })

export function jobDot(j: CronJob): string {
  if (!j.enabled || j.state === 'paused') return 'var(--theme-line)'
  if (j.lastStatus && /error|fail/i.test(j.lastStatus)) return 'var(--theme-danger)'
  return 'var(--theme-success)'
}

// ── Mutations: one home for what a schedule row can do ─────────────────────
// Both surfaces (one agent's panel, the fleet-wide tab) fired the same
// requests at `/api/fleet/agents/{agent}/crons[/{job}]`. Nothing about the
// request differed — what differed is what each panel does around it, so that
// arrives as options: the panel's own invalidation, its in-flight flag, and
// one of the two error channels below.

export type CronAction = 'pause' | 'resume' | 'run' | 'remove'
/** A mutation the hook can be asked for — the fan-out names what failed. */
export type CronVerb = CronAction | 'create' | 'edit'

/** The three fields a job is written from (the create and edit forms). */
export interface CronPatch {
  name: string
  schedule: string
  prompt: string
}

/** A panel's own state, which the hook drives: the inline error channel —
 *  the failure message, rendered where the rows are — and the in-flight flag
 *  its controls disable on. Declare it with `$state({ err: null, busy: false })`;
 *  the hook writes through that proxy, so the panel re-renders either way. */
export interface CronMutationState {
  err: string | null
  busy: boolean
}

export interface CronMutationOptions {
  /** Inline channel: a panel that has room for a message renders `state.err`. */
  state?: CronMutationState
  /** Fan-out channel: the raw failure and what was attempted, for a panel
   *  that reports it away from the rows (the fleet tab toasts it). The two
   *  are separate channels — a panel passes `state` OR `onError`, never one
   *  pretending to be the other. */
  onError?: (e: unknown, what: CronVerb) => void
  /** The panel's own list(s), re-read once a mutation settles — `failed` when
   *  the request was rejected. A rejected row action may still have partly
   *  applied server-side; a rejected write changed nothing. */
  invalidate: (what: CronVerb, failed: boolean) => void | Promise<void>
}

/** Where one agent's jobs live; the hook's only URL knowledge. */
const cronsOf = (agentId: string) => `/api/fleet/agents/${agentId}/crons`

/** Create, edit, and the row actions (pause · resume · run-now · delete) on
 *  one agent's jobs, for whichever surface is asking. */
export function useCronMutations(opts: CronMutationOptions) {
  /** One mutation, with the panel's side of it wrapped around: the error is
   *  cleared up front, `busy` holds through the re-read, and the failure goes
   *  out on whichever channel the panel passed. */
  const run = async (what: CronVerb, request: () => Promise<void>): Promise<boolean> => {
    const { state } = opts
    if (state) {
      state.busy = true
      state.err = null
    }
    let failed = false
    try {
      await request()
    } catch (e) {
      failed = true
      if (state) state.err = errorMessage(e)
      opts.onError?.(e, what)
    }
    try {
      await opts.invalidate(what, failed)
    } finally {
      if (state) state.busy = false
    }
    return !failed
  }

  return {
    create: (agentId: string, input: CronPatch) =>
      run('create', async () => {
        await postJson(cronsOf(agentId), input)
      }),

    edit: (agentId: string, jobId: string, patch: CronPatch) =>
      run('edit', async () => {
        await putJson(`${cronsOf(agentId)}/${jobId}`, patch)
      }),

    act: async (agentId: string, jobId: string, action: CronAction) => {
      if (
        action === 'remove' &&
        !(await confirm({ title: 'Delete scheduled job', message: 'Delete this scheduled job?', confirmLabel: 'Delete', danger: true }))
      )
        return
      await run(action, async () => {
        if (action === 'remove') await delJson(`${cronsOf(agentId)}/${jobId}`)
        else await postJson(`${cronsOf(agentId)}/${jobId}`, { action })
      })
    },
  }
}
