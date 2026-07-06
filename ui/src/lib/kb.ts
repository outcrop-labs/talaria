import { useQuery } from '@tanstack/react-query'

export interface KbSpace {
  id: string
  name: string
  description: string | null
  icon: string | null
}
export interface KbDocMeta {
  id: string
  spaceId: string
  parentId: string | null
  title: string
  kind: 'human' | 'agent'
  official: boolean
  visibility: 'private' | 'org' | 'public'
  publicSlug: string | null
  createdBy: string | null
  updatedBy: string | null
  updatedAt: string
}
export interface KbDoc extends KbDocMeta {
  body: string
}

export const useSpaces = () =>
  useQuery({
    queryKey: ['kb-spaces'],
    queryFn: async (): Promise<KbSpace[]> => {
      const r = await fetch('/api/kb/spaces')
      if (!r.ok) return []
      return ((await r.json()) as { spaces: KbSpace[] }).spaces
    },
  })

export const useDocs = (spaceId: string | null) =>
  useQuery({
    queryKey: ['kb-docs', spaceId],
    enabled: !!spaceId,
    queryFn: async (): Promise<KbDocMeta[]> => {
      const r = await fetch(`/api/kb/spaces/${spaceId}/docs`)
      if (!r.ok) return []
      return ((await r.json()) as { docs: KbDocMeta[] }).docs
    },
  })

export const useDoc = (id: string | null) =>
  useQuery({
    queryKey: ['kb-doc', id],
    enabled: !!id,
    queryFn: async (): Promise<KbDoc | null> => {
      const r = await fetch(`/api/kb/docs/${id}`)
      if (!r.ok) return null
      return ((await r.json()) as { doc: KbDoc }).doc
    },
  })

export const createSpace = (name: string) =>
  fetch('/api/kb/spaces', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name }) }).then((r) => r.json())

export const createDoc = (spaceId: string, input: { title?: string; kind?: 'human' | 'agent'; parentId?: string | null }) =>
  fetch(`/api/kb/spaces/${spaceId}/docs`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(input) }).then((r) => r.json())

export const saveDoc = (id: string, patch: Partial<Pick<KbDoc, 'title' | 'body' | 'visibility' | 'official'>>) =>
  fetch(`/api/kb/docs/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

export const deleteDoc = (id: string) => fetch(`/api/kb/docs/${id}`, { method: 'DELETE' })
