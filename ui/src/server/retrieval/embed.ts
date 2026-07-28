// Embeddings client — talks to the self-hosted TEI (text-embeddings-inference)
// service. Configurable URL; the docker-hostname → localhost fallback lets it
// work whether Talaria runs on the host (dev) or on ai_default (prod).
const EMBED_URL = () => (process.env.TALARIA_EMBED_URL ?? 'http://embeddings:80').replace(/\/$/, '')
const EMBED_FALLBACK = 'http://127.0.0.1:8055'

// The configured URL may be a docker-internal hostname that doesn't resolve
// from the host (dev). Resolving it FAILS SLOWLY — multi-second DNS timeouts —
// so remember which base actually answered and go straight there afterwards.
// Without this, every single embed call (search, indexing, health probes)
// paid the stall before falling back.
let stickyBase: string | null = null

/** Fetch against TEI with the hostname fallback, remembering the winner. */
export async function embedFetch(path: string, init: RequestInit): Promise<Response> {
  const primary = EMBED_URL()
  const base = stickyBase ?? primary
  try {
    const r = await fetch(`${base}${path}`, init)
    stickyBase = base
    return r
  } catch (err) {
    // Bare docker hostnames don't resolve from the host; TEI is published on
    // 127.0.0.1:8055 there. Only fall back when the primary looks docker-bare.
    const m = /^(https?):\/\/([^/:]+)(:\d+)?$/.exec(base)
    if (m && m[2] && !m[2].includes('.') && m[2] !== 'localhost' && base !== EMBED_FALLBACK) {
      const r = await fetch(`${EMBED_FALLBACK}${path}`, init)
      stickyBase = EMBED_FALLBACK
      return r
    }
    stickyBase = null // forget a base that stopped answering
    throw err
  }
}

async function post(_url: string, body: unknown): Promise<Response> {
  return embedFetch('/embed', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  })
}

/** Embed one or many texts. TEI returns a vector per input. */
export async function embed(inputs: string | string[]): Promise<number[][]> {
  const arr = Array.isArray(inputs) ? inputs : [inputs]
  if (arr.length === 0) return []
  const r = await post(EMBED_URL(), { inputs: arr, truncate: true })
  if (!r.ok) throw new Error(`embeddings ${r.status}`)
  return (await r.json()) as number[][]
}

export async function embedOne(text: string): Promise<number[]> {
  return (await embed(text))[0]!
}

/** The model's vector dimension, probed once. */
let cachedDim: number | null = null
export async function embedDim(): Promise<number> {
  if (cachedDim) return cachedDim
  cachedDim = (await embedOne('dimension probe')).length
  return cachedDim
}

export interface EmbedInfo {
  modelId: string
  dim: number
}

/** What the embedding service is serving RIGHT NOW (TEI /info + a live dim
 *  probe — never the process cache, since migration decisions hang on it). */
export async function embedInfo(): Promise<EmbedInfo | null> {
  try {
    const r = await embedFetch('/info', { signal: AbortSignal.timeout(5_000) })
    const j = r.ok ? ((await r.json()) as { model_id?: string }) : {}
    const dim = (await embed('dimension probe'))[0]!.length
    return { modelId: j.model_id ?? 'unknown', dim }
  } catch {
    return null
  }
}
