import { useQuery } from '@tanstack/react-query'

export type Visibility = 'private' | 'org' | 'public'
export type EditPolicy = 'owner' | 'org' | 'restricted'
export type GrantRole = 'viewer' | 'editor'
export interface KbEditor {
  principalType: 'user' | 'agent'
  principalId: string
  role: GrantRole
}

export interface KbSpace {
  id: string
  name: string
  description: string | null
  icon: string | null
  body: string
  visibility: Visibility
  publicSlug: string | null
  editPolicy: EditPolicy
  ownerUserId: string | null
  createdBy: string | null
}

export const useSpace = (id: string | null) =>
  useQuery({
    queryKey: ['kb-space', id],
    enabled: !!id,
    queryFn: async (): Promise<KbSpace | null> => {
      const r = await fetch(`/api/kb/spaces/${id}`)
      if (!r.ok) return null
      return ((await r.json()) as { space: KbSpace }).space
    },
  })
export interface KbDocMeta {
  id: string
  spaceId: string
  parentId: string | null
  title: string
  icon: string | null
  kind: 'human' | 'agent'
  official: boolean
  visibility: Visibility
  publicSlug: string | null
  editPolicy: EditPolicy
  permsInherited: boolean
  ownerUserId: string | null
  sort: number
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

export const updateSpace = (
  id: string,
  patch: Partial<Pick<KbSpace, 'name' | 'description' | 'icon' | 'body' | 'visibility' | 'editPolicy'>> & { editors?: KbEditor[] },
) => fetch(`/api/kb/spaces/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

/** Fetch the current editor grants for a doc / folder / artifact (from its GET
 *  route, which returns `editors`). */
export const fetchEditors = async (kind: 'docs' | 'spaces' | 'artifacts', id: string): Promise<KbEditor[]> => {
  const r = await fetch(kind === 'artifacts' ? `/api/artifacts/${id}` : `/api/kb/${kind}/${id}`)
  if (!r.ok) return []
  return ((await r.json()) as { editors?: KbEditor[] }).editors ?? []
}

export const deleteSpace = (id: string) => fetch(`/api/kb/spaces/${id}`, { method: 'DELETE' })

export const createDoc = (spaceId: string, input: { title?: string; kind?: 'human' | 'agent'; parentId?: string | null }) =>
  fetch(`/api/kb/spaces/${spaceId}/docs`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(input) }).then((r) => r.json())

export const saveDoc = (
  id: string,
  patch: Partial<Pick<KbDoc, 'title' | 'body' | 'icon' | 'visibility' | 'editPolicy' | 'permsInherited' | 'official'>> & { editors?: KbEditor[] },
) => fetch(`/api/kb/docs/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

export const deleteDoc = (id: string) => fetch(`/api/kb/docs/${id}`, { method: 'DELETE' })

export const moveDoc = (id: string, parentId: string | null, sort: number) =>
  fetch(`/api/kb/docs/${id}/move`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ parentId, sort }) }).then((r) => r.json())

export interface KbSearchHit {
  id: string
  spaceId: string
  spaceName: string
  title: string
  icon: string | null
  snippet: string
  visibility: 'private' | 'org' | 'public'
}
export const searchKb = (q: string) =>
  fetch(`/api/kb/search?q=${encodeURIComponent(q)}`).then((r) => (r.ok ? r.json() : { hits: [] })).then((d) => (d as { hits: KbSearchHit[] }).hits)

export interface KbBacklink {
  id: string
  title: string
  icon: string | null
  spaceId: string
}
export const useBacklinks = (docId: string | null) =>
  useQuery({
    queryKey: ['kb-backlinks', docId],
    enabled: !!docId,
    queryFn: async (): Promise<KbBacklink[]> => {
      const r = await fetch(`/api/kb/docs/${docId}/backlinks`)
      if (!r.ok) return []
      return ((await r.json()) as { backlinks: KbBacklink[] }).backlinks
    },
  })
