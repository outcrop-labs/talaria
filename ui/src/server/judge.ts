// Automated QA judge — the reliability gate on agent-completed work.
//
// When a ticket reaches quality_review, a judge (a configurable, ideally strong
// model reached over the Talaria gateway) reviews the agent's reported outcome
// against the ticket and posts an ADVISORY verdict + specific issues. The human
// reviewer still decides — the judge sharpens that decision, it doesn't replace
// it. (An enforcing revision-loop mode is planned; advisory ships first.)

import { completeViaGateway } from './llm-gateway'
import { guardText, recordFindings } from './guardrails'
import { getSetting, setSetting } from './audit'
import { db } from './db/pg'
import { publishBoard } from './realtime'

export interface JudgeConfig {
  enabled: boolean
  /** Model id (an alias/endpoint from the registry). null → a safe default. */
  model: string | null
}

const CONFIG_KEY = 'judge_config'
const DEFAULT_CONFIG: JudgeConfig = { enabled: false, model: null }

export const getJudgeConfig = () => getSetting<JudgeConfig>(CONFIG_KEY, DEFAULT_CONFIG)
export const setJudgeConfig = (c: JudgeConfig) => setSetting(CONFIG_KEY, c)

export type Verdict = 'pass' | 'revise' | 'escalate'
export interface JudgeReview {
  id: string
  model: string | null
  verdict: Verdict
  summary: string
  issues: string[]
  createdAt: string
}

export async function listJudgeReviews(taskId: string): Promise<JudgeReview[]> {
  const sql = await db()
  return (await sql`
    select id, model, verdict, summary, issues, created_at as "createdAt"
    from judge_reviews where task_id = ${taskId} order by created_at desc
  `) as unknown as JudgeReview[]
}

export type JudgeMode = 'off' | 'advisory' | 'enforcing'
/** Max times the enforcing loop bounces a ticket back before escalating. */
const MAX_REVISIONS = 3

/** Resolve the EFFECTIVE judge mode for a board (inherit → global config). */
async function shouldJudge(boardId: string): Promise<{ run: boolean; model: string | null; mode: JudgeMode }> {
  const cfg = await getJudgeConfig()
  const sql = await db()
  const [row] = (await sql`select judge_mode as "mode" from boards where id = ${boardId}`) as unknown as Array<{ mode: string }>
  const raw = row?.mode ?? 'inherit'
  const mode: JudgeMode = raw === 'off' ? 'off' : raw === 'advisory' ? 'advisory' : raw === 'enforcing' ? 'enforcing' : cfg.enabled ? 'advisory' : 'off'
  return { run: mode !== 'off', model: cfg.model, mode }
}

const SYSTEM = `You are a meticulous, skeptical QA reviewer for a task tracker. An agent has completed a ticket and reported its outcome. Judge whether the work credibly satisfies the ticket.

Return ONLY a JSON object, no prose around it:
{"verdict": "pass" | "revise" | "escalate", "summary": "<2-4 sentence assessment>", "issues": ["<specific, actionable issue>", ...]}

- "pass": the reported outcome credibly and completely satisfies the ticket.
- "revise": concrete gaps, unmet requirements, or likely defects the agent should fix. List them in issues.
- "escalate": needs a human decision — ambiguous/contradictory requirements, a risky or irreversible action, or a claim you cannot assess. Explain in issues.

Be concrete. Prefer "revise" over "pass" when the outcome is vague, unverifiable, or skips a requirement. Judge the WORK, not the writing.`

function buildPrompt(task: {
  title: string
  description?: string | null
  outcome?: string | null
  resolution?: string | null
  errorMessage?: string | null
}): string {
  const parts = [`TICKET: ${task.title}`]
  if (task.description) parts.push(`\nREQUIREMENTS:\n${task.description}`)
  parts.push(`\nAGENT REPORTED OUTCOME:\n${task.outcome || '(none provided)'}`)
  if (task.resolution) parts.push(`\nHOW IT WAS RESOLVED:\n${task.resolution}`)
  if (task.errorMessage) parts.push(`\nREPORTED ERROR:\n${task.errorMessage}`)
  return parts.join('\n')
}

