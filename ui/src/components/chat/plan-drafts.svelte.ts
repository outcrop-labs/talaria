// THE DRAFT JOB LIVES ON THE SERVER. Turning a plan into tickets is a durable
// 'plan-draft' run (server/runs/defs/plan-draft.ts): the agent reads the
// conversation server-side, the proposals land in a `plan_drafts` row, and
// this store is the CLIENT'S VIEW of that row — start it, poll it while it
// drafts, persist the review walk's edits back, drop it when consumed. Close
// the modal, leave the surface, reload the tab, restart the server: the draft
// is paired to the conversation, and the header button finds it again.
//
// The store keeps a three-state surface ('drafting' | 'ready' | 'failed') so
// PlanModal and the header button read one vocabulary; the server's run
// states fold into it here.
import type { Proposal } from './plan-modal'
import { delJson, getJson, HttpError, patchJson, postJson } from '@/lib/fetch-json'

export interface PlanDraftJob {
  status: 'drafting' | 'ready' | 'failed'
  proposals: Proposal[]
  note: string | null
  /** The board picked when the draft started — creation needs it, and the
   *  modal that reopens to review was not the modal that picked it. */
  boardId: string
}

interface Entry extends PlanDraftJob {
  /** The server draft row's id — the stale-response guard. A poll reply for
   *  any other id belongs to a draft this client has moved past. */
  id: string
}

/** What GET/POST return under `draft` (server/plan-drafts.ts projection). */
interface ServerDraft {
  id: string
  status: 'queued' | 'running' | 'done' | 'error' | 'cancelled'
  proposals?: Proposal[]
  note: string | null
  error: string | null
  boardId: string | null
}

const jobs = $state<Record<string, Entry>>({})
// Per-conversation route (plans and channels differ) and timers — plumbing,
// never read by a component.
const urls = new Map<string, string>()
const polls = new Map<string, ReturnType<typeof setTimeout>>()
const saves = new Map<string, ReturnType<typeof setTimeout>>()

const normalizeProposal = (p: Proposal): Proposal => ({
  ...p,
  dependsOn: p.dependsOn ?? [],
  tags: p.tags ?? [],
  include: p.include ?? true,
})

/** Server run state → the store's three states. 'cancelled' folds to absent:
 *  a draft discarded on another device is not this device's failure to show. */
const toEntry = (d: ServerDraft): Entry | null => {
  const boardId = d.boardId ?? ''
  if (d.status === 'queued' || d.status === 'running') {
    return { id: d.id, status: 'drafting', proposals: [], note: null, boardId }
  }
  if (d.status === 'error') {
    return { id: d.id, status: 'failed', proposals: [], note: d.error ?? d.note ?? 'planning failed', boardId }
  }
  if (d.status === 'cancelled') return null
  if (!d.proposals?.length) {
    return { id: d.id, status: 'failed', proposals: [], note: d.note ?? 'no tickets came back', boardId }
  }
  return { id: d.id, status: 'ready', proposals: d.proposals.map(normalizeProposal), note: d.note ?? null, boardId }
}

const stopPoll = (planId: string) => {
  const t = polls.get(planId)
  if (t) clearTimeout(t)
  polls.delete(planId)
}

const apply = (planId: string, entry: Entry | null) => {
  if (entry === null) {
    delete jobs[planId]
    stopPoll(planId)
    return
  }
  jobs[planId] = entry
  if (entry.status === 'drafting') schedulePoll(planId)
  else stopPoll(planId)
}

/** The poll cadence: fast enough that "Review drafts" flips within a beat of
 *  the run landing, slow enough that a parked plan surface is not a hammer on
 *  the API. Same order as the research list's live interval. */
const POLL_MS = 2500

const schedulePoll = (planId: string) => {
  if (polls.has(planId)) return
  polls.set(
    planId,
    setTimeout(() => {
      polls.delete(planId)
      void refresh(planId)
    }, POLL_MS),
  )
}

