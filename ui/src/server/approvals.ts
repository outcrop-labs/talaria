// The approval census — every thing a human is currently expected to approve,
// in one shape, with the person who owes the answer attached.
//
// WHY THIS FILE EXISTS
//   "Approvals" is not one queue in Talaria. It is five, and they were written
//   at five different times by five different features:
//
//     google_action    google_pending_actions — an agent drafted something that
//                      LEAVES the building (an email, an invite). Surfaced by
//                      the Inbox focus queue as sourceType 'approval'.
//     workbench_plan   workbench_jobs.status = 'awaiting_approval' — an agent
//                      wrote a plan and is BLOCKED until someone says build it.
//                      Surfaced by WorkbenchJobsStrip on the ticket.
//     ticket_review    a ticket sitting in a review-category column — an agent
//                      handed work over and only a person can sign it off
//                      (server/tasks.ts refuses to let an agent take it back).
//                      Surfaced by Home's review queue and the focus queue.
//     repo_request     workbench_repo_requests — an agent asked for a repo that
//                      does not exist. Admin-only; Admin → Agents.
//     run_decision     runs.state = 'awaiting' — a durable run (server/runs/)
//                      reached a question its own step cannot answer and
//                      PARKED. The fifth kind, and the first one with no table
//                      of its own: the run IS the record, and the question is a
//                      column on it. It has no surface of its own yet, which is
//                      exactly why it has to be here — until the runs strip
//                      lands, the announcement below is the only way anybody
//                      hears that a run stopped to ask.
//
//   Each of those has its own table, its own surface and its own idea of who
//   decides. None of them had an idea of how OLD it was, which is the whole
//   problem: an approval nobody looks at is indistinguishable from no approval
//   at all, and the agent on the other side of it waits for ever.
//
//   So this module answers one question — "what is waiting on a human, since
//   when, and on WHICH human" — for the digest (which counts them) and the
//   escalation job (which ages them). Neither of those should know four table
//   shapes, and neither should be the place a fifth approval kind gets
//   forgotten: add it HERE and both surfaces pick it up.
//
// WHAT IS DELIBERATELY NOT HERE
//   · capability_gaps — the ratification queue is explicitly a no-nag surface
//     (see server/gaps.ts: "delivery is the Studio's Suggested queue, not inbox
//     pings"). Nagging it would break a promise that file makes on purpose.
//   · triage / blocked tickets — work waiting for a person, but not a DECISION
//     on someone else's finished work. The digest reports them; nothing
//     escalates them, because there is no second party blocked on the answer.
//   · inbox_decisions confirmation tokens — a 10-minute in-session handshake
//     that expires on its own. Nothing ages.
//
// THE DISCLOSURE RULE — read this before adding a kind
//   An approval's title and detail are CONTENT. A personal confirm-send is one
//   person's mail; a workbench plan is a board's engineering; a ticket sign-off
//   names a ticket on a board. "It has been waiting a long time" is never a
//   reason to show any of that to someone new, because the person it would
//   reach cannot decide it either — every decision route below refuses them.
//
//   So each kind declares the AUTHORITY its decision route enforces, and that
//   authority is the only audience anything downstream may address:
//
//     kind             route                                  authority
//     google_action    google/pending-actions decideAction:   { by: 'user' } — the owner ALONE
//       (personal)       `action.ownerUserId === actor.id`      (an admin is refused)
//     google_action    same, `actor.isAdmin`                   { by: 'admin' }
//       (org)
//     workbench_plan   api/workbench.jobs PUT:                 { by: 'board' }
//                        `canEdit(boardRole(user, boardId))`     (admins get NO bypass)
//     ticket_review    api/tasks.$id PATCH / .review POST:     { by: 'board' }
//                        `canEdit(boardRole(user, boardId))`
//     repo_request     admin surface                           { by: 'admin', onBoard }
//                        (any admin may GRANT it; `onBoard` is the ticket the
//                        agent raised it from, and it bounds who may read the
//                        agent's free text — not who may act)
//     run_decision     runs/decide.ts decide():                DECLARED BY THE RUN:
//                        `mayDecide(census, approval, by)`       `RunDefinition.audience(run)`
//                        — the same predicate this file           returns one of the four
//                        already exports, against this            above
//                        same census
//
//   `run_decision` is the first kind that does not hard-code its authority, and
//   that is the point of it rather than a shortcut: a research run pauses to its
//   OWNER and a work-session run pauses to the ticket's BOARD, and neither
//   definition knows how disclosure works — it names an authority and this file
//   resolves it. A run kind that wanted to invent its own audience would have to
//   go around `audienceFor`, which is the one thing check-invariants fails on.
//
//   `audienceFor()` resolves that to a list of user ids. The digest filters on
//   it and the SLA job addresses it, so neither can widen an audience by
//   accident, and a new kind cannot be added without answering the question.
//
//   It is no longer only an approvals question — see WHO MAY BE TOLD, AND HOW
//   MUCH further down. The judge's QA escalation and the capability-gap
//   announcement resolve their audience through the same function.
import { db } from './db/pg'
import { humanAssigneeIds } from './tasks'
import { addNotification } from './notifications'
import { getSetting } from './audit'
import { statusCategorySql } from './statuses'
// runs/define.ts is types, one identity function and a registry Map — it
// imports NOTHING at runtime (its only import of this file is `import type
// { Authority }`, which is erased). So this edge is one-way and there is no
// cycle to break with a dynamic import, which is what makes it safe for the
// census to ask a run's own definition who may decide it.
import { runDefinition, type RunRow } from './runs/define'

export type ApprovalKind = 'google_action' | 'workbench_plan' | 'ticket_review' | 'repo_request' | 'run_decision'

/** Exactly who a route lets act — and therefore exactly who may be told what
 *  the thing IS. Nothing else in the codebase may widen this.
 *
 *  Declared for approvals first; it is now the discriminator for every "who
 *  should be told about this thing" question in the product. See WHO MAY BE
 *  TOLD, AND HOW MUCH, below `boardEditors`. */
export type Authority =
  /** These users and nobody else. A personal Google confirm-send: not the
   *  admins, not the board — the one person whose mailbox it is. */
  | { by: 'user'; userIds: string[] }
  /** Every admin. Org-scoped things an admin can already open in-app.
   *
   *  `onBoard` narrows the CONTENT to the admins who can also see that board,
   *  for a thing that is an admin's to action but quotes one board's work: a
   *  capability gap an agent raised mid-ticket, whose free text routinely names
   *  the ticket, the customer or the file. The rest of the admins get the fact
   *  and not the words. Omitted or null = org-wide, every admin. */
  | { by: 'admin'; onBoard?: string | null }
  /** The board's EDITORS (`canEdit`), which is what the ticket and workbench
   *  routes check. Admin is not a bypass: `boardRole` is membership only. */
  | { by: 'board'; boardId: string }
  /** No route in the product can decide this. An orphaned workbench job, a
   *  personal action whose owner is gone. Resolves to nobody, and the stall is
   *  reported to the admins without the content. */
  | { by: 'nobody' }

