// Template library client: org-wide ticket/plan formats + board bindings.
import { useQuery } from '@tanstack/react-query'
import { getList } from '@/lib/fetch-json'

export type TemplateKind = 'ticket' | 'plan'

export interface Template {
  id: string
  name: string
  kind: TemplateKind
  body: string
  guidance: string
  createdBy: string | null
  updatedAt: string
}

export interface BoardTemplateBinding {
  templateId: string
  isDefault: boolean
}

export function useTemplates() {
  return useQuery({
    queryKey: ['templates'],
    queryFn: (): Promise<Template[]> => getList<Template>('/api/templates', 'templates'),
  })
}

export function useBoardTemplates(boardId: string | null) {
  return useQuery({
    queryKey: ['board-templates', boardId],
    enabled: !!boardId,
    queryFn: (): Promise<BoardTemplateBinding[]> =>
      getList<BoardTemplateBinding>(`/api/boards/${boardId}/templates`, 'bindings'),
  })
}

const post = (url: string, method: string, body: unknown) =>
  fetch(url, {
    method,
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  }).then(async (r) => {
    const j = (await r.json().catch(() => ({}))) as { error?: string }
    if (!r.ok) throw new Error(j.error ?? `request failed (${r.status})`)
    return j
  })

export const createTemplate = (t: { name: string; kind: TemplateKind; body?: string; guidance?: string }) =>
  post('/api/templates', 'POST', t) as Promise<{ template: Template }>

export const updateTemplate = (id: string, patch: { name?: string; body?: string; guidance?: string }) =>
  post(`/api/templates/${id}`, 'PUT', patch) as Promise<{ template: Template }>

export const deleteTemplate = (id: string) =>
  fetch(`/api/templates/${id}`, { method: 'DELETE', credentials: 'same-origin' })

export const setBoardTemplates = (boardId: string, templateIds: string[], defaultId: string | null) =>
  post(`/api/boards/${boardId}/templates`, 'PUT', { templateIds, defaultId })
