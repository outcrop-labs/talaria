// Task workflow client: match rules → instructions/toolkits that ride with
// dispatched agent work. Managed on /workflows.
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getList, postJson, putJson } from '@/lib/fetch-json'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export interface WorkflowMatch {
  labels?: string[]
  boards?: string[]
  keywords?: string[]
}

export interface TaskWorkflow {
  id: string
  name: string
  description: string
  enabled: boolean
  match: WorkflowMatch
  skills: string[]
  toolkits: Array<{ server: string; tools?: string[] }>
  position: number
}

export function useWorkflows() {
  return createQuery(() => ({
    queryKey: ['workflows'],
    queryFn: (): Promise<TaskWorkflow[]> => getList<TaskWorkflow>('/api/workflows', 'workflows'),
  }))
}

export const createWorkflow = (h: { name: string; description?: string }) =>
  postJson<{ workflow: TaskWorkflow }>('/api/workflows', h)

export const updateWorkflow = (
  id: string,
  patch: Partial<Pick<TaskWorkflow, 'name' | 'description' | 'enabled' | 'match' | 'skills' | 'toolkits'>>,
) => putJson<{ ok: true }>(`/api/workflows/${id}`, patch)

export const deleteWorkflow = (id: string) => delJson<{ ok: true }>(`/api/workflows/${id}`)

// ── Skill structural ops (Studio row controls) ──────────────────────────────

export const copySkillTo = (owner: string, name: string, toOwner: string) =>
  postJson<{ ok: true }>(`/api/skills/${owner}/${name}`, { op: 'copy', toOwner })

export const moveSkillTo = (owner: string, name: string, toOwner: string) =>
  postJson<{ ok: true }>(`/api/skills/${owner}/${name}`, { op: 'move', toOwner })

// ── Capability gaps (the Studio's Suggested queue) ──────────────────────────

export interface CapabilityGap {
  id: string
  kind: string
  boardId: string | null
  agentModel: string
  missing: string
  needs: string
  exampleTaskId: string | null
  seenCount: number
  status: 'open' | 'dismissed' | 'resolved'
  createdAt: string
  lastSeen: string
}

export function useGaps(status: MaybeGetter<string> = 'open') {
  return createQuery(() => {
    const s = resolve(status)
    return {
      queryKey: ['gaps', s],
      queryFn: (): Promise<CapabilityGap[]> => getList<CapabilityGap>(`/api/gaps?status=${s}`, 'gaps'),
    }
  })
}

export const setGapStatus = (id: string, status: CapabilityGap['status']) =>
  putJson<{ ok: true }>(`/api/gaps/${id}`, { status })