/** The approvals-shaped name for it, kept because the census and the table at
 *  the top of this file are written in those terms. Same type. */
export type ApprovalAuthority = Authority

export interface PendingApproval {
  kind: ApprovalKind
  /** Stable across runs — the dedupe key the escalation job records against. */
  key: string
  id: string
  /** One line. Goes in a subject, a list item, or a notification title.
   *  CONTENT: only the authority below may be shown it. */
  title: string
  /** What the reader needs to decide it, in one sentence. Also CONTENT. */
  detail: string
  /** In-app path to the surface that can actually decide it. */
  href: string
  /** When a human became responsible. This is what ages. */
  waitingSince: string
  /** The named person(s) already on the hook — always a SUBSET of the
   *  authority, never a widening of it. Who gets nagged first; empty means
   *  nobody in particular owes the answer. */
  ownerUserIds: string[]
  /** Who may decide it, and so who may be told what it is. */
  authority: ApprovalAuthority
}

const iso = (v: string | Date): string => (v instanceof Date ? v.toISOString() : new Date(v).toISOString())

/** Human owners of a ticket-anchored approval: the ticket's HUMAN assignees.
 *
 *  Not watchers, and not "everyone on the board". A watcher subscribed to
 *  outcomes; nagging them for someone else's decision is the noise that gets a
 *  sender filtered. An approval with no human assignee has no owner at all —
 *  `ownerUserIds` is empty and the nag stage has nobody to write to. The
 *  escalation stage then addresses the ticket's board editors, who are the
 *  people the decision route was always going to let act. */
const ticketOwners = (assignees: unknown): string[] =>
  Array.isArray(assignees) ? humanAssigneeIds(assignees.filter((a): a is string => typeof a === 'string')) : []

async function googleActionApprovals(): Promise<PendingApproval[]> {
  const sql = await db()
  const rows = (await sql`
    select id, kind, summary, agent_model as "agentModel", owner_user_id as "ownerUserId",
           is_org as "isOrg", created_at as "createdAt"
    from google_pending_actions where status = 'pending'
  `) as unknown as Array<{
    id: string
    kind: string
    summary: string | null
    agentModel: string | null
    ownerUserId: string | null
    isOrg: boolean
    createdAt: string
  }>
  return rows.map((row) => {
    const what = row.kind === 'gmail_send' ? 'send an email' : 'create a calendar event'
    // The summary belongs in the TITLE, not only the body: this string is a
    // notification headline and an email subject fragment, and "an agent wants
    // to send an email" is not a thing anyone can decide from.
    const summary = row.summary?.trim()
    return {
      kind: 'google_action' as const,
      key: `google_action:${row.id}`,
      id: row.id,
      title: `${row.agentModel ?? 'An agent'} wants to ${what}${summary ? `: ${summary}` : ''}`,
      detail: row.summary?.trim() || `A drafted ${row.kind.replaceAll('_', ' ')} is held until someone approves it.`,
      // The focus queue is the surface that owns this decision.
      href: '/',
      waitingSince: iso(row.createdAt),
      ownerUserIds: row.isOrg || !row.ownerUserId ? [] : [row.ownerUserId],
      // The route: org → `actor.isAdmin`; personal → `ownerUserId === actor.id`,
      // which refuses an admin outright. A personal confirm-send is one
      // person's mail or calendar and an admin is not entitled to its subject
      // line by virtue of being an admin — they cannot action it either.
      authority: row.isOrg
        ? { by: 'admin' as const }
        : row.ownerUserId
          ? { by: 'user' as const, userIds: [row.ownerUserId] }
          : { by: 'nobody' as const },
    }
  })
}

async function workbenchPlanApprovals(): Promise<PendingApproval[]> {
  const sql = await db()
  const rows = (await sql`
    select j.id, j.agent_model as "agentModel", j.repo, j.effort, j.plan,
           j.task_id as "taskId", j.created_at as "createdAt",
           t.title as "taskTitle", t.board_id as "boardId", t.assignees,
           case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end as "ticketRef"
    from workbench_jobs j
    left join tasks t on t.id = j.task_id
    left join boards b on b.id = t.board_id
    where j.status = 'awaiting_approval'
      and (t.id is null or (t.archived_at is null and b.archived_at is null))
  `) as unknown as Array<{
    id: string
    agentModel: string
    repo: string
    effort: string
    plan: string
    taskId: string | null
    createdAt: string
    taskTitle: string | null
    boardId: string | null
    assignees: unknown
    ticketRef: string | null
  }>
  return rows.map((row) => ({
    kind: 'workbench_plan' as const,
    key: `workbench_plan:${row.id}`,
    id: row.id,
    title: `${row.agentModel} is waiting to build on ${row.repo}`,
    detail:
      `${row.effort}-effort work on ${row.repo}` +
      (row.ticketRef || row.taskTitle ? ` for ${row.ticketRef ?? ''}${row.ticketRef && row.taskTitle ? ' · ' : ''}${row.taskTitle ?? ''}` : '') +
      `. ${row.plan.trim().slice(0, 240) || 'The agent posted a plan and stopped.'}`,
    href: row.taskId && row.boardId ? `/boards/${row.boardId}/${row.taskId}` : '/',
    waitingSince: iso(row.createdAt),
    ownerUserIds: ticketOwners(row.assignees),
    // api/workbench.jobs PUT requires `canEdit(boardRole(user, task.boardId))`
    // and 403s when the job has no task at all. A job with no ticket is
    // therefore decidable by NOBODY — which is a real stall to report, not a
    // reason to hand an agent's plan to every admin in the workspace.
    authority: row.boardId ? { by: 'board' as const, boardId: row.boardId } : { by: 'nobody' as const },
  }))
}

