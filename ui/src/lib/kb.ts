import { useQuery } from '@tanstack/react-query'
import { getJsonOr404, getList } from '@/lib/fetch-json'

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
    // 404 = the space is gone or was never yours: a real "not found".
    queryFn: async (): Promise<KbSpace | null> =>
      (await getJsonOr404<{ space: KbSpace }>(`/api/kb/spaces/${id}`))?.space ?? null,
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
  /** RAG routing: 'auto' (space binding / org rules) | 'none' | a brain id. */
  ragRouting: string
  createdBy: string | null
  updatedBy: string | null
  updatedAt: string
}
export interface KbDoc extends KbDocMeta {
  body: string
  /** Hidden agent-facing OKF summary (Librarian-maintained, promoted docs). */
  okf?: string | null
  /** The viewer may change sharing: owner — or, for agent-created docs,
   *  anyone with access to that agent (server-computed). */
  governs?: boolean
}

export const useSpaces = () =>
  useQuery({
    queryKey: ['kb-spaces'],
    queryFn: (): Promise<KbSpace[]> => getList<KbSpace>('/api/kb/spaces', 'spaces'),
  })

export const useDocs = (spaceId: string | null) =>
  useQuery({
    queryKey: ['kb-docs', spaceId],
    enabled: !!spaceId,
    queryFn: (): Promise<KbDocMeta[]> => getList<KbDocMeta>(`/api/kb/spaces/${spaceId}/docs`, 'docs'),
  })

export const useDoc = (id: string | null) =>
  useQuery({
    queryKey: ['kb-doc', id],
    enabled: !!id,
    // 404 = the doc is gone or was never yours: a real "not found".
    queryFn: async (): Promise<KbDoc | null> =>
      (await getJsonOr404<{ doc: KbDoc }>(`/api/kb/docs/${id}`))?.doc ?? null,
  })

export const createSpace = (name: string) =>
  fetch('/api/kb/spaces', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name }) }).then((r) => r.json())

export const updateSpace = (
  id: string,
  patch: Partial<Pick<KbSpace, 'name' | 'description' | 'icon' | 'body' | 'visibility' | 'editPolicy'>> & { editors?: KbEditor[] },
) => fetch(`/api/kb/spaces/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

/** Fetch the current editor grants for a doc / folder / artifact (from its GET
 *  route, which returns `editors`).
 *
 *  KNOWN GAP (audit follow-up, needs a call-site fix): a failed read yields an
 *  empty grant list, and permissions-modal saves `editors: grants` — so saving
 *  any unrelated change after a blip would wipe every grant. The modal must
 *  block Save on a failed load; throwing from here alone would only convert
 *  that into an unhandled rejection with the same empty list behind it. */
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
  patch: Partial<Pick<KbDoc, 'title' | 'body' | 'icon' | 'visibility' | 'editPolicy' | 'permsInherited' | 'official' | 'ragRouting'>> & { editors?: KbEditor[] },
) => fetch(`/api/kb/docs/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

/** Custom brains (names only for members) — the doc "Brain" routing picker. */
export const useBrains = () =>
  useQuery({
    queryKey: ['rag-collections-public'],
    queryFn: async (): Promise<Array<{ id: string; name: string; kind: string }>> => {
      const all = await getList<{ id: string; name: string; kind: string }>('/api/rag/collections', 'collections')
      return all.filter((c) => c.kind === 'custom')
    },
  })

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
  /** Spaces are documents too (their overview) — hits open the space itself. */
  kind: 'doc' | 'space'
}
/** Search-as-you-type, called from bare `.then()` chains in three components.
 *  Same story as `loadConversation`: throwing needs those call sites to grow a
 *  failure branch first (audit follow-up). */
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
    queryFn: (): Promise<KbBacklink[]> => getList<KbBacklink>(`/api/kb/docs/${docId}/backlinks`, 'backlinks'),
  })
