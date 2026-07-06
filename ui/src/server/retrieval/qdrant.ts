// Minimal Qdrant client (REST) for Talaria's retrieval layer. We only need
// collection create, point upsert/delete, and filtered search — no SDK.
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

export async function deleteCollection(name: string): Promise<void> {
  await q(`/collections/${name}`, 'DELETE').catch(() => {})
}

export interface QdrantPoint {
  id: string
  vector: number[]
  payload: Record<string, unknown>
}

export async function upsertPoints(collection: string, points: QdrantPoint[]): Promise<void> {
  if (points.length === 0) return
  const r = await q(`/collections/${collection}/points?wait=true`, 'PUT', { points })
  if (!r.ok) throw new Error(`qdrant upsert: ${r.status} ${await r.text().catch(() => '')}`)
}

export async function deletePoints(collection: string, ids: string[]): Promise<void> {
  if (ids.length === 0) return
  await q(`/collections/${collection}/points/delete?wait=true`, 'POST', { points: ids }).catch(() => {})
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
