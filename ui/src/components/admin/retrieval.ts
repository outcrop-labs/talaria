// Shared types + queries for the Retrieval admin panel
// (RetrievalPanel/CollectionRow/RerankSection.svelte).
import { createQuery } from '@tanstack/svelte-query'
import { getJson, getList } from '@/lib/fetch-json'

export interface Binding {
  principalType: 'all' | 'user' | 'agent' | 'team'
  principalId: string | null
}
export interface RagCollection {
  id: string
  name: string
  kind: 'activity' | 'org-kb' | 'custom'
  description: string | null
  auto: boolean
  bindings: Binding[]
}
export interface RerankProviderMeta {
  id: string
  label: string
  country: string
  needsUrl: boolean
  needsKey: boolean
  liveCatalog: boolean
  fallbackModels: string[]
}
/** One collection as the vector store holds it. */
export interface IndexedCollection {
  id: string
  name: string
  pointsCount: number
  denseDim: number | null
  hybrid: boolean
  dimMismatch: boolean
  missing: boolean
}
export interface RagAdmin {
  health: { qdrant: boolean; embeddings: boolean }
  backfill: { state: 'idle' | 'running' | 'done' | 'error'; counts?: Record<string, number>; error?: string; finishedAt?: string }
  upgrade: {
    embed: { modelId: string; dim: number } | null
    collections: IndexedCollection[]
    dimMismatch: boolean
    legacySchema: boolean
    needsReindex: boolean
  } | null
  reindex: { state: 'idle' | 'running' | 'done' | 'error'; phase?: 'rebuilding' | 'backfilling'; error?: string }
  rerank: { providers: RerankProviderMeta[]; config: { provider: string; url?: string; model?: string; hasKey: boolean; candidates?: number } }
  spaces: Array<{ id: string; name: string; collectionId: string | null }>
}

// GET /api/rag/collections is 200 `{ collections }` for anyone signed in (it
// narrows the payload for non-admins rather than 404ing), so a non-2xx is only
// ever a failure. It used to answer `[]` — which drew the panel with no brains
// at all and a "Create" box, i.e. an invitation to rebuild what already exists.
export const useCollections = () =>
  createQuery(() => ({
    queryKey: ['rag-collections'],
    queryFn: (): Promise<RagCollection[]> => getList<RagCollection>('/api/rag/collections', 'collections'),
  }))

export const useRagAdmin = () =>
  createQuery(() => ({
    queryKey: ['rag-admin'],
    queryFn: (): Promise<RagAdmin> => getJson<RagAdmin>('/api/admin/rag'),
    refetchInterval: (q) =>
      q.state.data?.backfill.state === 'running' || q.state.data?.reindex.state === 'running' ? 3_000 : false,
  }))

/** What a collection's row says about its index. `broken` is a state search
 *  fails in, and a rebuild is the one fix for all of them. */
export interface IndexState {
  status: 'ok' | 'warn' | 'danger'
  word: string
  broken: boolean
}

export function indexState(c: IndexedCollection): IndexState {
  if (c.missing) return { status: 'danger', word: 'Not in the vector store', broken: true }
  if (c.dimMismatch) return { status: 'danger', word: 'Wrong embedding size', broken: true }
  if (!c.hybrid) return { status: 'warn', word: 'No keyword search', broken: false }
  return { status: 'ok', word: 'Indexed', broken: false }
}