async function ticketReviewApprovals(): Promise<PendingApproval[]> {
  const sql = await db()
  // THE CLOCK. `tasks.updated_at` is NOT when this started waiting — it is when
  // the row was last written, and every later touch (a comment's side effects, a
  // label, an estimate, an agent patching its own outcome) resets it. Combined
  // with "escalate once, ever", an actively-edited ticket escalates arbitrarily
  // late or never: the busiest ticket in review is the one the SLA forgets.
  //
  // The ticket's own history knows the answer. `server/tasks.ts` writes a
  // task_activity row of type 'status' — `moved to <key>` (plus an optional
  // "(why)") — every time the column changes, however it changed. The LATEST
  // such row for the status the ticket is in NOW is the moment it entered
  // review, and nothing but another column move can move it. `starts_with`
  // rather than LIKE because a status key is user-defined and `_` is a LIKE
  // wildcard; matching on the exact string or the exact string plus " (" cannot
  // collide with a longer key.
  //
  // Fallback stays `updated_at` for tickets that entered review before their
  // history was kept, or were created straight into it: younger than the truth,
  // so a late nag rather than a false one.
  const rows = (await sql`
    select t.id, t.title, t.board_id as "boardId", t.assignees, t.status,
           t.updated_at as "updatedAt", b.name as board,
           (select max(a.created_at) from task_activity a
             where a.task_id = t.id and a.type = 'status'
               and (a.description = 'moved to ' || t.status
                    or starts_with(a.description, 'moved to ' || t.status || ' ('))
           ) as "enteredAt",
           case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end as "ticketRef"
    from tasks t
    join boards b on b.id = t.board_id
    where t.archived_at is null and b.archived_at is null
      and ${sql.unsafe(statusCategorySql('review', ['quality_review']))}
  `) as unknown as Array<{
    id: string
    title: string
    boardId: string
    assignees: unknown
    status: string
    updatedAt: string
    enteredAt: string | null
    board: string
    ticketRef: string | null
  }>
  return rows.map((row) => ({
    kind: 'ticket_review' as const,
    key: `ticket_review:${row.id}`,
    id: row.id,
    title: `${row.ticketRef ? `${row.ticketRef} · ` : ''}${row.title} needs sign-off`,
    detail: `Finished work is parked in ${row.board}'s review column until a person approves it or asks for changes.`,
    href: `/boards/${row.boardId}/${row.id}`,
    waitingSince: iso(row.enteredAt ?? row.updatedAt),
    ownerUserIds: ticketOwners(row.assignees),
    // Sign-off is a column move, and both routes that perform one
    // (api/tasks.$id PATCH, api/tasks.$id.review POST) require
    // `canEdit(boardRole(user, task.boardId))`. Board editors, therefore — and
    // an admin who is not on the board is not one of them.
    authority: { by: 'board' as const, boardId: row.boardId },
  }))
}

async function repoRequestApprovals(): Promise<PendingApproval[]> {
  const sql = await db()
  // `why` is the AGENT'S free text, written while working a ticket, so it
  // routinely quotes that board's work — the same reason a capability gap
  // carries `onBoard`. The request is still an ADMIN's to grant (the route is
  // admin-gated), so the authority stays `admin`; `onBoard` only narrows who
  // may read the words. `task_id` is set on every request the workbench
  // raises, and is nullable in the schema, so a request without one is
  // genuinely org-wide rather than assumed to be.
  const rows = (await sql`
    select r.id, r.agent_model as "agentModel", r.org, r.name, r.why,
           r.created_at as "createdAt", t.board_id as "boardId"
    from workbench_repo_requests r
    left join tasks t on t.id = r.task_id
    where r.status = 'pending'
  `) as unknown as Array<{
    id: string
    agentModel: string
    org: string
    name: string
    why: string
    createdAt: string
    boardId: string | null
  }>
  return rows.map((row) => ({
    kind: 'repo_request' as const,
    key: `repo_request:${row.id}`,
    id: row.id,
    title: `${row.agentModel} asked for a new repo: ${row.org}/${row.name}`,
    detail: row.why.trim().slice(0, 240) || 'The agent needs a repository that does not exist yet.',
    href: '/admin/agents',
    waitingSince: iso(row.createdAt),
    ownerUserIds: [],
    authority: { by: 'admin' as const, onBoard: row.boardId },
  }))
}

// ── run_decision: a durable run parked on a question ─────────────────────────
//
// The other four kinds read a table that exists to hold a pending decision. This
// one reads `runs`, where the decision is a COLUMN on the work itself, and that
// difference is the whole reason the state is worth having: a run in `awaiting`
// is not failed (nothing is wrong), not running (nothing is burning) and not
// queued (no amount of driving advances it — server/runs/run.ts refuses to drive
// it). It is a piece of work stopped at a fork, with its checkpoint intact,
// waiting for a person. server/research.ts answers the same event today by
// marking the user's run FAILED.
//
// A row is skipped, LOUDLY and unmarked, rather than announced as widely as
// possible whenever this process cannot establish who may decide it. That is the
// opposite of the reflex, so it is worth stating: an approval that is announced
// to the wrong people cannot be un-announced, while an approval that goes round
// again on the next tick costs five minutes. A skipped key is never marked, so
// the moment an instance that HAS the definition runs a sweep, it is announced
// normally.
//
// ONE DEBT THIS KIND OPENS, for whoever ports the first run onto the runner: a
// definition whose `audience` returns `{ by: 'nobody' }` lands in the default
// branch of `whyUnreachable` in server/digest.ts, which tells an admin "It is
// not attached to a ticket, so no surface in Talaria can approve or reject it".
// That sentence was written about a workbench job and is false about a run. No
// run kind resolves to `nobody` today — nothing is ported onto the runner in
// this change — so it is a gap rather than a live bug, and the first kind that
// CAN resolve to nobody owes that branch a sentence of its own.

/** The `runs` row as this file reads it — every column, because `audience(run)`
 *  is the run definition's own code and it is handed a whole `RunRow`. A subset
 *  would be a `RunRow` with holes in it, and the first definition that keyed its
 *  audience off `input` or `subjectId` would resolve an audience from a null it
 *  was promised would not be there. The set of `awaiting` runs is bounded by the
 *  number of decisions humans currently owe, so this is a small read however
 *  large the run history gets. */
type RawRunRow = Omit<RunRow, 'createdAt' | 'updatedAt' | 'startedAt' | 'finishedAt' | 'leaseExpiresAt'> & {
  createdAt: Date | string
  updatedAt: Date | string
  startedAt: Date | string | null
  finishedAt: Date | string | null
  leaseExpiresAt: Date | string | null
}

const runRowFrom = (raw: RawRunRow): RunRow => ({
  ...raw,
  createdAt: iso(raw.createdAt),
  updatedAt: iso(raw.updatedAt),
  startedAt: raw.startedAt === null ? null : iso(raw.startedAt),
  finishedAt: raw.finishedAt === null ? null : iso(raw.finishedAt),
  leaseExpiresAt: raw.leaseExpiresAt === null ? null : iso(raw.leaseExpiresAt),
})

