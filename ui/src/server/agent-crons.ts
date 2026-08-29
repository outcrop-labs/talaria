// Native Hermes crons, managed by Talaria. Every agent's gateway runs Hermes'
// own scheduler (ticks /opt/data/cron/jobs.json every 60s), so jobs live and
// fire INSIDE the agent — Talaria is just the control surface: it reads
// jobs.json for truth and drives the `hermes cron` CLI for mutations through
// the running container (docker exec). Requires the container to be up.
//
// METERING CAVEAT — a cron fires the agent's own loop on the agent's own
// config, i.e. the PERSONA gateway key, which the gateway leaves unmetered
// because a Talaria flow normally writes that turn's ledger row. A cron has no
// flow, so its spend reaches no ledger. Not fixable from here: `hermes cron
// create` takes no model/provider/credential (only schedule, prompt, skills,
// script, workdir), and the scheduler runs inside the single `hermes gateway
// run` process — so at the gateway a cron turn is byte-identical to a persona
// turn. Closing it needs the schedule DRIVEN by Talaria (dispatch the prompt
// through gateway.ts's proxyChat like any other turn, which meters and
// attributes it for free) instead of by the agent's own ticker.
import { db } from './db/pg'
import { getSetting } from './audit'
import { agentContainer, dockerExec } from './docker-exec'

const JOBS_PATH = '/opt/data/cron/jobs.json'

async function agentFor(defId: string): Promise<{ department: string; slug: string }> {
  const sql = await db()
  const rows = (await sql`
    select department, slug from agent_defs where id = ${defId} and managed and enabled
  `) as unknown as Array<{ department: string; slug: string }>
  if (!rows[0]) throw new Error('not a running managed agent')
  return rows[0]
}

export interface CronJob {
  id: string
  name: string
  prompt: string
  /** Human schedule as Hermes displays it (cron expr or "every 2h"). */
  schedule: string
  enabled: boolean
  state: string
  nextRunAt: string | null
  lastRunAt: string | null
  lastStatus: string | null
  lastError: string | null
}

interface RawJob {
  id: string
  name: string | null
  prompt: string | null
  schedule_display?: string
  schedule?: { display?: string; expr?: string }
  enabled?: boolean
  state?: string
  next_run_at?: string | null
  last_run_at?: string | null
  last_status?: string | null
  last_error?: string | null
}

export async function listCronJobs(defId: string): Promise<CronJob[]> {
  const { department } = await agentFor(defId)
  const name = await agentContainer(department)
  const { stdout } = await dockerExec(name, ['cat', JOBS_PATH]).catch((e: Error) => {
    if (/no such file/i.test(e.message)) return { stdout: '{"jobs":[]}', stderr: '' }
    throw new Error(`cannot read crons from ${name}: ${e.message}`)
  })
  const raw = (JSON.parse(stdout) as { jobs?: RawJob[] }).jobs ?? []
  return raw.map((j) => ({
    id: j.id,
    name: j.name ?? j.id,
    prompt: j.prompt ?? '',
    schedule: j.schedule_display ?? j.schedule?.display ?? j.schedule?.expr ?? '',
    enabled: j.enabled ?? true,
    state: j.state ?? 'scheduled',
    nextRunAt: j.next_run_at ?? null,
    lastRunAt: j.last_run_at ?? null,
    lastStatus: j.last_status ?? null,
    lastError: j.last_error ?? null,
  }))
}

// Hermes is the schedule validator ('30m', 'every 2h', or a 5-field cron) and
// its error comes back verbatim; we only block flag injection and newlines.
function assertSafe(value: string, what: string): void {
  if (value.startsWith('-')) throw new Error(`${what} cannot start with "-"`)
  if (/[\n\r]/.test(value)) throw new Error(`${what} cannot contain newlines`)
}

