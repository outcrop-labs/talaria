// Minimal Qdrant client (REST) for Talaria's retrieval layer. We only need
// collection create, point upsert/delete, and filtered search — no SDK.
//
// Two collection shapes coexist:
//   legacy (v1) — one unnamed dense vector; dense-only search.
//   hybrid (v2) — named `dense` + sparse `sparse` (modifier: idf); searched
//                 with the Query API fusing both branches via RRF.
import type { SparseVector } from './sparse'

const QDRANT_URL = () => (process.env.TALARIA_QDRANT_URL ?? 'http://localhost:6333').replace(/\/$/, '')

async function q(path: string, method: string, body?: unknown): Promise<Response> {
  const url = `${QDRANT_URL()}${path}`
  const init: RequestInit = { method, signal: AbortSignal.timeout(30_000) }
  if (body !== undefined) {
    init.headers = { 'content-type': 'application/json' }
    init.body = JSON.stringify(body)
  }
  try {
    return await fetch(url, init)
  } catch (err) {
    const m = /^(https?):\/\/([^/:]+)(:\d+)?/.exec(QDRANT_URL())
    if (m && m[2] && !m[2].includes('.') && m[2] !== 'localhost') {
      return await fetch(`http://127.0.0.1:6333${path}`, init)
    }
    throw err
  }
}

export async function ensureCollection(name: string, dim: number): Promise<void> {
  const exists = await q(`/collections/${name}`, 'GET')
  if (exists.ok) return
  const r = await q(`/collections/${name}`, 'PUT', {
    vectors: { size: dim, distance: 'Cosine' },
  })
  if (!r.ok && r.status !== 409) throw new Error(`qdrant create ${name}: ${r.status}`)
}

/** Create a hybrid (v2) collection: named dense vector + IDF-modified sparse.
 *  Qdrant computes IDF server-side, so points/queries carry plain tf values. */
export async function ensureHybridCollection(name: string, dim: number): Promise<void> {
  const exists = await q(`/collections/${name}`, 'GET')
  if (exists.ok) return
  const r = await q(`/collections/${name}`, 'PUT', {
    vectors: { dense: { size: dim, distance: 'Cosine' } },
    sparse_vectors: { sparse: { modifier: 'idf' } },
  })
  if (!r.ok && r.status !== 409) throw new Error(`qdrant create ${name}: ${r.status}`)
}

export interface CollectionInfo {
  pointsCount: number
  /** Dense dimension, wherever it lives (unnamed legacy or named `dense`). */
  denseDim: number | null
  /** True when the collection has the v2 named dense + sparse layout. */
  hybrid: boolean
}

/** The ACTUAL shape of a live collection — the source of truth for migration
 *  decisions (registry columns can go stale; the collection itself cannot). */
export async function collectionInfo(name: string): Promise<CollectionInfo | null> {
  const r = await q(`/collections/${name}`, 'GET').catch(() => null)
  if (!r?.ok) return null
  const j = (await r.json()) as {
    result?: {
      points_count?: number
      config?: {
        params?: {
          vectors?: { size?: number } | Record<string, { size?: number }>
          sparse_vectors?: Record<string, unknown>
        }
      }
    }
  }
  const params = j.result?.config?.params
  const vectors = params?.vectors ?? {}
  const named = typeof (vectors as { size?: unknown }).size !== 'number'
  const denseDim = named
    ? ((vectors as Record<string, { size?: number }>).dense?.size ?? null)
    : ((vectors as { size?: number }).size ?? null)
  return {
    pointsCount: j.result?.points_count ?? 0,
    denseDim,
    hybrid: named && !!params?.sparse_vectors?.sparse,
  }
}

export async function deleteCollection(name: string): Promise<void> {
  await q(`/collections/${name}`, 'DELETE').catch(() => {})
}

export interface QdrantPoint {
  id: string
  vector: number[]
  /** Present for hybrid (v2) collections. */
  sparse?: SparseVector
  payload: Record<string, unknown>
}

export async function upsertPoints(collection: string, points: QdrantPoint[], hybrid = false): Promise<void> {
  if (points.length === 0) return
  const body = {
    points: points.map((p) =>
      hybrid
        ? { id: p.id, vector: { dense: p.vector, sparse: p.sparse ?? { indices: [], values: [] } }, payload: p.payload }
        : { id: p.id, vector: p.vector, payload: p.payload },
    ),
  }
  const r = await q(`/collections/${collection}/points?wait=true`, 'PUT', body)
  if (!r.ok) throw new Error(`qdrant upsert: ${r.status} ${await r.text().catch(() => '')}`)
}

export async function deletePoints(collection: string, ids: string[]): Promise<void> {
  if (ids.length === 0) return
  await q(`/collections/${collection}/points/delete?wait=true`, 'POST', { points: ids }).catch(() => {})
}

/** Delete every point matching a payload filter — used to purge a whole
 *  container's activity (e.g. all points with boardId=X when a board is
 *  deleted) without enumerating point ids. */
export async function deleteByFilter(collection: string, filter: Record<string, unknown>): Promise<void> {
  await q(`/collections/${collection}/points/delete?wait=true`, 'POST', { filter }).catch(() => {})
}

export interface SearchHit {
  id: string
  score: number
  payload: Record<string, unknown>
}

/** Search one collection with an optional payload filter (Qdrant filter DSL). */
export async function search(
  collection: string,
  vector: number[],
  limit: number,
  filter?: Record<string, unknown>,
): Promise<SearchHit[]> {
  const r = await q(`/collections/${collection}/points/search`, 'POST', {
    vector,
    limit,
    with_payload: true,
    ...(filter ? { filter } : {}),
  })
  if (!r.ok) return []
  const j = (await r.json()) as { result?: Array<{ id: string; score: number; payload: Record<string, unknown> }> }
  return (j.result ?? []).map((h) => ({ id: String(h.id), score: h.score, payload: h.payload ?? {} }))
}

/** Hybrid search on a v2 collection: dense + sparse prefetch branches fused
 *  with reciprocal-rank fusion. Scores are RRF ranks, not cosines — comparable
 *  within one query, meaningless across collections (the reranker, when
 *  configured, is what makes the cross-collection merge honest). */
export async function hybridQuery(
  collection: string,
  dense: number[],
  sparse: SparseVector,
  limit: number,
  filter?: Record<string, unknown>,
): Promise<SearchHit[]> {
  const branch = (query: unknown, using: string) => ({
    query,
    using,
    limit: Math.max(limit * 2, 20), // each branch over-fetches so fusion has real overlap to rank
    ...(filter ? { filter } : {}),
  })
  const prefetch = [branch(dense, 'dense')]
  if (sparse.indices.length > 0) prefetch.push(branch(sparse, 'sparse'))
  const r = await q(`/collections/${collection}/points/query`, 'POST', {
    prefetch,
    query: { fusion: 'rrf' },
    limit,
    with_payload: true,
  })
  if (!r.ok) return []
  const j = (await r.json()) as { result?: { points?: Array<{ id: string; score: number; payload: Record<string, unknown> }> } }
  return (j.result?.points ?? []).map((h) => ({ id: String(h.id), score: h.score, payload: h.payload ?? {} }))
}