/** ONE description of what a parked run is as an approval — the census below and
 *  `runs/decide.ts`'s authorization check both go through this function.
 *
 *  Exported for exactly that reason. `decide()` has to answer "may this person
 *  decide this run", and the only correct way to answer it is to build the same
 *  approval the census builds and put it through the same `mayDecide`. A second,
 *  hand-written answer at the decision route is the shape this file's header
 *  spends forty lines on: it is how `request_repo` ended up mailing every admin
 *  the free text the census had carefully bounded to a board.
 *
 *  Returns null when the run is not parked, has no question, or when THIS
 *  process has no definition for its kind. That last one is a real state and not
 *  an impossible one — a row from a newer deploy, or a kind whose module is not
 *  in this instance's import graph — and it is the same call server/runs/run.ts
 *  makes when it declines to drive a kind it does not know: leave it for an
 *  instance that has it. */
export function runDecisionApproval(run: RunRow): PendingApproval | null {
  if (run.state !== 'awaiting') return null
  const request = run.decision?.request
  const key = run.approvalKey
  if (!request || !key) {
    console.error(`[approvals] run ${run.id} (${run.kind}) is awaiting with no ${request ? 'approval key' : 'question'} on the row — nobody can be told about it`)
    return null
  }
  const def = runDefinition(run.kind)
  if (!def) {
    console.warn(
      `[approvals] run ${run.id} is awaiting a decision but kind "${run.kind}" is not registered on this instance — ` +
        `leaving it for one that has it. It stays unannounced and UNMARKED, so the next sweep on an instance that ` +
        `imports the definition announces it normally.`,
    )
    return null
  }
  let authority: Authority
  try {
    // The definition's own code, running inside the census. A throw here means
    // this process cannot say who may be told, and "cannot say" resolves to
    // nobody hearing anything rather than to the widest audience available.
    authority = def.audience(run)
  } catch (e) {
    console.error(`[approvals] run ${run.id} (${run.kind}): its definition could not name an audience, so nobody is told what it says:`, e)
    return null
  }
  const options = request.options.map((o) => o.label.trim()).filter(Boolean)
  const detail = request.detail?.trim() || `${def.label} stopped at "${run.phase}" and cannot go on until somebody chooses.`
  return {
    kind: 'run_decision' as const,
    // The key the DRIVER already wrote on the row. Deriving it a second time
    // here would be a second opinion about the identity of one approval, and the
    // announce marks are keyed on it: the two spellings would announce twice.
    key,
    id: run.id,
    title: `${def.label} needs a decision: ${request.question}`,
    detail: options.length ? `${detail} Options: ${options.join(' · ')}.` : detail,
    // A run that has nowhere to send a reader is not a bug worth suppressing the
    // approval over — the notification still says what the question is, and the
    // runs surface (which will carry the real href) is the next milestone.
    href: request.href ?? '/',
    // WHEN IT PARKED, and unusually for this file that is exactly what
    // `updated_at` means here. Compare the care `ticket_review` needs above: a
    // ticket's row is touched by every later edit, so its timestamp drifts. An
    // `awaiting` run's row is touched by nothing — every write in
    // server/runs/store.ts requires `state = 'running'`, and the only statement
    // that moves a row out of `awaiting` is the answer itself.
    waitingSince: iso(run.updatedAt),
    // The owner is who the run belongs to, NOT proof they may decide it: a
    // work-session run owned by one person can pause to a board they are not an
    // editor of. Every stage that uses `ownerUserIds` intersects it with the
    // resolved audience first, exactly as it does for a ticket's assignees.
    ownerUserIds: run.ownerUserId ? [run.ownerUserId] : [],
    authority,
  }
}

async function runDecisionApprovals(): Promise<PendingApproval[]> {
  const sql = await db()
  const rows = (await sql`
    select id, kind, owner_user_id as "ownerUserId", subject_type as "subjectType", subject_id as "subjectId",
           state, phase, checkpoint, input, result, error, attempt,
           lease_owner as "leaseOwner", lease_expires_at as "leaseExpiresAt",
           approval_key as "approvalKey", decision,
           created_at as "createdAt", updated_at as "updatedAt",
           started_at as "startedAt", finished_at as "finishedAt"
    from runs
    where state = 'awaiting' and approval_key is not null
  `) as unknown as RawRunRow[]
  const out: PendingApproval[] = []
  for (const raw of rows) {
    const approval = runDecisionApproval(runRowFrom(raw))
    if (approval) out.push(approval)
  }
  return out
}

/** Every pending approval in the workspace, oldest first.
 *
 *  One kind failing must not hide the other four — an escalation sweep that
 *  reports nothing because one table was locked is the silence this whole
 *  milestone is about. Each kind is settled independently; a rejection is
 *  logged and rethrown as a partial-failure marker so the caller can say the
 *  census was incomplete rather than say it was empty. */
export async function pendingApprovals(): Promise<{ approvals: PendingApproval[]; failedKinds: ApprovalKind[] }> {
  const sources: Array<[ApprovalKind, Promise<PendingApproval[]>]> = [
    ['google_action', googleActionApprovals()],
    ['workbench_plan', workbenchPlanApprovals()],
    ['ticket_review', ticketReviewApprovals()],
    ['repo_request', repoRequestApprovals()],
    // The newest source is also the one most likely to fail on a tree that has
    // not migrated yet (no `runs` table at all), which is precisely what
    // `failedKinds` is for: a workspace mid-deploy still gets its other four
    // queues, and the sweep below knows not to prune marks it could not see.
    ['run_decision', runDecisionApprovals()],
  ]
  const settled = await Promise.allSettled(sources.map(([, p]) => p))
  const approvals: PendingApproval[] = []
  const failedKinds: ApprovalKind[] = []
  settled.forEach((result, i) => {
    const kind = sources[i]![0]
    if (result.status === 'fulfilled') approvals.push(...result.value)
    else {
      failedKinds.push(kind)
      console.error(`[approvals] could not read pending ${kind} approvals:`, result.reason)
    }
  })
  approvals.sort((a, b) => a.waitingSince.localeCompare(b.waitingSince))
  return { approvals, failedKinds }
}

/** Every admin in the workspace. NOT exported: an admin list is an ingredient of
 *  an audience, never an audience, and the last caller outside this file
 *  (`server/digest.ts`, for the stall report) now reads the `fact` half of the
 *  disclosure the census already resolved. Nothing else may ask the question. */
async function adminUserIds(): Promise<string[]> {
  const sql = await db()
  const rows = (await sql`select id from users where role = 'admin'`) as unknown as Array<{ id: string }>
  return rows.map((r) => r.id)
}