// ── Frequency floor ─────────────────────────────────────────────────────────
// A cron job is an agent turn, and an agent turn is LLM spend. `* * * * *`
// used to be accepted verbatim: 1,440 turns a day, per agent, forever — the
// cheapest way in the product to run up an unbounded bill, and the schedule a
// well-meaning "check this often" request lands on. The floor is the coarse
// guard; the gateway budget is the fine one.
const DEFAULT_MIN_INTERVAL_MINUTES = 5

export const cronFloorMinutes = (): Promise<number> =>
  getSetting<number>('cron_min_interval_minutes', DEFAULT_MIN_INTERVAL_MINUTES)

const UNIT_MINUTES: Record<string, number> = { s: 1 / 60, m: 1, h: 60, d: 1440 }

/** Expand one comma-separated cron field into the values it fires on. */
function cronFieldValues(field: string, max: number): number[] {
  const out = new Set<number>()
  for (const part of field.split(',')) {
    const [spec, stepRaw] = part.split('/')
    const step = stepRaw ? Number(stepRaw) : 1
    if (!Number.isFinite(step) || step < 1) return []
    let lo = 0
    let hi = max
    if (spec && spec !== '*') {
      const [a, b] = spec.split('-')
      lo = Number(a)
      hi = b === undefined ? lo : Number(b)
      if (!Number.isFinite(lo) || !Number.isFinite(hi) || lo < 0 || hi > max || lo > hi) return []
    }
    for (let v = lo; v <= hi; v += step) out.add(v)
  }
  return [...out].sort((x, y) => x - y)
}

/**
 * The SHORTEST gap between two firings of a schedule, in minutes — the number
 * a floor has to be compared against. Null when we can't tell (Hermes stays the
 * validator for anything exotic; we only refuse what we can prove is too fast).
 */
export function minIntervalMinutes(schedule: string): number | null {
  const s = schedule.trim().toLowerCase()

  // Interval form: "30m", "every 2h", "90s".
  const iv = /^(?:every\s+)?(\d+)\s*(s|sec|secs|second|seconds|m|min|mins|minute|minutes|h|hr|hrs|hour|hours|d|day|days)$/.exec(s)
  if (iv) {
    const unit = UNIT_MINUTES[iv[2]![0]!]
    return unit === undefined ? null : Number(iv[1]) * unit
  }

  // 5-field cron: minute hour dom mon dow.
  const fields = s.split(/\s+/)
  if (fields.length !== 5) return null
  const minutes = cronFieldValues(fields[0]!, 59)
  if (minutes.length === 0) return null
  // One minute value an hour → at most hourly, whatever the other fields say.
  if (minutes.length === 1) return 60
  let gap = 60 - minutes[minutes.length - 1]! + minutes[0]! // wrap past the hour
  for (let i = 1; i < minutes.length; i++) gap = Math.min(gap, minutes[i]! - minutes[i - 1]!)
  return gap
}

/** Refuse a schedule we can PROVE fires faster than the floor lets it. */
async function assertScheduleAllowed(schedule: string): Promise<void> {
  const gap = minIntervalMinutes(schedule)
  if (gap === null) return
  const floor = await cronFloorMinutes()
  if (floor > 0 && gap < floor) {
    throw new Error(
      `schedule fires every ${gap < 1 ? `${Math.round(gap * 60)}s` : `${gap}m`}, faster than the ${floor}m minimum an admin set — ask them to lower the floor or pick a slower schedule`,
    )
  }
}

export async function createCronJob(
  defId: string,
  input: { name: string; schedule: string; prompt: string },
): Promise<{ id: string | null }> {
  const { department } = await agentFor(defId)
  const name = input.name.trim()
  const schedule = input.schedule.trim()
  const prompt = input.prompt.trim()
  if (!name || !schedule || !prompt) throw new Error('name, schedule, and prompt are required')
  assertSafe(name, 'name')
  assertSafe(schedule, 'schedule')
  await assertScheduleAllowed(schedule)
  if (prompt.startsWith('-')) throw new Error('prompt cannot start with "-"')
  const { stdout } = await dockerExec(await agentContainer(department), [
    'hermes',
    'cron',
    'create',
    '--name',
    name,
    '--deliver',
    'local',
    schedule,
    prompt,
  ])
  return { id: /Created job:\s*([a-f0-9]+)/.exec(stdout)?.[1] ?? null }
}

