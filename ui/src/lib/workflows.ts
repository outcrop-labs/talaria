// Task workflow client: match rules → instructions/toolkits that ride with
// dispatched agent work. Managed on /workflows.
import { createQuery } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'

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

export interface SkillLibraryOwner {
  owner: string // 'shared' or an agent slug
  label: string
  /** The agent's model id (absent for the shared root). */
  model?: string
  /** Whether THIS user may edit this owner's skills (server-computed). */
  canEdit: boolean
  skills: Array<{ name: string; description: string; platform?: boolean }>
}

/** The fleet skill library (shared + per-agent) — what workflows bind to
 *  and what the Studio manages. Any member reads. */
export function useSkillLibrary() {
  return createQuery(() => ({
    queryKey: ['skill-library'],
    queryFn: (): Promise<SkillLibraryOwner[]> => getList<SkillLibraryOwner>('/api/skills', 'owners'),
  }))
}

export function useWorkflows() {
  return createQuery(() => ({
    queryKey: ['workflows'],
    queryFn: (): Promise<TaskWorkflow[]> => getList<TaskWorkflow>('/api/workflows', 'workflows'),
  }))
}

const send = (url: string, method: string, body?: unknown) =>
  fetch(url, {
    method,
    credentials: 'same-origin',
    ...(body !== undefined ? { headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) } : {}),
  }).then(async (r) => {
    const j = (await r.json().catch(() => ({}))) as { error?: string }
    if (!r.ok) throw new Error(j.error ?? `request failed (${r.status})`)
    return j
  })

export const createWorkflow = (h: { name: string; description?: string }) =>
  send('/api/workflows', 'POST', h) as Promise<{ workflow: TaskWorkflow }>

export const updateWorkflow = (
  id: string,
  patch: Partial<Pick<TaskWorkflow, 'name' | 'description' | 'enabled' | 'match' | 'skills' | 'toolkits'>>,
) => send(`/api/workflows/${id}`, 'PUT', patch)

export const deleteWorkflow = (id: string) => send(`/api/workflows/${id}`, 'DELETE')

// ── Skill structural ops (Studio row controls) ──────────────────────────────

export const renameSkill = (owner: string, name: string, toName: string) =>
  send(`/api/skills/${owner}/${name}`, 'POST', { op: 'rename', toName })

export const copySkillTo = (owner: string, name: string, toOwner: string) =>
  send(`/api/skills/${owner}/${name}`, 'POST', { op: 'copy', toOwner })

export const moveSkillTo = (owner: string, name: string, toOwner: string) =>
  send(`/api/skills/${owner}/${name}`, 'POST', { op: 'move', toOwner })

export const deleteSkillReq = (owner: string, name: string) => send(`/api/skills/${owner}/${name}`, 'DELETE')

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
  send(`/api/gaps/${id}`, 'PUT', { status })
