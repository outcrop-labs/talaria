// Reranking — the precision stage after vector recall. Vector search across
// collections merges by raw cosine score (not truly comparable, and bi-encoder
// recall is fuzzy); a cross-encoder rescoring the query against each candidate
// fixes both. Providers are a registry (like LLM endpoints): pick one, add a
// key, choose a model — model lists are fetched LIVE where the provider has a
// catalog API, with documented fallbacks where none exists. Keys are sealed
// (envelope-encrypted) in app_settings; plaintext never reaches the client.
// Reranking is best-effort — a provider failure falls back to vector order,
// never breaks search.
import { getSetting, setSetting } from '../audit'
import { listEndpoints } from '../agent-defs'
import { resolveEndpointKey } from '../provider-catalog'
import { open, seal } from '../secretbox'

/** OpenRouter's key can come from the LLM endpoint the org already registered
 *  — the rerank config only needs its own key when none exists there. */
async function openrouterFallbackKey(): Promise<string | null> {
  const ep = (await listEndpoints()).find((e) => e.provider === 'openrouter' || e.name === 'openrouter')
  return ep ? resolveEndpointKey(ep) : null
}

export type RerankProviderId = 'off' | 'tei' | 'openrouter' | 'voyage' | 'together' | 'nvidia' | 'pinecone' | 'cohere' | 'jina'

export interface RerankProviderMeta {
  id: RerankProviderId
  label: string
  /** HQ country — surfaced so no-train/US-routing shops choose knowingly. */
  country: string
  needsUrl: boolean
  needsKey: boolean
  /** Documented models — the fallback when there's no live catalog API. */
  fallbackModels: string[]
  /** Whether we can list models live (with a key). */
  liveCatalog: boolean
}

export const RERANK_PROVIDERS: RerankProviderMeta[] = [
  {
    id: 'tei',
    label: 'Self-hosted (TEI)',
    country: 'your hardware',
    needsUrl: true,
    needsKey: false,
    fallbackModels: [],
    liveCatalog: false,
  },
  // OpenRouter reuses the LLM endpoint's registered key automatically when no
  // rerank-specific key is set — one key, whole stack.
  { id: 'openrouter', label: 'OpenRouter', country: 'US', needsUrl: false, needsKey: true, fallbackModels: ['cohere/rerank-v3.5', 'baai/bge-reranker-v2-m3'], liveCatalog: true },
  { id: 'voyage', label: 'Voyage AI', country: 'US', needsUrl: false, needsKey: true, fallbackModels: ['rerank-2.5', 'rerank-2.5-lite', 'rerank-2', 'rerank-2-lite'], liveCatalog: false },
  { id: 'together', label: 'Together AI', country: 'US', needsUrl: false, needsKey: true, fallbackModels: ['Salesforce/Llama-Rank-V1', 'mixedbread-ai/Mxbai-Rerank-Large-V2'], liveCatalog: true },
  { id: 'nvidia', label: 'NVIDIA', country: 'US', needsUrl: false, needsKey: true, fallbackModels: ['nvidia/llama-3.2-nv-rerankqa-1b-v2', 'nvidia/nv-rerankqa-mistral-4b-v3'], liveCatalog: false },
  { id: 'pinecone', label: 'Pinecone', country: 'US', needsUrl: false, needsKey: true, fallbackModels: ['bge-reranker-v2-m3', 'pinecone-rerank-v0', 'cohere-rerank-3.5'], liveCatalog: true },
  { id: 'cohere', label: 'Cohere', country: 'Canada', needsUrl: false, needsKey: true, fallbackModels: ['rerank-v3.5', 'rerank-english-v3.0', 'rerank-multilingual-v3.0'], liveCatalog: true },
  { id: 'jina', label: 'Jina AI', country: 'Germany', needsUrl: false, needsKey: true, fallbackModels: ['jina-reranker-v2-base-multilingual', 'jina-reranker-m0'], liveCatalog: false },
]

export interface RerankConfig {
  provider: RerankProviderId
  url?: string
  model?: string
  keySealed?: string
  /** How many merged candidates to rescore (cost vs recall). */
  candidates?: number
}

