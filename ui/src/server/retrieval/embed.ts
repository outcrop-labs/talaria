// Embeddings client — talks to the self-hosted TEI (text-embeddings-inference)
// service. Configurable URL; the docker-hostname → localhost fallback lets it
// work whether Talaria runs on the host (dev) or on ai_default (prod).
const EMBED_URL = () => (process.env.TALARIA_EMBED_URL ?? 'http://embeddings:80').replace(/\/$/, '')

async function post(url: string, body: unknown): Promise<Response> {
  const init = {
    method: 'POST' as const,
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  }
  try {
    return await fetch(`${url}/embed`, init)
  } catch (err) {
    // Dev fallback: bare docker hostnames don't resolve from the host; TEI is
    // published on 127.0.0.1:8055 there.
    const m = /^(https?):\/\/([^/:]+)(:\d+)?$/.exec(url)
    if (m && m[2] && !m[2].includes('.') && m[2] !== 'localhost') {
      return await fetch(`http://127.0.0.1:8055/embed`, init)
    }
    throw err
  }
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
  const get = async (url: string) => {
    const init = { signal: AbortSignal.timeout(5_000) }
    try {
      return await fetch(`${url}/info`, init)
    } catch (err) {
      const m = /^(https?):\/\/([^/:]+)(:\d+)?$/.exec(url)
      if (m && m[2] && !m[2].includes('.') && m[2] !== 'localhost') {
        return await fetch(`http://127.0.0.1:8055/info`, init)
      }
      throw err
    }
  }
  try {
    const r = await get(EMBED_URL())
    const j = r.ok ? ((await r.json()) as { model_id?: string }) : {}
    const dim = (await embed('dimension probe'))[0]!.length
    return { modelId: j.model_id ?? 'unknown', dim }
  } catch {
    return null
  }
}