const refresh = async (planId: string) => {
  const url = urls.get(planId)
  if (!url) return
  try {
    const j = await getJson<{ draft: ServerDraft | null }>(url)
    const current = jobs[planId]
    // The stale guard: a reply for another draft row (this client dropped the
    // old one and started fresh) is not news.
    if (j.draft && (!current || current.id === j.draft.id)) apply(planId, toEntry(j.draft))
    else if (!j.draft && current && current.status === 'drafting') apply(planId, null)
  } catch (e) {
    // HttpError is the old `!r.ok` return: the plan (or its draft row) is
    // gone, and polling stops. Anything else is a dropped poll — a late poll;
    // if the draft is still drafting, the next tick is already scheduled.
    if (!(e instanceof HttpError) && jobs[planId]?.status === 'drafting') schedulePoll(planId)
  }
}

export function planDraft(planId: string): PlanDraftJob | undefined {
  return jobs[planId]
}

/** Start (or ride — the server single-flights) the conversation's draft. */
export function startPlanDraft(
  planId: string,
  req: { draftUrl: string; agentModel: string | undefined; tier: string | null; boardId: string | null; templateId: string | null },
): void {
  if (jobs[planId]?.status === 'drafting') return
  urls.set(planId, req.draftUrl)
  void (async () => {
    try {
      const j = await postJson<{ draft?: ServerDraft }>(req.draftUrl, {
        agentModel: req.agentModel,
        tier: req.tier,
        boardId: req.boardId,
        templateId: req.templateId,
      })
      if (!j.draft) {
        jobs[planId] = { id: '', status: 'failed', proposals: [], note: 'planning failed', boardId: req.boardId ?? '' }
        return
      }
      apply(planId, toEntry(j.draft))
    } catch (e) {
      // The server's failure sentence (a 500 {error} body) arrives as the
      // rejection's message; only a network failure gets the reach sentence.
      const note = e instanceof HttpError ? e.message : 'planning failed. Is the gateway up?'
      jobs[planId] = { id: '', status: 'failed', proposals: [], note, boardId: req.boardId ?? '' }
    }
  })()
}

/** Ask the server what is paired to this conversation. Called when a surface
 *  opens a conversation — a reload must find an in-flight or finished draft
 *  the client has never seen. */
export async function hydratePlanDraft(planId: string, draftUrl: string): Promise<void> {
  urls.set(planId, draftUrl)
  if (jobs[planId]) {
    if (jobs[planId].status === 'drafting') schedulePoll(planId)
    return
  }
  await refresh(planId)
}

/** An edit from the review walk (title, drop, dependency…) — applied locally
 *  at once, and written through to the row (debounced: the walk edits in
 *  bursts) so the edits survive a reload of the review. */
export function patchPlanDraft(planId: string, i: number, p: Partial<Proposal>): void {
  const j = jobs[planId]
  if (j?.status !== 'ready') return
  j.proposals = j.proposals.map((x, k) => (k === i ? { ...x, ...p } : x))
  const t = saves.get(planId)
  if (t) clearTimeout(t)
  saves.set(
    planId,
    setTimeout(() => {
      saves.delete(planId)
      const url = urls.get(planId)
      const now = jobs[planId]
      if (!url || now?.status !== 'ready') return
      // Swallowed on purpose: the walk edits in bursts and the row is the
      // source of truth — a lost write resurfaces on the next reload.
      void patchJson(url, { proposals: now.proposals }).catch(() => {})
    }, 600),
  )
}

/** Drop the conversation's draft — Back to config, or the batch was created
 *  and consumed. Cancels the run server-side if it is still drafting. */
export function discardPlanDraft(planId: string): void {
  stopPoll(planId)
  const t = saves.get(planId)
  if (t) clearTimeout(t)
  saves.delete(planId)
  delete jobs[planId]
  const url = urls.get(planId)
  // Best-effort cancel: the local job is gone either way.
  if (url) void delJson(url).catch(() => {})
}
