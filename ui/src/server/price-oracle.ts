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

/** "anthropic/claude-opus-4.8" → vendor "anthropic", norm "claude-opus-4-8".
 *  Leading "~" (Talaria's alias marker) never appears in public catalogs. */
const normalize = (s: string) => s.toLowerCase().replace(/^~/, '').replace(/\./g, '-')

/** The id shapes a model might wear in a public catalog, most-specific first:
 *  as-is → ":variant" stripped → trailing "-YYYYMMDD" date stripped →
 *  "-latest" stripped. Each step feeds the next, so all combinations occur. */
function idCandidates(model: string): string[] {
  const out: string[] = []
  const push = (m: string) => {
    if (m && !out.includes(m)) out.push(m)
  }
  let cur = model.replace(/^~/, '')
  push(cur)
  if (cur.includes(':')) push((cur = cur.split(':')[0]!))
  const dateless = cur.replace(/-\d{8}$/, '')
  if (dateless !== cur) push((cur = dateless))
  const unlatest = cur.replace(/-latest$/, '')
  if (unlatest !== cur) push(unlatest)
  return out
}

let lastRefresh = 0
let lastAttempt = 0 // failure backoff: don't hammer an unreachable catalog
let inFlight: Promise<{ priced: number; endpoints: number }> | null = null
const RETRY_INTERVAL_MS = 10 * 60 * 1000 // 10min between attempts after a failure

interface Catalog {
  /** Keyed by the FULL normalized id ("anthropic/claude-opus-4-8") — for
   *  vendor-prefixed model ids (OpenRouter-style endpoints). */
  byId: Map<string, { in: number; out: number }>
  /** Keyed by the normalized suffix ("claude-opus-4-8") — for bare ids. */
  bySuffix: Map<string, Array<{ vendor: string; in: number; out: number }>>
}

async function fetchCatalog(): Promise<Catalog> {
  const r = await fetch('https://openrouter.ai/api/v1/models', { signal: AbortSignal.timeout(15_000) })
  if (!r.ok) throw new Error(`openrouter ${r.status}`)
  const models = ((await r.json()) as { data?: OrModel[] }).data ?? []
  const byId = new Map<string, { in: number; out: number }>()
  const bySuffix = new Map<string, Array<{ vendor: string; in: number; out: number }>>()
  for (const m of models) {
    const inTok = Number(m.pricing?.prompt)
    const outTok = Number(m.pricing?.completion)
    if (!Number.isFinite(inTok) || !Number.isFinite(outTok)) continue
    const [vendor, ...rest] = m.id.split('/')
    const suffix = normalize(rest.join('/') || vendor!)
    const price = { in: inTok * 1e6, out: outTok * 1e6 }
    byId.set(normalize(m.id), price)
    const list = bySuffix.get(suffix) ?? []
    list.push({ vendor: normalize(vendor!), ...price })
    bySuffix.set(suffix, list)
  }
  return { byId, bySuffix }
}

/** Pick a price for a model on a provider. Vendor-prefixed ids match the full
 *  catalog id; bare ids match by suffix (exact vendor wins, else only an
 *  unambiguous candidate). Alias shapes (":free" variants, "-latest",
 *  trailing dates, "~" prefixes) fall back through idCandidates. Null = no
 *  match — stays unpriced, never guessed. */
function match(catalog: Catalog, provider: string, model: string): { in: number; out: number } | null {
  for (const id of idCandidates(model)) {
    if (id.includes('/')) {
      const hit = catalog.byId.get(normalize(id))
      if (hit) return hit
      // A vendor-prefixed id can still suffix-match when the catalog files the
      // model under a different vendor path (rare, but aliases do this).
      const [vendor, ...rest] = id.split('/')
      const suffixHit = suffixMatch(catalog, vendor!, rest.join('/'))
      if (suffixHit) return suffixHit
      continue
    }
    const hit = suffixMatch(catalog, provider, id)
    if (hit) return hit
  }
  return null
}

function suffixMatch(catalog: Catalog, vendor: string, model: string): { in: number; out: number } | null {
  const candidates = catalog.bySuffix.get(normalize(model))
  if (!candidates?.length) return null
  const vendorHit = candidates.find((c) => c.vendor === normalize(vendor))
  if (vendorHit) return { in: vendorHit.in, out: vendorHit.out }
  const first = candidates[0]!
  const unambiguous = candidates.every((c) => c.in === first.in && c.out === first.out)
  return unambiguous ? { in: first.in, out: first.out } : null
}

/** Refresh auto_prices for every cloud endpoint — its registered catalog
 *  models PLUS every model usage has actually been attributed to on it. The
 *  second set is what keeps costing honest: tier routing and aliases send
 *  usage to models nobody registered, and those rows must price too. Keys are
 *  the EXACT strings usage carries, so the costing join hits directly. */
async function refreshAutoPrices(): Promise<{ priced: number; endpoints: number }> {
  lastAttempt = Date.now()
  const catalog = await fetchCatalog()
  const sql = await db()
  const endpoints = (await sql`
    select id, name, provider, models from llm_endpoints where class = 'cloud'
  `) as unknown as Array<{ id: string; name: string; provider: string; models: string[] }>
  const seen = (await sql`
    select distinct endpoint, llm_model as model from usage_events
    where endpoint is not null and llm_model is not null
  `) as unknown as Array<{ endpoint: string; model: string }>

  let priced = 0
  for (const ep of endpoints) {
    const models = new Set<string>([
      ...(ep.models ?? []),
      ...seen.filter((s) => s.endpoint === ep.name).map((s) => s.model),
    ])
    const auto: Record<string, { in: number; out: number }> = {}
    for (const m of models) {
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

/** Single-flight around `refreshAutoPrices`. The scheduled refresh (the Rust
 *  api's `price-refresh` job) and the usage nudge both funnel through their
 *  own copy of this throttle, so this one serves the in-process callers. It
 *  REJECTS when the refresh failed; the fire-and-forget nudge below swallows
 *  on purpose, because for it the next pass is the retry. */
function refreshOnce(): Promise<{ priced: number; endpoints: number }> {
  if (inFlight) return inFlight
  const run = refreshAutoPrices()
  inFlight = run
  void run.then(
    () => {
      if (inFlight === run) inFlight = null
    },
    () => {
      if (inFlight === run) inFlight = null
    },
  )
  return run
}

// The CADENCE is the Rust api's job (`price-refresh`, api/src/price_oracle.rs)
// — it ticks every 6h so an instance nobody administers still prices its
// usage against a current catalog. What stays here is the in-process nudge:
// the metering path, pricing a model the catalog pass has not seen yet, fires
// the refresh sooner than the tick would.
/** Usage just landed on a cloud model with NO price: refresh sooner than the
 *  6h cadence (new model, new alias), but never storm — 15min between nudges,
 *  and the failure backoff still applies. */
const NUDGE_INTERVAL_MS = 15 * 60 * 1000
export function nudgeAutoPrices(): void {
  const now = Date.now()
  if (inFlight || now - lastRefresh < NUDGE_INTERVAL_MS || now - lastAttempt < RETRY_INTERVAL_MS) return
  void refreshOnce().catch(() => {})
}