/** The EDITORS of each of these boards — `canEdit(boardRole(user, board))`
 *  expressed as a set instead of a predicate, because both callers need the
 *  whole membership rather than one yes/no.
 *
 *  Same two sources `boardRole` unions: an explicit share of owner/editor rank,
 *  and membership of the board's team (which `boardRole` maps to owner for a
 *  team owner and editor for everyone else — so every team member is at least
 *  an editor). Viewers are excluded because the routes exclude them. */
async function boardEditors(boardIds: string[]): Promise<Map<string, string[]>> {
  const out = new Map<string, string[]>(boardIds.map((id) => [id, []]))
  if (!boardIds.length) return out
  const sql = await db()
  const rows = (await sql`
    select board_id as "boardId", user_id as "userId" from board_members
    where board_id = any(${boardIds}::uuid[]) and role in ('owner', 'editor')
    union
    select b.id as "boardId", tm.user_id as "userId"
    from boards b join team_members tm on tm.team_id = b.team_id
    where b.id = any(${boardIds}::uuid[])
  `) as unknown as Array<{ boardId: string; userId: string }>
  for (const r of rows) out.get(r.boardId)?.push(r.userId)
  return out
}

/** A census plus the one thing every consumer of it needs: WHO each approval
 *  may be shown to. Built together, in one pass, so no caller can hold a list of
 *  approvals without also holding the answer to "who is allowed to read this". */
export interface ApprovalCensus {
  approvals: PendingApproval[]
  failedKinds: ApprovalKind[]
  /** approval key → who may be told about it, and HOW MUCH. `content` is the
   *  exact user ids the decision route would let act, and every audience
   *  downstream — digest section, announcement, nag, escalation — is that set or
   *  a subset of it. `fact` is the people who may be told only THAT it is stuck,
   *  and it is carried here rather than re-derived by each consumer for the
   *  reason the whole file exists: the stall report used to fetch the admin list
   *  itself, so "who may read this" and "who is told it is stuck" were two
   *  answers to one question and could drift apart. */
  audience: Map<string, Disclosure>
}

// ── WHO MAY BE TOLD, AND HOW MUCH ────────────────────────────────────────────
//
// THE ONE ANSWER. This began as the approvals-only question and it is now the
// whole question, for one reason: it had been answered three times, by hand, in
// three files, and two of the three leaked.
//
//   server/approvals.ts  resolved a declared authority to user ids.       ✔
//   server/judge.ts:145  `owners.length ? owners : await adminUserIds()`  ✘
//   server/gaps.ts:84    `select id from users where role = 'admin'`      ✘
//
// The judge's copy failed in BOTH directions at once. On an unassigned ticket
// it sent the ticket's TITLE and the judge's issue list to every org admin —
// including admins with no membership of that board, the exact disclosure the
// approval escalation had just closed, through a different door — and it never
// told the board's own editors, who are the only people who can approve it, ask
// for changes or close it. Its doc comment claimed to implement "the same rule"
// as the approvals path and named a function that does not exist. The gaps copy
// fanned an agent's free text — 160 characters of `missing`, 800 of `needs`,
// written while working one board's ticket — to every admin in the workspace.
//
// So the resolver takes an AUTHORITY and not an approval, every caller imports
// it, and `scripts/check-invariants.mjs` fails the build on a fourth copy: on
// the `owners.length ? owners : adminUserIds()` shape, on `adminUserIds()`
// outside this file, and on `where role = 'admin'` outside this file. Adding a
// new subject means declaring its authority, not writing a new audience.
//
// TWO AUDIENCES, NOT ONE — the distinction the escalation work established and
// the reason this is "and how much of it" rather than just "who". THE FACT MAY
// TRAVEL FURTHER THAN THE CONTENT. Somebody who may not be told WHAT is stuck
// can still be told THAT something is stuck, when they are the person who can
// unblock it: add an editor to that board, grant the tool, ratify the gap. A
// fact report carries no title, no detail and no deep link — the recipient
// would be 403'd from the other end of it and does not need it to act.

/** Who may be told about one thing, split by how much of it they may be told. */
export interface Disclosure {
  /** May be told the whole thing: title, detail, deep link. */
  content: string[]
  /** May be told ONLY that it exists and is stuck, so they can unblock it.
   *  Never overlaps `content`, and never carries a word of the thing itself. */
  fact: string[]
}

/** Resolve a batch of authorities in one pass. Two round trips at most — the
 *  board memberships, then the admin list — shared across every subject and
 *  every recipient, so two people can never be told different things about the
 *  same subject and nobody can be told about one they cannot act on. */
async function resolveDisclosures(authorities: Authority[]): Promise<Disclosure[]> {
  if (!authorities.length) return []
  const editors = await boardEditors([
    ...new Set(
      authorities.flatMap((a) => (a.by === 'board' ? [a.boardId] : a.by === 'admin' && a.onBoard ? [a.onBoard] : [])),
    ),
  ])
  // Everything answerable without the admin list, answered first.
  const direct = authorities.map((a) =>
    a.by === 'user' ? [...new Set(a.userIds)] : a.by === 'board' ? [...new Set(editors.get(a.boardId) ?? [])] : null,
  )
  // The admins are needed by every authority that IS the admins — and by the
  // FACT report of every authority that reached nobody, which is the only
  // reason a `board` or `user` subject ever costs a second query.
  const admins = direct.some((who) => who === null || !who.length) ? await adminUserIds() : []
  return authorities.map((a, i) => {
    const boundedToBoard = a.by === 'admin' && !!a.onBoard
    const content =
      direct[i] ??
      (a.by === 'admin'
        ? boundedToBoard
          ? admins.filter((id) => (editors.get(a.onBoard!) ?? []).includes(id))
          : [...new Set(admins)]
        : [])
    // The fact goes to the admins the content could not reach: all of them when
    // it reached nobody at all, and the off-board ones when it was bounded to a
    // board. Never to somebody who is already getting the content.
    const fact = content.length && !boundedToBoard ? [] : admins.filter((id) => !content.includes(id))
    return { content, fact }
  })
}

/** THE resolver: who may be told about this thing, and how much of it.
 *
 *  Every caller in the product goes through this function. There is exactly one
 *  of it, and `scripts/check-invariants.mjs` asserts that on every run — a rule
 *  that only forbids a SECOND definition keeps passing after the first one is
 *  renamed away from under it. */
export async function audienceFor(authority: Authority): Promise<Disclosure> {
  const [only] = await resolveDisclosures([authority])
  return only ?? { content: [], fact: [] }
}

