// Research client: runs list + detail, mode catalog, start/delete.
import { createQuery } from '@tanstack/svelte-query'
import { getJson, getList } from '@/lib/fetch-json'

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
  const r = await fetch('/api/research', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ question, mode, agentModel }),
  })
  const j = (await r.json().catch(() => ({}))) as { run?: ResearchRun; error?: string; duplicateOf?: string }
  if (r.status === 409 && j.duplicateOf) throw new Error('that question is already being researched')
  if (!r.ok || !j.run) throw new Error(j.error ?? `failed (${r.status})`)
  return j.run
}

export async function deleteResearch(id: string): Promise<void> {
  await fetch(`/api/research/${id}`, { method: 'DELETE', credentials: 'same-origin' })
}
