// EVERY CASE OF EVERY FITNESS RUN, IN FULL — the audit trail the archived
// report could never be.
//
// WHY THE REPORT IS NOT ENOUGH, and this is the whole argument. `FitnessRecord`
// lives in one `app_settings` row read whole on every page load, so it keeps a
// transcript only for cases that FAILED something, capped at thirty. That is
// exactly right for a drill-down and exactly wrong for verification:
//
//   THE PASSING TRANSCRIPTS ARE THE ONES THAT ANSWER THE HARD QUESTION. "Did
//   this model do the work, or did our fixture accept something weak?" cannot be
//   answered from a failure. Several fixtures rewritten this month were rewritten
//   because a PASSING transcript turned out to show a model being credited for
//   the wrong thing — or a failing one showing a model being punished for obeying
//   an instruction we gave it. Every time, the evidence had to be re-bought by
//   re-running the sweep, because we had thrown it away for being green.
//
//   AND A CAP IS NOT A RETENTION POLICY. Thirty of two hundred and forty-seven
//   is a sample chosen by arrival order.
//
// So the numbers stay in the settings row — that is a summary and belongs there —
// and the EVIDENCE goes in a table: one row per case, written as it lands,
// pruned by run rather than by size. Nothing in the read path of the fitness
// page touches it; it is opened on purpose, per model, when somebody is auditing.
import { db } from '../db/pg'
import type { EvalCaseScore } from './evals'

/** One archived case, as it comes back out. */
export interface Transcript {
  model: string
  runStartedAt: string
  harness: string
  case: string
  band: string
  /** The same vocabulary the live feed colours by — see `liveLog`. */
  verdict: string
  prompt: string | null
  raw: string | null
  turns: unknown
  toolCalls: unknown
  upstream: unknown
  latencyMs: number
  /** What the case cost the sweep, including retries — see the migration. */
  wallMs: number
  startedAt: string | null
  promptTokens: number
  completionTokens: number
  createdAt: string
}

/** RUNS KEPT PER MODEL. Not cases: a run is the unit somebody audits, and
 *  pruning by row count would leave half of one. Five is enough to compare a
 *  model against itself across a month of releases and small enough that the
 *  table stays a tool rather than a warehouse. */
export const KEEP_RUNS_PER_MODEL = 5

/** Bounded here as well as at the call site, because this is the last thing
 *  between a model's 200KB reply and a table row. */
const TEXT_CAP = 20_000

const verdictOf = (c: EvalCaseScore): string =>
  c.skipped !== null ? 'skip' : c.timedOut ? 'timeout' : c.gap !== null ? 'gap' : !c.contractHeld || c.error !== null ? 'error' : c.task === 'fail' ? 'fail' : 'pass'

/** File one case. Called as the case lands, so a sweep an admin stops still has
 *  every transcript it paid for.
 *
 *  NEVER THROWS. The audit trail is valuable and the sweep is more valuable: a
 *  full disk or a locked table must cost the run its evidence, not its results.
 *  The caller does not check a return value for the same reason. */
export async function recordTranscript(model: string, runStartedAt: string, c: EvalCaseScore): Promise<void> {
  try {
    const sql = await db()
    await sql`
      insert into fitness_transcripts
        (model, run_started_at, harness, case_name, band, verdict, prompt, raw, turns, tool_calls, upstream,
         latency_ms, wall_ms, started_at, prompt_tokens, completion_tokens)
      values (
        ${model}, ${runStartedAt}, ${c.harness}, ${c.case}, ${c.band}, ${verdictOf(c)},
        ${c.prompt?.slice(0, TEXT_CAP) ?? null}, ${c.raw?.slice(0, TEXT_CAP) ?? null},
        ${sql.json((c.turns ?? null) as never)}, ${sql.json((c.calls ?? null) as never)}, ${sql.json((c.upstream ?? null) as never)},
        ${c.latencyMs}, ${c.wallMs}, ${c.startedAt}, ${c.promptTokens}, ${c.completionTokens}
      )
    `
  } catch {
    // Deliberately silent per case: a broken audit table would otherwise print
    // 247 identical lines per sweep. `pruneTranscripts` runs at the end of a run
    // and is where a persistent failure surfaces.
  }
}

/** Drop every run for this model beyond the newest `KEEP_RUNS_PER_MODEL`.
 *
 *  BY RUN, using the run's own start time as its identity — which is what makes
 *  "the last five runs" a truthful phrase rather than "the last five thousand
 *  rows, whatever that spans". */
export async function pruneTranscripts(model: string, keep = KEEP_RUNS_PER_MODEL): Promise<void> {
  const sql = await db()
  await sql`
    delete from fitness_transcripts
    where model = ${model}
      and run_started_at not in (
        select run_started_at from fitness_transcripts
        where model = ${model}
        group by run_started_at
        order by run_started_at desc
        limit ${keep}
      )
  `
}

/** The runs this model has archived evidence for, newest first. */
export async function transcriptRuns(model: string): Promise<Array<{ runStartedAt: string; cases: number }>> {
  const sql = await db()
  const rows = (await sql`
    select run_started_at as "runStartedAt", count(*)::int as cases
    from fitness_transcripts
    where model = ${model}
    group by run_started_at
    order by run_started_at desc
  `) as unknown as Array<{ runStartedAt: string | Date; cases: number }>
  return rows.map((r) => ({ runStartedAt: new Date(r.runStartedAt).toISOString(), cases: r.cases }))
}

/** Every case of one run, in the order the harnesses declare them. `runStartedAt`
 *  omitted means the newest run on record. */
export async function readTranscripts(model: string, runStartedAt?: string): Promise<Transcript[]> {
  const sql = await db()
  const rows = (await sql`
    select model, run_started_at as "runStartedAt", harness, case_name as "case", band, verdict,
           prompt, raw, turns, tool_calls as "toolCalls", upstream,
           latency_ms as "latencyMs", wall_ms as "wallMs", started_at as "startedAt",
           prompt_tokens as "promptTokens", completion_tokens as "completionTokens",
           created_at as "createdAt"
    from fitness_transcripts
    where model = ${model}
      and run_started_at = ${
        runStartedAt ?? sql`(select max(run_started_at) from fitness_transcripts where model = ${model})`
      }
    order by created_at
  `) as unknown as Array<Transcript & { runStartedAt: string | Date; createdAt: string | Date; startedAt: string | Date | null }>
  return rows.map((r) => ({
    ...r,
    runStartedAt: new Date(r.runStartedAt).toISOString(),
    createdAt: new Date(r.createdAt).toISOString(),
    startedAt: r.startedAt ? new Date(r.startedAt).toISOString() : null,
  }))
}

/** Delete archived transcripts — one model, or every one. Returns the row count,
 *  because "cleared" with no number is a claim an admin cannot check. */
export async function clearTranscripts(model: string | null): Promise<number> {
  const sql = await db()
  const rows = model === null
    ? ((await sql`delete from fitness_transcripts returning id`) as unknown as unknown[])
    : ((await sql`delete from fitness_transcripts where model = ${model} returning id`) as unknown as unknown[])
  return rows.length
}
