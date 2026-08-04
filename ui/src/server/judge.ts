// Automated QA judge — the reliability gate on agent-completed work.
//
// When a ticket reaches quality_review, a judge (a configurable, ideally strong
// model reached over the Talaria gateway) reviews the agent's reported outcome
// against the ticket and posts an ADVISORY verdict + specific issues. The human
// reviewer still decides — the judge sharpens that decision, it doesn't replace
// it. (An enforcing revision-loop mode is planned; advisory ships first.)

import { completeViaGateway } from './llm-gateway'
import { guardText, recordFindings } from './guardrails'
import { resolveTemplate } from './templates'
import { getSetting, setSetting } from './audit'
import { db } from './db/pg'
import { publishBoard } from './realtime'
import { addNotification } from './notifications'
// './tasks' and './approvals' are reached with `await import` further down, not
// statically: tasks.ts imports THIS file (`listJudgeReviews`) and approvals.ts
// imports tasks.ts, so either static edge would close a cycle.

export interface JudgeConfig {
  enabled: boolean
  /** Model id (an alias/endpoint from the registry). null → a safe default. */
  model: string | null
  /** The GLOBAL stance boards inherit: enforcing = bad submissions never sit
   *  in QA (revise bounces back to the agent); advisory = verdicts only. */
  mode: 'advisory' | 'enforcing'
}

const CONFIG_KEY = 'judge_config'
const DEFAULT_CONFIG: JudgeConfig = { enabled: false, model: null, mode: 'enforcing' }

export const getJudgeConfig = async (): Promise<JudgeConfig> => ({
  ...DEFAULT_CONFIG,
  ...(await getSetting<Partial<JudgeConfig>>(CONFIG_KEY, {})),
})
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
  const mode: JudgeMode = raw === 'off' ? 'off' : raw === 'advisory' ? 'advisory' : raw === 'enforcing' ? 'enforcing' : cfg.enabled ? cfg.mode : 'off'
  return { run: mode !== 'off', model: cfg.model, mode }
}

const SYSTEM = `You are a meticulous, skeptical QA reviewer for a task tracker. An agent has completed a ticket and reported its outcome. Judge whether the work credibly satisfies the ticket.

Return ONLY a JSON object, no prose around it:
{"verdict": "pass" | "revise" | "escalate", "summary": "<2-4 sentence assessment>", "issues": ["<specific, actionable issue>", ...]}

- "pass": the reported outcome credibly and completely satisfies the ticket.
- "revise": concrete gaps, unmet requirements, or likely defects the agent should fix. List them in issues.
- "escalate": needs a human decision — ambiguous/contradictory requirements, a risky or irreversible action, or a claim you cannot assess. Explain in issues.

When a TICKET TEMPLATE is provided, treat it as the objective rubric: the ticket's requirements are its sections. Check each template section is meaningfully addressed by the ticket and its outcome ("n/a" only where truly inapplicable). A section that is missing, empty, or still skeleton text is a concrete "revise" issue — name the section.

Be concrete. Prefer "revise" over "pass" when the outcome is vague, unverifiable, or skips a requirement. Judge the WORK, not the writing.`