const KEY = 'rag_rerank_config'
const DEFAULTS: RerankConfig = { provider: 'off', candidates: 30 }

export const getRerankConfig = () => getSetting<RerankConfig>(KEY, DEFAULTS)

export async function setRerankConfig(patch: {
  provider?: RerankProviderId
  url?: string | null
  model?: string | null
  apiKey?: string | null // plaintext in, sealed at rest; null clears
  candidates?: number
}): Promise<RerankConfig> {
  const cur = await getRerankConfig()
  const next: RerankConfig = { ...cur }
  if (patch.provider !== undefined) next.provider = patch.provider
  if (patch.url !== undefined) next.url = patch.url ?? undefined
  if (patch.model !== undefined) next.model = patch.model ?? undefined
  if (patch.candidates !== undefined) next.candidates = Math.min(100, Math.max(5, patch.candidates))
  if (patch.apiKey !== undefined) next.keySealed = patch.apiKey ? seal(patch.apiKey) : undefined
  await setSetting(KEY, next)
  return next
}

/** Redacted view for the admin UI. */
export async function rerankConfigPublic(): Promise<Omit<RerankConfig, 'keySealed'> & { hasKey: boolean }> {
  const { keySealed, ...rest } = await getRerankConfig()
  return { ...rest, hasKey: !!keySealed }
}

const jsonFetch = async (url: string, init: RequestInit = {}): Promise<unknown> => {
  const r = await fetch(url, { ...init, signal: AbortSignal.timeout(15_000) })
  if (!r.ok) throw new Error(`${r.status}: ${(await r.text()).slice(0, 200)}`)
  return r.json()
}

/** Live model catalog for a provider (falls back to the documented list).
 *  Mirrors the LLM-endpoint pattern: fetch live wherever an API exists. */
export async function rerankModels(provider: RerankProviderId, apiKey?: string | null): Promise<string[]> {
  const meta = RERANK_PROVIDERS.find((p) => p.id === provider)
  if (!meta) return []
  let plaintext = apiKey ?? null
  if (!plaintext) {
    const cfg = await getRerankConfig()
    if (cfg.provider === provider && cfg.keySealed) plaintext = open(cfg.keySealed)
  }
  try {
    if (provider === 'cohere' && plaintext) {
      const j = (await jsonFetch('https://api.cohere.com/v1/models?endpoint=rerank&page_size=50', {
        headers: { authorization: `Bearer ${plaintext}` },
      })) as { models?: Array<{ name?: string }> }
      const ids = j.models?.map((m) => m.name).filter((n): n is string => !!n) ?? []
      if (ids.length) return ids
    }
    if (provider === 'together' && plaintext) {
      const j = (await jsonFetch('https://api.together.xyz/v1/models', {
        headers: { authorization: `Bearer ${plaintext}` },
      })) as Array<{ id?: string; type?: string }>
      const ids = j.filter((m) => m.type === 'rerank').map((m) => m.id!).filter(Boolean)
      if (ids.length) return ids
    }
    if (provider === 'pinecone' && plaintext) {
      const j = (await jsonFetch('https://api.pinecone.io/models?type=rerank', {
        headers: { 'Api-Key': plaintext, 'X-Pinecone-API-Version': '2025-01' },
      })) as { models?: Array<{ model?: string }> }
      const ids = j.models?.map((m) => m.model).filter((n): n is string => !!n) ?? []
      if (ids.length) return ids
    }
    if (provider === 'openrouter') {
      // The public catalog needs no key; rerank models are the ones tagged so.
      const j = (await jsonFetch('https://openrouter.ai/api/v1/models')) as {
        data?: Array<{ id?: string; architecture?: { output_modalities?: string[] } }>
      }
      const ids = (j.data ?? [])
        .filter((m) => m.id && (m.architecture?.output_modalities?.includes('rerank') || /rerank/i.test(m.id)))
        .map((m) => m.id!)
      if (ids.length) return ids
    }
  } catch {
    /* live catalog unreachable → documented list */
  }
  return meta.fallbackModels
}

interface TeiScored {
  index: number
  score: number
}

/** Rescore candidates against the query. Returns per-candidate scores aligned
 *  to the input order, or null when reranking is off/unconfigured/failing. */
