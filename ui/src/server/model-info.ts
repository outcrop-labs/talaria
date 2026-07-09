// Human-readable model identity, populated automatically. OpenRouter's public
// catalog (no key) carries a pretty name + description for essentially every
// major model; we match registered ids against it the same way the price
// oracle does (full id for slashed ids, unambiguous suffix for bare ones) and
// serve a display label + a one-line "what it's good at" blurb. Unknown models
// (e.g. self-hosted) simply have no blurb — nothing is invented.
import { NATIVE_BASE } from './provider-catalog'

export interface ModelInfo {
  /** Pretty display name, e.g. "Qwen: Qwen3 14B". */
  label: string
  /** First sentence of the catalog description, clamped. */
  blurb: string
}

interface Catalog {
  byId: Map<string, ModelInfo>
  bySuffix: Map<string, ModelInfo[]>
}

const normalize = (s: string) => s.toLowerCase().replace(/\./g, '-')

/** One line out of a marketing paragraph: first sentence, clamped. */
const toBlurb = (description: string): string => {
  const first = description.replace(/\s+/g, ' ').trim().split(/(?<=\.)\s/)[0] ?? ''
  return first.length > 160 ? `${first.slice(0, 157)}…` : first
}

let cache: { at: number; catalog: Catalog } | null = null
const TTL_MS = 6 * 60 * 60_000

async function catalog(): Promise<Catalog | null> {
  if (cache && Date.now() - cache.at < TTL_MS) return cache.catalog
  try {
    const r = await fetch(`${NATIVE_BASE['openrouter']}/models`, { signal: AbortSignal.timeout(10_000) })
    if (!r.ok) throw new Error(`models ${r.status}`)
    const j = (await r.json()) as { data?: Array<{ id?: string; name?: string; description?: string }> }
    const byId = new Map<string, ModelInfo>()
    const bySuffix = new Map<string, ModelInfo[]>()
    for (const m of j.data ?? []) {
      if (!m.id || !m.name) continue
      const info: ModelInfo = { label: m.name, blurb: toBlurb(m.description ?? '') }
      byId.set(normalize(m.id), info)
      const [, ...rest] = m.id.split('/')
      const suffix = normalize(rest.join('/'))
      if (suffix) bySuffix.set(suffix, [...(bySuffix.get(suffix) ?? []), info])
    }
    cache = { at: Date.now(), catalog: { byId, bySuffix } }
    return cache.catalog
  } catch {
    return cache?.catalog ?? null
  }
}

/** Info for one registered model id, or null when nothing matches cleanly. */
export async function modelInfo(modelId: string): Promise<ModelInfo | null> {
  const cat = await catalog()
  if (!cat) return null
  if (modelId.includes('/')) return cat.byId.get(normalize(modelId)) ?? null
  const candidates = cat.bySuffix.get(normalize(modelId)) ?? []
  return candidates.length === 1 ? candidates[0]! : null // ambiguous bare id → no guess
}