const JOB_ID = /^[a-f0-9]{6,32}$/

/** In-place edit via `hermes cron edit` — name/schedule/prompt, any subset. */
export async function editCronJob(
  defId: string,
  jobId: string,
  input: { name?: string; schedule?: string; prompt?: string },
): Promise<void> {
  if (!JOB_ID.test(jobId)) throw new Error('bad job id')
  const { department } = await agentFor(defId)
  const args = ['hermes', 'cron', 'edit', jobId]
  if (input.name !== undefined) {
    assertSafe(input.name, 'name')
    args.push('--name', input.name.trim())
  }
  if (input.schedule !== undefined) {
    assertSafe(input.schedule, 'schedule')
    await assertScheduleAllowed(input.schedule)
    args.push('--schedule', input.schedule.trim())
  }
  if (input.prompt !== undefined) {
    if (input.prompt.startsWith('-')) throw new Error('prompt cannot start with "-"')
    args.push('--prompt', input.prompt.trim())
  }
  if (args.length === 4) throw new Error('nothing to edit')
  await dockerExec(await agentContainer(department), args)
}

async function jobAction(defId: string, jobId: string, action: 'remove' | 'pause' | 'resume' | 'run'): Promise<void> {
  if (!JOB_ID.test(jobId)) throw new Error('bad job id')
  const { department } = await agentFor(defId)
  await dockerExec(await agentContainer(department), ['hermes', 'cron', action, jobId])
}

export const removeCronJob = (defId: string, jobId: string) => jobAction(defId, jobId, 'remove')
export const pauseCronJob = (defId: string, jobId: string) => jobAction(defId, jobId, 'pause')
export const resumeCronJob = (defId: string, jobId: string) => jobAction(defId, jobId, 'resume')
/** Queue the job for the next scheduler tick (≤60s). */
export const runCronJob = (defId: string, jobId: string) => jobAction(defId, jobId, 'run')

export interface FleetCronAgent {
  id: string
  slug: string
  displayName: string
  jobs: CronJob[]
  error?: string
}

/** Crons across the whole managed fleet; a down container is reported, not fatal. */
export async function listFleetCrons(): Promise<FleetCronAgent[]> {
  const sql = await db()
  const defs = (await sql`
    select id, slug, display_name as "displayName" from agent_defs
    where managed and enabled order by slug
  `) as unknown as Array<{ id: string; slug: string; displayName: string }>
  return Promise.all(
    defs.map(async (d) => {
      try {
        return { ...d, jobs: await listCronJobs(d.id) }
      } catch (e) {
        return { ...d, jobs: [], error: (e as Error).message }
      }
    }),
  )
}

/** Create the same job on many agents. When the schedule is a cron expression
 *  with a plain numeric minute, each agent is staggered by `staggerMinutes` so
 *  the fleet doesn't hit the shared LLM at the same instant (the stack's
 *  long-standing convention); interval schedules ("every 2h") pass through. */
export async function createFleetCrons(input: {
  agentIds: string[]
  name: string
  schedule: string
  prompt: string
  staggerMinutes?: number
}): Promise<Array<{ agentId: string; ok: boolean; error?: string }>> {
  const stagger = input.staggerMinutes ?? 2
  const m = /^(\d{1,2})((?:\s+\S+){4})$/.exec(input.schedule.trim())
  const results: Array<{ agentId: string; ok: boolean; error?: string }> = []
  for (let i = 0; i < input.agentIds.length; i++) {
    const schedule = m ? `${(Number(m[1]) + i * stagger) % 60}${m[2]}` : input.schedule
    try {
      await createCronJob(input.agentIds[i]!, { name: input.name, schedule, prompt: input.prompt })
      results.push({ agentId: input.agentIds[i]!, ok: true })
    } catch (e) {
      results.push({ agentId: input.agentIds[i]!, ok: false, error: (e as Error).message })
    }
  }
  return results
}
