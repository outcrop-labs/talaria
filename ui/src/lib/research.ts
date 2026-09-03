// Research client: runs list + detail, mode catalog, start/delete.
import { createQuery } from '@tanstack/svelte-query'
import { delJson, errorMessage, getJson, getList, postJson, postJsonOr } from '@/lib/fetch-json'
import { pushToast } from '@/lib/toast.svelte'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export type ResearchMode = 'recon' | 'brief' | 'expedition'
/** 'awaiting' is its own value, not a shade of 'running': a run parked on a
 *  question reads as waiting-on-you, never as working (the ticket-#5 lesson).
 *  It still polls like an in-flight run — the answer can land any second. */
export type ResearchStatus = 'queued' | 'running' | 'awaiting' | 'done' | 'error'

export interface ResearchRun {
  id: string
  ownerUserId: string | null
  requestedBy: string
  agentModel: string
  mode: ResearchMode
  question: string
  title: string | null
  status: ResearchStatus
  phase: string | null
  /** THE QUESTION A PARKED RUN IS WAITING ON, when it is waiting on one — the
   *  panel at the top of the run's page. Null the rest of the time. */
  awaiting: RunQuestion | null
  artifactId: string | null
  /** The run's discussion, created with the run — the clarify question, the
   *  scope ack, and the report-ready turn all land in it. Null for a run an
   *  ownerless org-agent started (still readable, not talkable-in) and for old
   *  rows predating eager conversations. */
  conversationId: string | null
  error: string | null
  stats: { queries?: number; sources?: number; cited?: number }
  createdAt: string
  completedAt: string | null
}

export interface ResearchSource {
  idx: number
  url: string
  title: string | null
  snippet: string | null
}

/** WHAT A PARKED RUN ASKS — the step's own question with the options it
 *  offered. Answers go back by option id; an id the step never declared is
 *  refused by the server, so the run can only ever be told something it wrote
 *  a branch for. A `freeText` question is answered in the run's discussion —
 *  the options are a placeholder, not buttons. */
export interface RunQuestion {
  key: string
  question: string
  detail?: string
  options: Array<{ id: string; label: string; detail?: string }>
  freeText?: boolean
}

export const MODE_META: Record<ResearchMode, { label: string; tagline: string; eta: string }> = {
  recon: { label: 'Recon', tagline: 'One fast pass: a cited answer', eta: '~1 min' },
  brief: { label: 'Brief', tagline: 'Planned angles: a briefing document', eta: 'a few min' },
  expedition: { label: 'Expedition', tagline: 'Iterative deep dive: a full report', eta: '10 min +' },
}

export function useResearchRuns() {
  return createQuery(() => ({
    queryKey: ['research-runs'],
    queryFn: (): Promise<ResearchRun[]> => getList<ResearchRun>('/api/research', 'runs'),
    // Live while anything is in flight — including parked: an answer in a
    // discussion can resume a run between any two polls.
    refetchInterval: (q: { state: { data?: ResearchRun[] } }) =>
      q.state.data?.some((r) => r.status === 'queued' || r.status === 'running' || r.status === 'awaiting') ? 3_000 : false,
  }))
}

export function useResearchRun(id: MaybeGetter<string | null>) {
  return createQuery(() => {
    const i = resolve(id)
    return {
      queryKey: ['research-run', i],
      enabled: !!i,
      queryFn: (): Promise<{ run: ResearchRun; sources: ResearchSource[] }> =>
        getJson<{ run: ResearchRun; sources: ResearchSource[] }>(`/api/research/${i}`),
      refetchInterval: (q: { state: { data?: { run: ResearchRun } } }) => {
        const s = q.state.data?.run.status
        return s === 'queued' || s === 'running' || s === 'awaiting' ? 2_500 : false
      },
    }
  })
}

export async function startResearch(question: string, mode: ResearchMode, agentModel: string): Promise<ResearchRun> {
  // 409 is an answer, not a failure: its body names the run already asking
  // this question. Every other non-2xx throws with the server's sentence.
  const j = await postJsonOr<{ run?: ResearchRun | null; duplicateOf?: string; error?: string }>(
    '/api/research',
    { question, mode, agentModel },
    [409],
  )
  if (j.duplicateOf) throw new Error('that question is already being researched')
  if (!j.run) throw new Error(j.error ?? 'could not start the run')
  return j.run
}

export async function deleteResearch(id: string): Promise<void> {
  try {
    await delJson<{ ok: true }>(`/api/research/${id}`)
  } catch (e) {
    // The rail's remove flow has no error state of its own, and its caller
    // does not catch — the toast is the only place a refused delete gets said.
    pushToast({ title: 'Delete failed', body: errorMessage(e), tone: 'danger' })
  }
}

/** ANSWER THE QUESTION A RUN IS PARKED ON. The run resumes on the server the
 *  moment the answer lands — the caller invalidates and the refetch shows it
 *  moving again. Throws with the server's sentence for the cases that matter
 *  to a person: 403 you cannot decide this run, 409 somebody already answered
 *  (or a stale tab), 400 an option the run never offered. */
export async function answerResearch(runId: string, optionId: string): Promise<void> {
  await postJson(`/api/research/${runId}/decide`, { optionId })
}

/** OPEN THE THREAD FOR A RUN, creating it on the first call.
 *
 *  Returns the conversation id, or an error sentence for the one case that is
 *  not a failure: a run an AGENT started for the org has no human owner and so
 *  has no conversation to own. The report is still readable. */
export async function openResearchConversation(runId: string): Promise<{ conversationId?: string; error?: string }> {
  try {
    return await postJson<{ conversationId?: string }>(`/api/research/${runId}/conversation`)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

/** A person on a run: the owner, or somebody it was shared with. */
export interface ResearchMember {
  userId: string
  name: string | null
  email: string | null
  role: 'owner' | 'collaborator'
}

/** THE ROOM. One definition because two surfaces read it now — the share
 *  control in the header, and the @mention list in the discussion — and a
 *  second spelling would eventually offer a mention to somebody who cannot open
 *  the report being discussed. */
export const useResearchMembers = (runId: () => string) =>
  createQuery(() => ({
    queryKey: ['research-members', runId()],
    queryFn: (): Promise<{ members: ResearchMember[] }> => getJson<{ members: ResearchMember[] }>(`/api/research/${runId()}/members`),
  }))
