// Zero-config pricing: OpenRouter's public model catalog (no auth) carries
// per-token prices for essentially every major model. We match each cloud
// endpoint's catalog models against it by normalized name and store the result
// in llm_endpoints.auto_prices — separate from user overrides (model_prices),
// which always win. Local endpoints are skipped ($0 by definition).
import { db } from './db/pg'

interface OrModel {
  id: string
  pricing?: { prompt?: string | number; completion?: string | number }
}

/** "anthropic/claude-opus-4.8" → vendor "anthropic", norm "claude-opus-4-8". */
const normalize = (s: string) => s.toLowerCase().replace(/\./g, '-')

let lastRefresh = 0
let refreshing: Promise<void> | null = null
const MIN_INTERVAL_MS = 6 * 60 * 60 * 1000 // 6h

async function fetchCatalog(): Promise<Map<string, Array<{ vendor: string; in: number; out: number }>>> {
  const r = await fetch('https://openrouter.ai/api/v1/models', { signal: AbortSignal.timeout(15_000) })
  if (!r.ok) throw new Error(`openrouter ${r.status}`)
  const models = ((await r.json()) as { data?: OrModel[] }).data ?? []
  const bySuffix = new Map<string, Array<{ vendor: string; in: number; out: number }>>()
  for (const m of models) {
    const inTok = Number(m.pricing?.prompt)
    const outTok = Number(m.pricing?.completion)
    if (!Number.isFinite(inTok) || !Number.isFinite(outTok)) continue
    const [vendor, ...rest] = m.id.split('/')
    const suffix = normalize(rest.join('/') || vendor!)
    const entry = { vendor: normalize(vendor!), in: inTok * 1e6, out: outTok * 1e6 }
    const list = bySuffix.get(suffix) ?? []
    list.push(entry)
    bySuffix.set(suffix, list)
  }
  return bySuffix
}

/** Pick a price for a model on a provider. Exact vendor match wins; otherwise
 *  accept only an unambiguous candidate (all same price). Null = no match —
 *  stays unpriced/manual rather than guessing. */
function match(
  catalog: Map<string, Array<{ vendor: string; in: number; out: number }>>,
  provider: string,
  model: string,
): { in: number; out: number } | null {
  const candidates = catalog.get(normalize(model))
  if (!candidates?.length) return null
  const vendorHit = candidates.find((c) => c.vendor === normalize(provider))
  if (vendorHit) return { in: vendorHit.in, out: vendorHit.out }
  const first = candidates[0]!
  const unambiguous = candidates.every((c) => c.in === first.in && c.out === first.out)
  return unambiguous ? { in: first.in, out: first.out } : null
}

/** Refresh auto_prices for every cloud endpoint's catalog models. */
export async function refreshAutoPrices(): Promise<{ priced: number; endpoints: number }> {
  const catalog = await fetchCatalog()
  const sql = await db()
  const endpoints = (await sql`
    select id, name, provider, models from llm_endpoints where class = 'cloud'
  `) as unknown as Array<{ id: string; name: string; provider: string; models: string[] }>

  let priced = 0
  for (const ep of endpoints) {
    const auto: Record<string, { in: number; out: number }> = {}
    for (const m of ep.models ?? []) {
      const hit = match(catalog, ep.provider, m)
      if (hit) {
        auto[m] = { in: Number(hit.in.toFixed(4)), out: Number(hit.out.toFixed(4)) }
        priced++
      }
    }
    await sql`update llm_endpoints set auto_prices = ${sql.json(auto)}, updated_at = now() where id = ${ep.id}`
  }
  lastRefresh = Date.now()
  return { priced, endpoints: endpoints.length }
}

/** Fire-and-forget refresh, at most every 6h. Call from hot read paths. */
export function maybeRefreshAutoPrices(): void {
  if (refreshing || Date.now() - lastRefresh < MIN_INTERVAL_MS) return
  refreshing = refreshAutoPrices()
    .catch(() => {}) // offline/unreachable → prices stay as they were
    .finally(() => {
      refreshing = null
    }) as unknown as Promise<void>
}