export async function rerank(query: string, texts: string[]): Promise<number[] | null> {
  if (texts.length === 0) return null
  const cfg = await getRerankConfig()
  const align = (results: Array<{ index: number; score: number }>): number[] => {
    const scores = new Array<number>(texts.length).fill(0)
    for (const s of results) if (s.index >= 0 && s.index < texts.length) scores[s.index] = s.score
    return scores
  }
  const auth = (): Record<string, string> => (cfg.keySealed ? { authorization: `Bearer ${open(cfg.keySealed)}` } : {})
  const post = (url: string, body: unknown, headers: Record<string, string> = {}) =>
    jsonFetch(url, { method: 'POST', headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(body) })

  try {
    switch (cfg.provider) {
      case 'tei': {
        if (!cfg.url) return null
        const res = (await post(`${cfg.url.replace(/\/$/, '')}/rerank`, { query, texts, truncate: true })) as TeiScored[]
        return align(res)
      }
      case 'openrouter': {
        const key = cfg.keySealed ? open(cfg.keySealed) : await openrouterFallbackKey()
        if (!key) return null
        const res = (await post(
          'https://openrouter.ai/api/v1/rerank',
          { model: cfg.model || 'cohere/rerank-v3.5', query, documents: texts },
          { authorization: `Bearer ${key}` },
        )) as { results?: Array<{ index: number; relevance_score: number }> }
        return align((res.results ?? []).map((r) => ({ index: r.index, score: r.relevance_score })))
      }
      case 'voyage': {
        if (!cfg.keySealed) return null
        const res = (await post('https://api.voyageai.com/v1/rerank', { model: cfg.model || 'rerank-2.5-lite', query, documents: texts }, auth())) as {
          data?: Array<{ index: number; relevance_score: number }>
        }
        return align((res.data ?? []).map((r) => ({ index: r.index, score: r.relevance_score })))
      }
      case 'together': {
        if (!cfg.keySealed) return null
        const res = (await post('https://api.together.xyz/v1/rerank', { model: cfg.model || 'Salesforce/Llama-Rank-V1', query, documents: texts }, auth())) as {
          results?: Array<{ index: number; relevance_score: number }>
        }
        return align((res.results ?? []).map((r) => ({ index: r.index, score: r.relevance_score })))
      }
      case 'nvidia': {
        if (!cfg.keySealed) return null
        const model = cfg.model || 'nvidia/llama-3.2-nv-rerankqa-1b-v2'
        const res = (await post(
          `https://ai.api.nvidia.com/v1/retrieval/${model.replace('/', '/')}/reranking`,
          { model, query: { text: query }, passages: texts.map((t) => ({ text: t })) },
          auth(),
        )) as { rankings?: Array<{ index: number; logit: number }> }
        return align((res.rankings ?? []).map((r) => ({ index: r.index, score: r.logit })))
      }
      case 'pinecone': {
        if (!cfg.keySealed) return null
        const res = (await post(
          'https://api.pinecone.io/rerank',
          { model: cfg.model || 'bge-reranker-v2-m3', query, documents: texts.map((t) => ({ text: t })) },
          { 'Api-Key': open(cfg.keySealed), 'X-Pinecone-API-Version': '2025-01' },
        )) as { data?: Array<{ index: number; score: number }> }
        return align(res.data ?? [])
      }
      case 'cohere': {
        if (!cfg.keySealed) return null
        const res = (await post('https://api.cohere.com/v2/rerank', { model: cfg.model || 'rerank-v3.5', query, documents: texts }, auth())) as {
          results?: Array<{ index: number; relevance_score: number }>
        }
        return align((res.results ?? []).map((r) => ({ index: r.index, score: r.relevance_score })))
      }
      case 'jina': {
        if (!cfg.keySealed) return null
        const res = (await post('https://api.jina.ai/v1/rerank', { model: cfg.model || 'jina-reranker-v2-base-multilingual', query, documents: texts }, auth())) as {
          results?: Array<{ index: number; relevance_score: number }>
        }
        return align((res.results ?? []).map((r) => ({ index: r.index, score: r.relevance_score })))
      }
      default:
        return null
    }
  } catch {
    return null // fall back to vector order — reranking must never break search
  }
}