/** The approvals-shaped call: each approval's declared authority, resolved to
 *  the people its decision route would let act — AND to the people who may be
 *  told only that it is stuck.
 *
 *  Both halves, in one map, because this used to return the content half alone
 *  and every consumer that needed the other one went and fetched the admin list
 *  by hand. That is how an approval bounded to `{ by: 'admin', onBoard }` could
 *  be announced to NOBODY (content empty, no fact path) while the SLA reported
 *  it to admins the census had never heard of. One resolution, one answer. */
export async function approvalAudience(approvals: PendingApproval[]): Promise<Map<string, Disclosure>> {
  const resolved = await resolveDisclosures(approvals.map((a) => a.authority))
  return new Map(approvals.map((a, i) => [a.key, resolved[i] ?? { content: [], fact: [] }]))
}

export async function approvalCensus(): Promise<ApprovalCensus> {
  const { approvals, failedKinds } = await pendingApprovals()
  return { approvals, failedKinds, audience: await approvalAudience(approvals) }
}

/** May this person be told what this approval is? The single predicate the
 *  digest and the SLA job both go through. */
export function mayDecide(census: Pick<ApprovalCensus, 'audience'>, approval: PendingApproval, userId: string): boolean {
  return (census.audience.get(approval.key)?.content ?? []).includes(userId)
}

// ── Announcing an approval when it is RAISED ─────────────────────────────────
//
// The digest is a floor and the SLA is a ceiling; neither is an announcement. A
// confirm-send drafted at 09:05 was, until this existed, first heard about in
// the next morning's digest — the agent stopped for a day over a decision that
// takes four seconds. That latency is the thing this milestone exists to
// remove, so a raised approval is announced to the people who can decide it,
// immediately.
//
// TWO WAYS IN, ONE ANNOUNCER. `announceApproval` is what the code that RAISES
// an approval calls — `server/google/pending-actions.ts` at the end of
// `queueAction`, and `server/workbench-mcp.ts` when `start_job` gates a heavy
// plan or `request_repo` files a repository request. `sweepUnannounced` is the
// safety net that catches everything nobody called it for, on the SLA job's tick
// — so the latency is bounded even for a raiser that never learns about this,
// and adding the direct call later is a latency improvement rather than a
// correctness fix. Both mark the same state, so a raised approval is announced
// exactly once.
//
// ONE ANNOUNCER IS THE WHOLE POINT, and it is not a tidiness argument. A subject
// with two announcement paths has two answers to "who may be told", and only one
// of them asks the resolver. `request_repo` was exactly that: the census
// resolved its row to `{ by: 'admin', onBoard }` while the verb itself, in the
// same request, selected every admin in raw SQL and mailed each of them 120
// characters of the agent's free text. Bounding the census did not bound the
// leak, because the leak was the copy that always fired first. Nothing that
// raises an approval may write its own notification.
//
// EXACTLY ONCE IS A CLAIM ABOUT CONCURRENCY, so the mark is written the way a
// claim like that has to be written — merged in the database, never as a
// read-modify-write over the whole blob. `markAnnounced` is where that is
// argued; it is the difference between "both mark the same key" and "both mark
// the same key AND neither can erase the other's".

const ANNOUNCE_STATE_KEY = 'approval_announce_state'

interface AnnounceState {
  /** approval key → when it was announced. */
  announced: Record<string, string>
}

const announceState = () => getSetting<AnnounceState>(ANNOUNCE_STATE_KEY, { announced: {} })

/** Record marks WITHOUT reading the blob first, because the read-then-write the
 *  rest of this file uses for settings is exactly what would break the promise
 *  above.
 *
 *  `getSetting`/`setSetting` are read-modify-write over a whole jsonb blob, and
 *  the two announce paths OVERLAP IN TIME by design: `announceApproval` fires
 *  from the request that raised the approval, and the sweep's pass is seconds
 *  long — a census, then a notification write per approval per recipient. A
 *  `setSetting` at the end of that pass writes a map that was read BEFORE the
 *  request marked its key, so the fresh mark is erased and the NEXT tick
 *  announces the same approval a second time. "Both mark the same key" is only
 *  true if the marks cannot overwrite each other, and a whole-blob write is
 *  precisely how they would.
 *
 *  So marks are merged in the database instead. `||` on jsonb is a union and
 *  the statement is one row update, so both writers survive whatever the order.
 *  Nothing is read first, so there is no window to lose. */
async function markAnnounced(marks: Record<string, string>): Promise<void> {
  if (!Object.keys(marks).length) return
  const sql = await db()
  await sql`
    insert into app_settings (key, value)
    values (${ANNOUNCE_STATE_KEY}, ${sql.json({ announced: marks } as never)}::jsonb)
    on conflict (key) do update set
      value = jsonb_build_object(
        'announced',
        coalesce(app_settings.value -> 'announced', '{}'::jsonb) || ${sql.json(marks as never)}::jsonb
      ),
      updated_at = now()
  `
}

/** Forget marks for approvals that are no longer pending, so a decided-and-
 *  recreated id cannot inherit a stale "already announced".
 *
 *  Same statement, same reason — but `liveKeys` comes from a census taken at a
 *  moment in the past, and an approval raised SINCE that moment is not in it.
 *  Deleting its mark because it "isn't live" is the double-notify by another
 *  route, so a mark written at or after `censusTakenAt` is kept regardless: the
 *  census could not have seen what it refers to. ISO-8601 UTC strings from
 *  `toISOString()` compare lexicographically in the order they compare
 *  chronologically, and comparing as text rather than casting means a
 *  hand-edited value cannot make this statement throw. */
async function pruneAnnounced(liveKeys: string[], censusTakenAt: string): Promise<void> {
  const sql = await db()
  await sql`
    update app_settings set value = jsonb_build_object('announced', (
      select coalesce(jsonb_object_agg(e.key, e.value), '{}'::jsonb)
      from jsonb_each(coalesce(value -> 'announced', '{}'::jsonb)) as e
      where e.key = any(${liveKeys}::text[]) or (e.value #>> '{}') >= ${censusTakenAt}
    )), updated_at = now()
    where key = ${ANNOUNCE_STATE_KEY}
  `
}

/** What a content-free message may say an approval IS. Fixed per kind and never
 *  `approval.title` — the recipient is being told the workspace has something
 *  stuck in it, not what is in it.
 *
 *  Lives here, with the kinds, because two surfaces need it: the raise-time FACT
 *  announcement below and the SLA's stall report in server/digest.ts. It was
 *  written out in digest.ts alone, so the announcement had no way to name a kind
 *  without either importing from the job that ages approvals or writing the
 *  table a second time. A fifth kind now gets its content-free name in the same
 *  file it gets its authority. */
