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
export interface RagAdmin {
  health: { qdrant: boolean; embeddings: boolean }
  backfill: { state: 'idle' | 'running' | 'done' | 'error'; counts?: Record<string, number>; error?: string; finishedAt?: string }
  upgrade: {
    embed: { modelId: string; dim: number } | null
    collections: Array<{ id: string; name: string; pointsCount: number; denseDim: number | null; hybrid: boolean; dimMismatch: boolean; missing: boolean }>
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
