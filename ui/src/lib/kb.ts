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

/** The editor grants on a doc / folder / artifact, read from its GET route.
 *
 *  THROWS on any non-2xx, and this one has to more than most: the Share modal
 *  seeds an EDITABLE list from this call and `save()` PUTs that list back
 *  wholesale. A swallowed failure here does not merely misreport — it hands
 *  Save an empty list to write over every real grant, destroying them. Reading
 *  it through `useEditors` gives the modal a real error state to render and a
 *  `data === undefined` it can gate Save on.
 *
 *  A 200 whose `editors` key is missing is a broken contract, not "nobody at
 *  all" — `getList` rejects that too, for the same reason. */
export const fetchEditors = (kind: 'docs' | 'spaces' | 'artifacts', id: string): Promise<KbEditor[]> =>
  getList<KbEditor>(kind === 'artifacts' ? `/api/artifacts/${id}` : `/api/kb/${kind}/${id}`, 'editors')

/** The Share modal's read of the current grants. Enabled only while the modal
 *  is open, so reopening re-reads rather than trusting a stale copy. The query
 *  key lives here next to the fetcher — one place owns this read. */
export const useEditors = (kind: 'docs' | 'spaces' | 'artifacts', id: string, enabled: boolean) =>
  useQuery({
    queryKey: ['kb-editors', kind, id],
    enabled: enabled && !!id,
    queryFn: () => fetchEditors(kind, id),
  })

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
/** KB search, honestly: a failed search REJECTS, so "nothing matched" and "the
 *  search broke" are different answers. This is the only function in the app
 *  that talks to /api/kb/search — reach for it, do not re-derive it. */
export const searchKbHits = (q: string): Promise<KbSearchHit[]> =>
  getList<KbSearchHit>(`/api/kb/search?q=${encodeURIComponent(q)}`, 'hits')

/** @deprecated Swallows failures as an empty result — use `searchKbHits` and
 *  render the rejection.
 *
 *  THE LAST SWALLOW, deliberately left in exactly ONE line rather than four:
 *  every remaining `.then()` search call site funnels through here, so closing
 *  it is a single edit once those sites can show a failure. They cannot yet —
 *  all three drop the promise on the floor (`void search(q).then(setState)`),
 *  so rejecting today buys an unhandled rejection and STALE results, which is
 *  a worse lie than an empty one, not a better one. The three sites that must
 *  grow a `.catch` and an error branch before this shim is deleted:
 *
 *    routes/_app/knowledge.tsx:422   KbSearch — `void searchKb(t).then(...)`
 *    routes/_app/knowledge.tsx:111   docSearch — feeds rich-editor's DocLinkPopover
 *    components/chat/attachments.tsx:112  RefPicker — try/finally, no catch
 *
 *  (rich-editor.tsx:437 drops it too, so `docSearch` cannot simply reject.) */
export const searchKb = (q: string): Promise<KbSearchHit[]> => searchKbHits(q).catch(() => [])

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
