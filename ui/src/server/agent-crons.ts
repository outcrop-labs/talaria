// Native Hermes crons, managed by Talaria. Every agent's gateway runs Hermes'
// own scheduler (ticks /opt/data/cron/jobs.json every 60s), so jobs live and
// fire INSIDE the agent — Talaria is just the control surface: it reads
// jobs.json for truth and drives the `hermes cron` CLI for mutations through
// the running container (docker exec). Requires the container to be up.
import { execFile } from 'node:child_process'
import { db } from './db/pg'

const JOBS_PATH = '/opt/data/cron/jobs.json'

function exec(args: string[]): Promise<{ stdout: string; stderr: string }> {
  return new Promise((res, rej) => {
    execFile('docker', args, { timeout: 30_000, maxBuffer: 4 * 1024 * 1024 }, (err, stdout, stderr) =>
      err ? rej(new Error(stderr.trim() || err.message)) : res({ stdout, stderr }),
    )
  })
}

async function agentFor(defId: string): Promise<{ department: string; slug: string }> {
  const sql = await db()
  const rows = (await sql`
    select department, slug from agent_defs where id = ${defId} and managed and enabled
  `) as unknown as Array<{ department: string; slug: string }>
  if (!rows[0]) throw new Error('not a running managed agent')
  return rows[0]
}

const container = (department: string) => `talaria-fleet-agent-${department}-1`

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
  const { stdout } = await exec(['exec', container(department), 'cat', JOBS_PATH]).catch((e: Error) => {
    if (/no such file/i.test(e.message)) return { stdout: '{"jobs":[]}', stderr: '' }
    throw new Error(`cannot read crons from ${container(department)}: ${e.message}`)
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
  if (prompt.startsWith('-')) throw new Error('prompt cannot start with "-"')
  const { stdout } = await exec([
    'exec',
    container(department),
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

async function jobAction(defId: string, jobId: string, action: 'remove' | 'pause' | 'resume' | 'run'): Promise<void> {
  if (!JOB_ID.test(jobId)) throw new Error('bad job id')
  const { department } = await agentFor(defId)
  await exec(['exec', container(department), 'hermes', 'cron', action, jobId])
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