export const KIND_LABEL: Record<ApprovalKind, string> = {
  google_action: 'an outbound email or calendar invite an agent drafted',
  workbench_plan: 'a build plan an agent is blocked on',
  ticket_review: 'a ticket waiting for sign-off',
  repo_request: 'a repository an agent asked for',
  run_decision: 'a long-running job parked on a question only a person can answer',
}

/** THE announcement. Both halves of the disclosure, in one call, because they
 *  are one event: the people who may read the words get the words, and the
 *  people who can act on it but may not read them get the FACT.
 *
 *  The fact half is not a nicety. Before it existed this function addressed
 *  `content` and nothing else, so bounding an audience could silence the whole
 *  announcement: `{ by: 'admin', onBoard }` resolves content to
 *  `admins ∩ boardEditors(onBoard)`, which is EMPTY in a workspace where no
 *  admin is a member of that board — and every admin in that workspace can still
 *  GRANT the thing. Nobody was told, and thirty days later the SLA reported that
 *  the workspace had no admin at all. `server/gaps.ts` had already worked this
 *  out for capability gaps and addressed both halves by hand; this is that shape
 *  moved to where every raiser gets it.
 *
 *  Returns how many were reached on each channel — the caller marks the approval
 *  announced when EITHER landed, and says which, because "announced to the
 *  admins as a fact" and "announced to the people who can read it" are different
 *  states of the workspace and only one of them is fine. */
async function announce(
  approval: PendingApproval,
  target: { to: string[]; fact: string[] },
): Promise<{ content: number; fact: number }> {
  const told = { content: 0, fact: 0 }
  const content = new Set(target.to)
  for (const userId of content) {
    try {
      await addNotification(userId, {
        kind: 'approval_pending',
        title: approval.title,
        body: `${approval.detail}\n\nNothing happens until someone approves or rejects it.`,
        href: approval.href,
      })
      told.content++
    } catch (e) {
      console.error(`[approvals] could not announce ${approval.key} to ${userId}:`, e)
    }
  }
  for (const userId of new Set(target.fact)) {
    // Never to somebody already getting the content — `resolveDisclosures`
    // guarantees the two lists are disjoint, and this is the belt on it, because
    // the one thing worse than a fact nobody gets is a person told the same
    // approval twice in two different amounts of detail.
    if (content.has(userId)) continue
    try {
      await addNotification(userId, {
        kind: 'approval_pending',
        title: `Waiting for a decision: ${KIND_LABEL[approval.kind]}`,
        body: factBody(approval, told.content),
        // NO href, on purpose. A fact report is not a link to the thing — the
        // recipient may not read the thing. What they can act on (adding an
        // admin to that board, granting the repo from their own admin surface)
        // is on a screen they can already find.
      })
      told.fact++
    } catch (e) {
      console.error(`[approvals] could not tell ${userId} that ${approval.key} exists:`, e)
    }
  }
  return told
}

/** The fact, in words that are true in every case it is sent. `reached` is how
 *  many people were told what it says, which is the difference between "this is
 *  in hand and you are simply not on that board" and "nobody has been told at
 *  all" — and an admin cannot tell those apart from the outside. */
function factBody(approval: PendingApproval, reached: number): string {
  const bounded = approval.authority.by === 'admin' && !!approval.authority.onBoard
  const who = bounded
    ? reached
      ? 'It was raised while working a board you are not a member of, so what the agent wrote went to the admins who are.'
      : 'It was raised while working a board no admin of this workspace is a member of, so nobody has been told what it says.'
    : reached
      ? 'The people whose decision it is have been told what it says.'
      : 'Nobody in this workspace can decide it as it stands, so nobody has been told what it says.'
  return (
    `${KIND_LABEL[approval.kind]} is waiting for a decision. ${who}\n\n` +
    'Not a word of it is repeated here. Open Talaria to act on it, or to give somebody who can the access they need.'
  )
}

// ── WHO HEARS IT THE MOMENT IT IS RAISED ─────────────────────────────────────
//
// `approvalAudience` answers "who MAY be told". That is a disclosure CEILING,
// not a mailing list, and for three of the four kinds it is also the right list.
//
// `ticket_review` is the one that breaks. Its audience is every EDITOR of the
// board, and a ticket entering review is not a rare event — it is the ordinary
// end of every piece of agent work on that board. Six editors and three tickets
// is eighteen notifications, and `approval_pending` routes to `both` by default
// (lib/notifications.ts: "the definition of blocked-on-a-human"), so it is also
// eighteen emails, for three decisions at most two people were ever going to
// make. That is the shape that teaches a workspace to filter Talaria into a
// folder — and the classes that genuinely had to reach somebody, a blocked
// agent at 02:00, go into the folder with it. The milestone dies there.
//
// So a review's FIRST-CONTACT announcement is addressed to the people already
// NAMED on the ticket, and the rest of the board still hears it three ways,
// none of which this removes:
//
//   · THE DIGEST, which is the point. Its "In review" section is built from
//     `homeQueues` — the same query Home draws — and that is every ticket in a
//     review column on every board you are a member of, assigned to you or not.
//     So a review nobody was named on is in tomorrow's digest for every editor,
//     with the count in the subject line. The immediate notification is now the
//     narrow channel and the digest is the wide one, which is the division of
//     labour the digest exists for.
//   · Home and the focus queue, the moment anybody looks.
//   · the SLA, which escalates to EVERY decider once it has aged past
//     `escalateAfterMinutes`. The review that is genuinely being ignored still
//     goes wide — and only that one.
//
// The alternative policies were weighed and lost. Per-board setting: a switch
// nobody will find, defaulted either to the noise or to the silence, which is
// the real decision made twice. In-app for the many and email for the aged: it
// is what the SLA already does, and doing it again here needs a per-call route
// override that `addNotification` does not have and should not grow — the
// routing table is the user's, not the caller's.

/** Does first contact go to the whole audience, or only to the named? Declared
 *  per kind so a fifth kind cannot be added without answering it. */
const ANNOUNCE_TO_NAMED_ONLY: Record<ApprovalKind, boolean> = {
  // For a personal action the audience IS the owner, so narrowing is a no-op;
  // for an ORG action there is no owner at all and narrowing would silence it.
  google_action: false,
  // An agent is stopped dead behind this one and heavy plans are rare. Loud is
  // correct, and an unassigned ticket must not make it quiet.
  workbench_plan: false,
  // The noisy one. See above.
  ticket_review: true,
  // Admin-only, never has an owner. Narrowing would silence it.
  repo_request: false,
  // Same argument as `workbench_plan`, and stronger: a parked run is a piece of
  // work stopped dead, holding a checkpoint, going nowhere until somebody
  // answers. Narrowing to the named would also silence every ORG-WIDE run —
  // `owner_user_id` is null for a fitness sweep or a retrieval migration, which
  // are exactly the runs whose questions nobody currently hears at all.
  run_decision: false,
}