function buildPrompt(
  task: {
    title: string
    description?: string | null
    outcome?: string | null
    resolution?: string | null
    errorMessage?: string | null
  },
  template?: { name: string; body: string } | null,
): string {
  const parts = [`TICKET: ${task.title}`]
  if (task.description) parts.push(`\nREQUIREMENTS:\n${task.description}`)
  if (template) {
    parts.push(`\nTICKET TEMPLATE ("${template.name}" — the rubric this ticket is expected to follow):\n<<<\n${template.body}\n>>>`)
  }
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

/** Tell the people who can act on it that the gate stopped.
 *
 *  WHO — and how much — is `audienceFor` in server/approvals.ts, the one answer
 *  to that question in the product. The authority is `{ by: 'board' }`: this
 *  escalation IS the `ticket_review` approval, and both routes that end one
 *  (api/tasks.$id PATCH, api/tasks.$id.review POST) require
 *  `canEdit(boardRole(user, boardId))`. Board editors are therefore exactly the
 *  people who can approve it, ask for changes, or close it.
 *
 *  WHAT THIS REPLACED, because it was wrong in both directions at once:
 *  `owners.length ? owners : await adminUserIds()`, under a comment claiming it
 *  was "the same rule" as the approvals path and citing a function that does not
 *  exist. On an unassigned ticket it sent the ticket's TITLE and the judge's
 *  issue list to every org admin — including admins with no membership of that
 *  board, the disclosure the approval escalation had just closed, through a
 *  different door — while the board's own editors, the only people who could do
 *  anything about it, were never told at all.
 *
 *  When the board has no editors, nobody can act on it. The FACT still travels,
 *  because an agent's finished work is now parked indefinitely, so the admins
 *  get a stall report — no ticket title, no verdict, no issue list, and no deep
 *  link they would be 403'd from. Adding an editor to that board is a thing an
 *  admin can do; deciding the ticket is not. `fact` is non-empty only when
 *  `content` is empty, so nobody is told twice.
 *
 *  The kind IS the class (`judge_escalation`): `notifyClassOf` accepts a class
 *  id directly, so the "Judge escalations" control in Settings governs this
 *  line and no mapping table needs an entry. Until this call site existed that
 *  control configured nothing at all.
 *
 *  Best-effort and logged, never thrown: the judge runs detached from the
 *  request that moved the ticket, and a notification failure must not lose the
 *  verdict row that was already written. */
async function tellHumansTheGateStopped(
  task: { boardId: string },
  taskId: string,
  n: { title: string; body: string },
): Promise<void> {
  try {
    // Dynamic for the same reason as the imports at the top of this file:
    // approvals.ts → tasks.ts → judge.ts, so a static edge would close a cycle.
    const { audienceFor } = await import('./approvals')
    const who = await audienceFor({ by: 'board', boardId: task.boardId })
    for (const userId of who.content) {
      await addNotification(userId, {
        kind: 'judge_escalation',
        title: n.title,
        body: n.body,
        href: `/boards/${task.boardId}/${taskId}`,
      }).catch((e: unknown) => console.error(`[judge] could not notify ${userId} of an escalation:`, e))
    }
    for (const userId of who.fact) {
      await addNotification(userId, {
        kind: 'judge_escalation',
        title: 'A QA gate stopped on a board nobody can act on',
        body:
          'The quality gate handed a ticket back to a person, and that board has no members who can ' +
          'approve it, ask for changes or close it — so the work is parked indefinitely and the details ' +
          'are not shown here. Add an editor to the board and they will be able to see it and decide it.',
      }).catch((e: unknown) => console.error(`[judge] could not report a stalled escalation to ${userId}:`, e))
    }
  } catch (e) {
    console.error('[judge] could not raise the escalation notification:', e)
  }
}

/** Run the judge for a task now (best-effort; swallows its own errors so it can
 *  be fired without blocking the request that triggered the transition). */
export async function runJudgeForTask(taskId: string): Promise<JudgeReview | null> {
  try {
    const sql = await db()
    const [task] = (await sql`
      select board_id as "boardId", title, description, outcome, resolution, error_message as "errorMessage", assigned_to as "assignedTo"
      from tasks where id = ${taskId}
    `) as unknown as Array<{ boardId: string; title: string; description: string | null; outcome: string | null; resolution: string | null; errorMessage: string | null; assignedTo: string | null }>
    if (!task) return null
    const { run, model, mode } = await shouldJudge(task.boardId)
    if (!run) return null

    // Template conformance: resolve the same chain ticket creation uses
    // (assignee's binding → board default) and hand the judge the skeleton as
    // an objective rubric.
    const template = await resolveTemplate('ticket', { agentModel: task.assignedTo, boardId: task.boardId }).catch(() => null)

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
        { role: 'user', content: buildPrompt(task, template) + preNote },
      ],
      { temperature: 0, caller: 'platform:judge' },
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

    const detail = [summary, ...issues.map((i) => `- ${i}`)].filter(Boolean).join('\n')

    // "escalate" is the judge saying a HUMAN has to decide this — ambiguous
    // requirements, a risky action, a claim it cannot assess, or a verdict it
    // could not even parse. In advisory mode nothing moves the ticket and in
    // enforcing mode nothing bounces it, so before this line the entire effect
    // of an escalation was a row in an activity feed on a ticket nobody had a
    // reason to open. That is the silence this milestone exists to end.
    if (verdict === 'escalate') {
      await tellHumansTheGateStopped(task, taskId, {
        title: `QA judge escalated: ${task.title}`,
        body: `The quality gate could not sign this off and handed it to a person.\n\n${detail}`,
      })
    }

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
        // Bounce to the board's own first ACTIVE column (custom statuses
        // included), THROUGH updateTask — so the move validates, notifies,
        // and re-fires the dispatch push: the agent gets the work back in
        // its loop with the judge's feedback waiting on the ticket. The judge
        // writes as the PLATFORM, not as an agent: the human-in-the-loop
        // invariant is Talaria's to enforce, and sending work back for
        // revision is the one move only it makes.
        const { statusMeta } = await import('./statuses')
        const meta = await statusMeta(task.boardId)
        // The bounce destination is a DESTINATION, so it comes from statusMeta's
        // `placeable` list — `activeKey`, the same field the dispatch prompt and
        // the human reviewer's "request changes" now use. This was
        // `listStatuses(...).find(st => st.category === 'active')?.key`, which
        // does not exclude terminal columns: on a board whose first active column
        // is labelled "Cancelled" the judge CLOSED the ticket it meant to send
        // back for revision.
        const active = meta.activeKey ?? meta.assignedKey
        const { updateTask } = await import('./tasks')
        // No raw-SQL fallback: forcing the status past updateTask skips every
        // validation the board's columns exist for, and a bounce that can't
        // land belongs to the human reviewer, who still has the ticket.
        const bounced = await updateTask(taskId, { status: active as import('./tasks').TaskStatus }, { kind: 'platform', id: actor })
          .then(() => true)
          .catch(() => false)
        const note = bounced
          ? `sent back for revision (${reviseCount}/${MAX_REVISIONS})`
          : `could not send back for revision (${reviseCount}/${MAX_REVISIONS}) — left for a human`
        await sql`insert into task_activity (task_id, actor, type, description) values (${taskId}, ${actor}, 'status', ${note})`
      } else {
        await sql`insert into task_activity (task_id, actor, type, description) values (${taskId}, ${actor}, 'judge', ${`revision limit reached (${MAX_REVISIONS}) — needs a human`})`
        // The loop has given up. The agent will not be asked again, the ticket
        // is parked in review, and the ONLY thing that changes it now is a
        // person — so a person is told, for the same reason as above.
        await tellHumansTheGateStopped(task, taskId, {
          title: `QA judge gave up after ${MAX_REVISIONS} revisions: ${task.title}`,
          body:
            `The agent has been sent back ${MAX_REVISIONS} times and the work still does not satisfy the ticket. ` +
            `It stays in review until you approve it, ask for changes yourself, or close it.\n\n${detail}`,
        })
      }
    }

    publishBoard(task.boardId, { type: 'task', taskId })
    return row ?? null
  } catch (err) {
    if (import.meta.env.DEV) console.error('[judge] failed:', err)
    return null
  }
}
