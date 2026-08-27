import { createQuery } from '@tanstack/svelte-query'
import { delJson, getJsonOr404, getList, postJson, putJson } from '@/lib/fetch-json'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export type Visibility = 'private' | 'org' | 'public'
export type EditPolicy = 'owner' | 'org' | 'restricted'

/** What the Share dialog can be pointed at. All four share one access model,
 *  one grants table, and one dialog — the kind only picks the REST path. */
export type PermKind = 'docs' | 'spaces' | 'artifacts' | 'artifact-folders'
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

export const useSpace = (id: MaybeGetter<string | null>) =>
  createQuery(() => {
    const i = resolve(id)
    return {
      queryKey: ['kb-space', i],
      enabled: !!i,
      // 404 = the space is gone or was never yours: a real "not found".
      queryFn: async (): Promise<KbSpace | null> =>
        (await getJsonOr404<{ space: KbSpace }>(`/api/kb/spaces/${i}`))?.space ?? null,
    }
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
  createQuery(() => ({
    queryKey: ['kb-spaces'],
    queryFn: (): Promise<KbSpace[]> => getList<KbSpace>('/api/kb/spaces', 'spaces'),
  }))

export const useDocs = (spaceId: MaybeGetter<string | null>) =>
  createQuery(() => {
    const i = resolve(spaceId)
    return {
      queryKey: ['kb-docs', i],
      enabled: !!i,
      queryFn: (): Promise<KbDocMeta[]> => getList<KbDocMeta>(`/api/kb/spaces/${i}/docs`, 'docs'),
    }
  })

export const useDoc = (id: MaybeGetter<string | null>) =>
  createQuery(() => {
    const i = resolve(id)
    return {
      queryKey: ['kb-doc', i],
      enabled: !!i,
      // 404 = the doc is gone or was never yours: a real "not found".
      queryFn: async (): Promise<KbDoc | null> =>
        (await getJsonOr404<{ doc: KbDoc }>(`/api/kb/docs/${i}`))?.doc ?? null,
    }
  })

// Mutations go through the fetch-json door: a non-2xx REJECTS with the
// server's sentence. These used to `.then((r) => r.json())`, which resolved
// the `{ error }` body as if it were the created record — a 403 "only the
// owner can change sharing" read to the caller as a successful save.

export const createSpace = (name: string): Promise<KbSpace> =>
  postJson<{ space: KbSpace }>('/api/kb/spaces', { name }).then((r) => r.space)

export const updateSpace = (
  id: string,
  patch: Partial<Pick<KbSpace, 'name' | 'description' | 'icon' | 'body' | 'visibility' | 'editPolicy'>> & { editors?: KbEditor[] },
): Promise<KbSpace> => putJson<{ space: KbSpace }>(`/api/kb/spaces/${id}`, patch).then((r) => r.space)

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
const fetchEditors = (kind: PermKind, id: string): Promise<KbEditor[]> =>
  getList<KbEditor>(
    kind === 'artifacts' ? `/api/artifacts/${id}`
    : kind === 'artifact-folders' ? `/api/artifact-folders/${id}`
    : `/api/kb/${kind}/${id}`,
    'editors',
  )

/** The Share modal's read of the current grants. Enabled only while the modal
 *  is open, so reopening re-reads rather than trusting a stale copy. The query
 *  key lives here next to the fetcher — one place owns this read. */
export const useEditors = (kind: PermKind, id: MaybeGetter<string>, enabled: MaybeGetter<boolean>) =>
  createQuery(() => {
    const i = resolve(id)
    return {
      queryKey: ['kb-editors', kind, i],
      enabled: resolve(enabled) && !!i,
      queryFn: () => fetchEditors(kind, i),
    }
  })

export const deleteSpace = (id: string) => delJson<{ ok: true }>(`/api/kb/spaces/${id}`)

export const createDoc = (spaceId: string, input: { title?: string; kind?: 'human' | 'agent'; parentId?: string | null }): Promise<KbDoc> =>
  postJson<{ doc: KbDoc }>(`/api/kb/spaces/${spaceId}/docs`, input).then((r) => r.doc)

export const saveDoc = (
  id: string,
  patch: Partial<Pick<KbDoc, 'title' | 'body' | 'icon' | 'visibility' | 'editPolicy' | 'permsInherited' | 'official' | 'ragRouting'>> & { editors?: KbEditor[] },
): Promise<KbDoc> => putJson<{ doc: KbDoc }>(`/api/kb/docs/${id}`, patch).then((r) => r.doc)

/** Custom brains (names only for members) — the doc "Brain" routing picker. */
export const useBrains = () =>
  createQuery(() => ({
    queryKey: ['rag-collections-public'],
    queryFn: async (): Promise<Array<{ id: string; name: string; kind: string }>> => {
      const all = await getList<{ id: string; name: string; kind: string }>('/api/rag/collections', 'collections')
      return all.filter((c) => c.kind === 'custom')
    },
  }))

export const deleteDoc = (id: string) => delJson<{ ok: true }>(`/api/kb/docs/${id}`)

export const moveDoc = (id: string, parentId: string | null, sort: number): Promise<KbDoc> =>
  postJson<{ doc: KbDoc }>(`/api/kb/docs/${id}/move`, { parentId, sort }).then((r) => r.doc)

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
const searchKbHits = (q: string): Promise<KbSearchHit[]> =>
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
export const useBacklinks = (docId: MaybeGetter<string | null>) =>
  createQuery(() => {
    const i = resolve(docId)
    return {
      queryKey: ['kb-backlinks', i],
      enabled: !!i,
      queryFn: (): Promise<KbBacklink[]> => getList<KbBacklink>(`/api/kb/docs/${i}/backlinks`, 'backlinks'),
    }
  })