export interface AnnounceTarget {
  /** Who to tell right now, in full. Always a SUBSET of the disclosure's
   *  `content` — this narrows, and nothing in this file may widen. */
  to: string[]
  /** Who to tell that it EXISTS. Passed straight through from the disclosure:
   *  the fact audience is the people who can unblock it, and no per-kind
   *  first-contact policy narrows that. Narrowing `to` for a noisy kind must not
   *  quietly narrow this, or the bound that made a kind quiet would be the thing
   *  that also stopped anybody hearing it was stuck. */
  fact: string[]
  /** Nobody is being told YET, and that is the policy working rather than
   *  failing: somebody can decide it, nobody is NAMED on it, so there is no one
   *  it is waiting on and nothing urgent to say. Deliberately distinct from an
   *  empty `to` because NOBODY can decide it, which is a stall the SLA reports
   *  to the admins.
   *
   *  Not called `deferred`: the escalation job in server/digest.ts already has a
   *  `deferred` counter and it means something else entirely (due, but held back
   *  by that pass's escalation budget). One word, two meanings, ten lines apart
   *  is how a rule gets re-expressed by hand at the next call site. */
  awaitingOwner: boolean
}

/** Narrow an approval's disclosure to the people it is announced to on sight. */
export function announceTarget(approval: PendingApproval, who: Disclosure): AnnounceTarget {
  const fact = who.fact
  if (!ANNOUNCE_TO_NAMED_ONLY[approval.kind]) return { to: who.content, fact, awaitingOwner: false }
  // Intersected for the same reason the nag stage intersects: an assignee who
  // has since lost edit access on the board cannot open what we would send.
  const named = approval.ownerUserIds.filter((id) => who.content.includes(id))
  if (named.length) return { to: named, fact, awaitingOwner: false }
  return { to: [], fact, awaitingOwner: who.content.length > 0 }
}

/** Announce ONE just-raised approval, now. Safe to call from the request that
 *  created it — it is idempotent (a second call is a no-op) and it should be
 *  fired without awaiting, because the caller is servicing somebody else's
 *  request. Unknown keys are ignored: an approval that was decided between the
 *  insert and this call has nothing to announce.
 *
 *  An approval still AWAITING AN OWNER is left unmarked, on purpose: a review
 *  raised with no human on it that gets one two minutes later is still
 *  announced to that person by the next sweep, and if it is still nameless when
 *  it passes the nag threshold the sweep retires it to the ageing stages. */
export async function announceApproval(key: string): Promise<number> {
  const { approvals } = await pendingApprovals()
  const approval = approvals.find((a) => a.key === key)
  if (!approval) return 0
  const state = await announceState()
  if (state.announced?.[key]) return 0
  const audience = await approvalAudience([approval])
  const target = announceTarget(approval, audience.get(key) ?? { content: [], fact: [] })
  if (target.awaitingOwner) return 0
  const told = await announce(approval, target)
  // Marked when EITHER channel landed. A fact that reached the admins is an
  // announcement — leaving it unmarked would repeat it every five minutes for
  // the life of the approval.
  if (told.content || told.fact) await markAnnounced({ [key]: new Date().toISOString() })
  return told.content + told.fact
}

export interface AnnounceSweepResult {
  announced: number
  /** Announced as a FACT only: nobody may be told what it says, and the people
   *  who can unblock that were told it exists. Marked (it WAS announced), and
   *  counted separately because it is a workspace with a hole in it — an admin
   *  missing from a board — and a run that reports it as an ordinary
   *  announcement is the silence this milestone keeps rediscovering. */
  factOnly: number
  /** Raised, still unannounced, and NOBODY can decide it. Left unmarked; the
   *  SLA's stall report is what raises those. */
  unreachable: number
  /** Raised, decidable, but nobody is named on it yet and its kind announces to
   *  the named only. Not a failure and not a silence — see `AnnounceTarget`. */
  awaitingOwner: number
}

/** Announce everything raised since the last pass.
 *
 *  `maxAgeMinutes` is the nag threshold: past it, the nag stage is already
 *  going to speak and a first-contact announcement would be a second message
 *  about the same thing. It is also what stops the first run after a deploy
 *  from announcing a backlog that has been sitting there for a week. */
export async function sweepUnannounced(
  census: ApprovalCensus,
  now: Date,
  maxAgeMinutes: number,
): Promise<AnnounceSweepResult> {
  const state = await announceState()
  const already = state.announced ?? {}
  // Only what THIS pass decided. Merged, never written over the top of the map
  // it was read from — see `markAnnounced` for the mark a whole-blob write eats.
  const marks: Record<string, string> = {}
  const result: AnnounceSweepResult = { announced: 0, factOnly: 0, unreachable: 0, awaitingOwner: 0 }
  for (const approval of census.approvals) {
    if (already[approval.key]) continue
    const ageMinutes = (now.getTime() - new Date(approval.waitingSince).getTime()) / 60_000
    if (ageMinutes >= maxAgeMinutes) {
      // Older than the nag threshold: not new, and the nag stage owns it. Marked
      // so it is not reconsidered every tick for the rest of its life.
      marks[approval.key] = now.toISOString()
      continue
    }
    const target = announceTarget(approval, census.audience.get(approval.key) ?? { content: [], fact: [] })
    if (target.awaitingOwner) {
      // Nobody named yet. NOT marked — whoever gets assigned before the nag
      // threshold is still announced to, and if nobody is, the branch above
      // retires it. Not counted as unreachable: somebody can decide this.
      result.awaitingOwner++
      continue
    }
    const told = await announce(approval, target)
    if (told.content) {
      marks[approval.key] = now.toISOString()
      result.announced++
    } else if (told.fact) {
      // Nobody may be told what it says, and the people who can fix that have
      // been told it exists. That IS the announcement for this approval — mark
      // it, or it repeats every tick — but it is not the same as somebody who
      // can decide it having heard.
      marks[approval.key] = now.toISOString()
      result.factOnly++
    } else {
      // Deliberately NOT marked: nobody heard, so nothing was announced.
      result.unreachable++
    }
  }
  await markAnnounced(marks)
  // Only prune against a COMPLETE census — a kind that failed to read this pass
  // looks like a kind with nothing pending, and dropping its marks would
  // re-announce every one of its approvals the moment the table came back.
  if (!census.failedKinds.length) {
    await pruneAnnounced(census.approvals.map((a) => a.key), now.toISOString())
  }
  return result
}
