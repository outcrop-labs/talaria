// Research client: runs list + detail, mode catalog, start/delete.
import { createQuery } from '@tanstack/svelte-query'
import { delJson, errorMessage, getJson, getList, postJson, postJsonOr } from '@/lib/fetch-json'
import { pushToast } from '@/lib/toast.svelte'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export type ResearchMode = 'recon' | 'brief' | 'expedition'
export type ResearchStatus = 'queued' | 'running' | 'done' | 'error'

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
  artifactId: string | null
  /** Null until the first message — the conversation is created on demand. */
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

export const MODE_META: Record<ResearchMode, { label: string; tagline: string; eta: string }> = {
  recon: { label: 'Recon', tagline: 'One fast pass: a cited answer', eta: '~1 min' },
  brief: { label: 'Brief', tagline: 'Planned angles: a briefing document', eta: 'a few min' },
  expedition: { label: 'Expedition', tagline: 'Iterative deep dive: a full report', eta: '10 min +' },
}

export function useResearchRuns() {
  return createQuery(() => ({
    queryKey: ['research-runs'],
    queryFn: (): Promise<ResearchRun[]> => getList<ResearchRun>('/api/research', 'runs'),
    // Live while anything is in flight.
    refetchInterval: (q: { state: { data?: ResearchRun[] } }) =>
      q.state.data?.some((r) => r.status === 'queued' || r.status === 'running') ? 3_000 : false,
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
        return s === 'queued' || s === 'running' ? 2_500 : false
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
