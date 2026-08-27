// Template library client: org-wide ticket/plan formats + board bindings.
import { createQuery } from '@tanstack/svelte-query'
import { delJson, errorMessage, getList, postJson, putJson } from '@/lib/fetch-json'
import { pushToast } from '@/lib/toast.svelte'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

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
  return createQuery(() => ({
    queryKey: ['templates'],
    queryFn: (): Promise<Template[]> => getList<Template>('/api/templates', 'templates'),
  }))
}

export function useBoardTemplates(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const i = resolve(boardId)
    return {
      queryKey: ['board-templates', i],
      enabled: !!i,
      queryFn: (): Promise<BoardTemplateBinding[]> =>
        getList<BoardTemplateBinding>(`/api/boards/${i}/templates`, 'bindings'),
    }
  })
}

export const createTemplate = (t: { name: string; kind: TemplateKind; body?: string; guidance?: string }) =>
  postJson<{ template: Template }>('/api/templates', t)

export const updateTemplate = (id: string, patch: { name?: string; body?: string; guidance?: string }) =>
  putJson<{ template: Template }>(`/api/templates/${id}`, patch)

export const deleteTemplate = (id: string) =>
  // The call site fires and forgets (`void remove(t)`, no catch), so a refused
  // delete is surfaced here rather than left as an unhandled rejection.
  delJson<{ ok: true }>(`/api/templates/${id}`).catch((e: unknown) =>
    pushToast({ title: 'Delete failed', body: errorMessage(e), tone: 'danger' }),
  )

export const setBoardTemplates = (boardId: string, templateIds: string[], defaultId: string | null) =>
  putJson<{ bindings: BoardTemplateBinding[] }>(`/api/boards/${boardId}/templates`, { templateIds, defaultId })
