// Task workflow client: match rules → instructions/toolkits that ride with
// dispatched agent work. Managed on /workflows.
import { useQuery } from '@tanstack/react-query'

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
  instructions: string
  toolkits: Array<{ server: string; tools?: string[] }>
  position: number
}

export function useWorkflows() {
  return useQuery({
    queryKey: ['workflows'],
    queryFn: async (): Promise<TaskWorkflow[]> => {
      const r = await fetch('/api/workflows', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { workflows: TaskWorkflow[] }).workflows
    },
  })
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
  patch: Partial<Pick<TaskWorkflow, 'name' | 'description' | 'enabled' | 'match' | 'instructions' | 'toolkits'>>,
) => send(`/api/workflows/${id}`, 'PUT', patch)

export const deleteWorkflow = (id: string) => send(`/api/workflows/${id}`, 'DELETE')