/** Pull the JSON verdict out of a model response (tolerates code fences/prose). */
function parseVerdict(text: string): { verdict: Verdict; summary: string; issues: string[] } {
  const escalate = (summary: string) => ({ verdict: 'escalate' as Verdict, summary, issues: [] as string[] })
  const match = text.match(/\{[\s\S]*\}/)
  if (!match) return escalate('Judge returned no parseable verdict — surfacing to a human.')
  try {
    const raw = JSON.parse(match[0]) as { verdict?: string; summary?: string; issues?: unknown }
    const verdict: Verdict = raw.verdict === 'pass' || raw.verdict === 'revise' ? raw.verdict : 'escalate'
    const issues = Array.isArray(raw.issues) ? raw.issues.map((i) => String(i)).filter(Boolean).slice(0, 20) : []
    return { verdict, summary: String(raw.summary ?? '').slice(0, 4000), issues }
  } catch {
    return escalate('Judge returned malformed JSON — surfacing to a human.')
  }
}

/** Run the judge for a task now (best-effort; swallows its own errors so it can
 *  be fired without blocking the request that triggered the transition). */
export async function runJudgeForTask(taskId: string): Promise<JudgeReview | null> {
  try {
    const sql = await db()
    const [task] = (await sql`
      select board_id as "boardId", title, description, outcome, resolution, error_message as "errorMessage"
      from tasks where id = ${taskId}
    `) as unknown as Array<{ boardId: string; title: string; description: string | null; outcome: string | null; resolution: string | null; errorMessage: string | null }>
    if (!task) return null
    const { run, model, mode } = await shouldJudge(task.boardId)
    if (!run) return null

    // Layered tiering: a cheap structural pre-pass (gate-safe guard rules, e.g.
    // secret-leak) over the reported outcome, fed to the judge as evidence.
    const preFindings = await guardText(`${task.outcome ?? ''}\n${task.resolution ?? ''}`).catch(() => [])
    const preNote = preFindings.length
      ? `\n\nAUTOMATED PRE-CHECKS FLAGGED (weigh these):\n${preFindings.map((f) => `- ${f.check.replace(/_/g, ' ')}: ${f.message}`).join('\n')}`
      : ''
    if (preFindings.length) {
      await recordFindings(preFindings, { caller: `ticket:${taskId}`, model: model ?? 'pl-main', endpoint: null, mode: 'observe' }).catch(() => {})
    }

    const { text } = await completeViaGateway(
      model ?? 'pl-main',
      [
        { role: 'system', content: SYSTEM },
        { role: 'user', content: buildPrompt(task) + preNote },
      ],
      { temperature: 0, caller: `judge${model ? `:${model}` : ''}` },
    )
    const { verdict, summary, issues } = parseVerdict(text)

    const [row] = (await sql`
      insert into judge_reviews (task_id, model, verdict, summary, issues)
      values (${taskId}, ${model}, ${verdict}, ${summary}, ${sql.json(issues)})
      returning id, model, verdict, summary, issues, created_at as "createdAt"
    `) as unknown as JudgeReview[]
    const actor = `judge${model ? `:${model}` : ''}`
    const label = `QA judge: ${verdict}${issues.length ? ` (${issues.length} issue${issues.length > 1 ? 's' : ''})` : ''}`
    await sql`insert into task_activity (task_id, actor, type, description) values (${taskId}, ${actor}, 'judge', ${label})`

    // Enforcing mode: bounce a "revise" back to the agent with the issues, bounded
    // by MAX_REVISIONS, then stop looping and escalate to a human. "pass" and
    // "escalate" always go to the human (never auto-approve).
    if (mode === 'enforcing' && verdict === 'revise') {
      const revRows = (await sql`
        select count(*)::int as n from judge_reviews where task_id = ${taskId} and verdict = 'revise'
      `) as unknown as Array<{ n: number }>
      const reviseCount = revRows[0]?.n ?? 0
      if (reviseCount <= MAX_REVISIONS) {
        const feedback =
          `**QA judge requested changes** (revision ${reviseCount}/${MAX_REVISIONS})\n\n${summary}` +
          (issues.length ? `\n\n${issues.map((i) => `- ${i}`).join('\n')}` : '')
        await sql`insert into task_comments (task_id, author, content) values (${taskId}, ${actor}, ${feedback})`
        await sql`update tasks set status = 'in_progress', updated_at = now() where id = ${taskId}`
        await sql`insert into task_activity (task_id, actor, type, description) values (${taskId}, ${actor}, 'status', ${`sent back for revision (${reviseCount}/${MAX_REVISIONS})`})`
      } else {
        await sql`insert into task_activity (task_id, actor, type, description) values (${taskId}, ${actor}, 'judge', ${`revision limit reached (${MAX_REVISIONS}) — needs a human`})`
      }
    }

    publishBoard(task.boardId, { type: 'task', taskId })
    return row ?? null
  } catch (err) {
    if (import.meta.env.DEV) console.error('[judge] failed:', err)
    return null
  }
}
