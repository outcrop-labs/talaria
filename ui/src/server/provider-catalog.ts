// Live provider catalogs: ask each backend what models it actually offers
// (GET /models on OpenAI-compatible APIs; provider-specific hosts for the
// native ones). Keys resolve from the server env or the fleet .env — values
// never leave the server.
import { readFile } from 'node:fs/promises'
import type { LlmEndpoint } from './agent-defs'
import { db } from './db/pg'
import { open, seal } from './secretbox'
import { FLEET_ENV } from './fleet-dir'

/** Env names the catalog fetch may read. Only provider-key-shaped vars — an
 *  endpoint pairs an admin-chosen env name with an admin-chosen base URL, so
 *  without this gate an admin could exfiltrate ANY env value (GH_TOKEN,
 *  HERMES_KEY_*, LITELLM_MASTER_KEY) to an arbitrary host. Cross-provider
 *  key-to-wrong-host remains possible for *_API_KEY names — that's the same
 *  residual class as agent-config rendering and is part of the admin trust
 *  model; everything else is refused here. */
export const KEY_ENV_RE = /^(LLM_API_KEY|[A-Z][A-Z0-9_]*_API_KEY)$/

export async function resolveKey(envVar: string | null | undefined): Promise<string | null> {
  if (!envVar || !KEY_ENV_RE.test(envVar)) return null
  if (process.env[envVar]) return process.env[envVar]!.trim()
  const env = await readFile(FLEET_ENV(), 'utf8').catch(() => '')
  const m = new RegExp(`^${envVar}=(.*)$`, 'm').exec(env)
  return m ? m[1]!.trim() : null
}

/** The provider's API key: the sealed key stored in the DB is the source of
 *  truth (encrypted at rest, entered via /models — never in a config file). An
 *  env var / fleet .env of the endpoint's api_key_env is a fallback for ops
 *  overrides and pre-migration deployments. */
export async function resolveEndpointKey(ep: Pick<LlmEndpoint, 'id' | 'apiKeyEnv' | 'provider'>): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select api_key_cipher from llm_endpoints where id = ${ep.id}`) as unknown as Array<{
    api_key_cipher: string | null
  }>
  const cipher = rows[0]?.api_key_cipher
  if (cipher) {
    try {
      return open(cipher)
    } catch {
      /* tampered/old key material — fall back to env */
    }
  }
  return resolveKey(ep.apiKeyEnv ?? DEFAULT_KEY_ENV[ep.provider] ?? null)
}

/** One-time migration: seal any provider key that still lives only in the env /
 *  fleet .env into the DB, so keys stop depending on config files. Idempotent —
 *  only touches endpoints with no sealed key yet. Safe to call on every boot. */
export async function migrateEnvKeysToCipher(): Promise<number> {
  const sql = await db()
  const eps = (await sql`
    select id, provider, api_key_env as "apiKeyEnv" from llm_endpoints where api_key_cipher is null
  `) as unknown as Array<{ id: string; provider: string; apiKeyEnv: string | null }>
  let sealed = 0
  for (const ep of eps) {
    const key = await resolveKey(ep.apiKeyEnv ?? DEFAULT_KEY_ENV[ep.provider] ?? null)
    if (!key) continue
    await sql`update llm_endpoints set api_key_cipher = ${seal(key)}, updated_at = now() where id = ${ep.id}`
    sealed++
  }
  return sealed
}

export const NATIVE_BASE: Record<string, string> = {
  anthropic: 'https://api.anthropic.com/v1',
  openai: 'https://api.openai.com/v1',
  openrouter: 'https://openrouter.ai/api/v1',
  deepseek: 'https://api.deepseek.com/v1',
  'x-ai': 'https://api.x.ai/v1',
}

// Endpoints imported from configs often omit api_key (the provider reads its
// ambient env var) — assume the conventional name per provider.
export const DEFAULT_KEY_ENV: Record<string, string> = {
  anthropic: 'ANTHROPIC_API_KEY',
  openai: 'OPENAI_API_KEY',
  openrouter: 'OPENROUTER_API_KEY',
  deepseek: 'DEEPSEEK_API_KEY',
  'x-ai': 'XAI_API_KEY',
}

/** Fetch with a dev-mode fallback: docker-internal hostnames (bare names like
 *  inference-router) don't resolve from the host — retry on localhost with the
 *  same port, where the compose stacks publish their ports. */
async function fetchModels(base: string, headers: Record<string, string>, qs = ''): Promise<Response> {
  try {
    return await fetch(`${base}/models${qs}`, { headers, signal: AbortSignal.timeout(10_000) })
  } catch (err) {
    const m = /^(https?):\/\/([^/:]+)(:\d+)?(\/.*)?$/.exec(base)
    const host = m?.[2] ?? ''
    if (m && host && !host.includes('.') && host !== 'localhost') {
      const local = `${m[1]}://localhost${m[3] ?? ''}${m[4] ?? ''}`
      return await fetch(`${local}/models${qs}`, { headers, signal: AbortSignal.timeout(10_000) })
    }
    throw err
  }
}

/** OpenRouter's live US provider pool for no-train routing: every provider
 *  whose datacenters include the US (or, when datacenters are unreported, is
 *  US-headquartered). Fetched fresh — never a maintained list — and cached
 *  briefly; a failed fetch serves the last good pool. The no-train half of the
 *  policy is `data_collection: 'deny'`, which OpenRouter enforces dynamically. */
let usPoolCache: { at: number; slugs: string[] } | null = null
const US_POOL_TTL_MS = 15 * 60_000

export async function openrouterUsPool(): Promise<string[] | null> {
  if (usPoolCache && Date.now() - usPoolCache.at < US_POOL_TTL_MS) return usPoolCache.slugs
  try {
    const r = await fetch(`${NATIVE_BASE['openrouter']}/providers`, { signal: AbortSignal.timeout(10_000) })
    if (!r.ok) throw new Error(`providers ${r.status}`)
    const j = (await r.json()) as {
      data?: Array<{ slug?: string; headquarters?: string | null; datacenters?: string[] | null }>
    }
    const slugs = (j.data ?? [])
      .filter((p) => p.slug && (p.datacenters?.length ? p.datacenters.includes('US') : p.headquarters === 'US'))
      .map((p) => p.slug!)
      .sort()
    if (slugs.length) usPoolCache = { at: Date.now(), slugs }
    return usPoolCache?.slugs ?? null
  } catch {
    return usPoolCache?.slugs ?? null
  }
}

/** Perplexity has no /models API — its DOCS are the catalog. The models index
 *  (Mintlify serves it as markdown) links one card per model, and each card's
 *  slug IS the API model id (verified against the request examples on the
 *  per-model pages, e.g. "model": "sonar-pro"). Still live-fetched — no
 *  maintained list; if they ship new models, new cards appear here. */
async function perplexityModels(): Promise<string[]> {
  const r = await fetch('https://docs.perplexity.ai/docs/sonar/models.md', { signal: AbortSignal.timeout(10_000) })
  if (!r.ok) throw new Error(`the Perplexity docs answered ${r.status}`)
  const ids = [...(await r.text()).matchAll(/\/docs\/sonar\/models\/([a-z0-9][a-z0-9.-]*)/g)].map((m) => m[1]!)
  const unique = [...new Set(ids)]
  if (unique.length === 0) throw new Error('could not read the model list from the Perplexity docs. Type an id manually.')
  return unique
}

/** ONE MODEL AS THE PROVIDER DESCRIBES IT — the fields every OpenAI-compatible
 *  catalog that publishes them agrees on, normalized.
 *
 *  WHY THIS TYPE EXISTS AT ALL. `availableModels` used to return `string[]`, and
 *  it parsed the provider's answer down to that: OpenRouter publishes
 *  `context_length`, `architecture.input_modalities`, `supported_parameters` and
 *  `pricing` for all 400 models it serves, and every one of those fields was
 *  read into a local and discarded. The cost was not abstract. The fitness page
 *  reported "nothing advertises a context window" for a model whose catalog
 *  entry says 1,048,576, because `smallestWindow` was reading a single integer
 *  off the ENDPOINT row — one number for a gateway serving four hundred models,
 *  written only by `fleet-federate.ts`, and not even refreshed by
 *  `ensureEndpoint`'s `on conflict do update`. Meanwhile `tools`, `json`,
 *  `json-strict` and `vision` sat unmeasured on the matrix while the provider
 *  had already answered all four for free.
 *
 *  EVERY FIELD IS OPTIONAL AND MEANS "THE PROVIDER DID NOT SAY". Never "no":
 *  see `capabilitiesFromCatalog` for why that distinction is load-bearing
 *  rather than pedantic. */
export interface CatalogModel {
  id: string
  /** Display name where the provider gives one. */
  name: string | null
  /** Total window in tokens, as advertised. Null when unpublished. */
  contextLength: number | null
  /** e.g. `['text', 'image']`. Null when the provider does not describe
   *  modalities — which is most non-OpenRouter catalogs. */
  inputModalities: string[] | null
  /** The request parameters this model accepts — `tools`, `response_format`,
   *  `structured_outputs`, `web_search_options`, … Null when unpublished. */
  supportedParameters: string[] | null
  /** The reasoning-effort levels this model accepts (`reasoning_effort`) —
   *  `['low', 'medium', 'high']` and friends, exactly as the provider spells
   *  them. Null when unpublished: most catalogs never describe parameters at
   *  all, and a model with no list is a model nobody has vouched for, not a
   *  model that was refused. The picker only appears for a non-empty list. */
  efforts: string[] | null
  /** USD per token, as strings in the source; parsed to numbers per MILLION
   *  tokens to match how everything else in Talaria quotes a price. */
  pricing: { inPerMTok: number | null; outPerMTok: number | null } | null
}

const num = (v: unknown): number | null => {
  const n = typeof v === 'string' ? Number(v) : typeof v === 'number' ? v : Number.NaN
  return Number.isFinite(n) ? n : null
}
const strings = (v: unknown): string[] | null => (Array.isArray(v) && v.every((x) => typeof x === 'string') ? (v as string[]) : null)

/** The raw shape, unioned across the catalogs Talaria talks to. Everything is
 *  optional because every one of these providers omits something. */
interface RawCatalogEntry {
  id?: string
  name?: string
  context_length?: unknown
  /** Anthropic and some OpenAI-compat servers spell it this way. */
  max_context_length?: unknown
  architecture?: { input_modalities?: unknown; modality?: unknown }
  supported_parameters?: unknown
  /** OpenRouter publishes the effort ladder here (and only here — the other
   *  catalogs Talaria reads describe no parameters at all):
   *    reasoning: { supported_efforts: ['low','medium','high'], default_effort: 'medium', … }
   *  An empty or absent list means "not vouched for", never "cannot". */
  reasoning?: { supported_efforts?: unknown }
  top_provider?: { context_length?: unknown }
  pricing?: { prompt?: unknown; completion?: unknown }
}

/** THE SMALLER OF THE TWO WINDOWS OPENROUTER PUBLISHES. `context_length` is the
 *  model's own; `top_provider.context_length` is what the endpoint it routes to
 *  will actually accept, and it is routinely lower. A claim has to hold for the
 *  request that gets made, not for the spec sheet. */
function windowOf(m: RawCatalogEntry): number | null {
  const candidates = [num(m.context_length), num(m.top_provider?.context_length), num(m.max_context_length)].filter(
    (n): n is number => n !== null && n > 0,
  )
  return candidates.length ? Math.min(...candidates) : null
}

function toCatalogModel(m: RawCatalogEntry, normalize: (id: string) => string): CatalogModel | null {
  const id = m.id ?? m.name ?? ''
  if (!id) return null
  // `modality: 'text+image->text'` is the older spelling, still served by some
  // entries. Split it rather than ignore it — it is the only vision signal
  // those rows carry.
  const modalities =
    strings(m.architecture?.input_modalities) ??
    (typeof m.architecture?.modality === 'string' ? (m.architecture.modality.split('->')[0]?.split('+').filter(Boolean) ?? null) : null)
  const inTok = num(m.pricing?.prompt)
  const outTok = num(m.pricing?.completion)
  const efforts = strings(m.reasoning?.supported_efforts)
  return {
    id: normalize(id),
    name: m.name ?? null,
    contextLength: windowOf(m),
    inputModalities: modalities,
    supportedParameters: strings(m.supported_parameters),
    efforts: efforts?.length ? efforts : null,
    pricing: inTok === null && outTok === null ? null : { inPerMTok: inTok === null ? null : inTok * 1e6, outPerMTok: outTok === null ? null : outTok * 1e6 },
  }
}

/** The models a provider reports right now, WITH everything it says about them.
 *  Throws with a human message. `availableModels` is this, projected to ids. */
export async function catalogModels(ep: LlmEndpoint): Promise<CatalogModel[]> {
  const base = (ep.baseUrl ?? NATIVE_BASE[ep.provider])?.replace(/\/$/, '')
  if (!base) throw new Error('no API base known for this provider')
  // Perplexity has no catalog API; its docs give ids and nothing else, so every
  // descriptive field is honestly null rather than guessed at.
  if (base.includes('api.perplexity.ai')) {
    return (await perplexityModels()).map((id) => ({ id, name: null, contextLength: null, inputModalities: null, supportedParameters: null, efforts: null, pricing: null }))
  }
  const keyEnv = ep.apiKeyEnv ?? DEFAULT_KEY_ENV[ep.provider] ?? null
  const key = await resolveEndpointKey(ep)

  const headers: Record<string, string> = {}
  if (ep.provider === 'anthropic') {
    if (!key) throw new Error(`add the API key for this provider (or set ${keyEnv ?? 'ANTHROPIC_API_KEY'})`)
    headers['x-api-key'] = key
    headers['anthropic-version'] = '2023-06-01'
  } else if (key) {
    headers['Authorization'] = `Bearer ${key}`
  }

  // Gemini's OpenAI-compat layer reports ids as "models/gemini-" while its
  // chat API is documented with the bare id — normalize to what chat expects.
  const normalize = base.includes('generativelanguage.googleapis.com') ? (id: string) => id.replace(/^models\//, '') : (id: string) => id

  // Follow pagination where the provider pages (Anthropic defaults to 20 per
  // page); OpenAI-compatible catalogs return everything in one response. The
  // provider's own ordering is preserved — OpenRouter lists newest first.
  const out: CatalogModel[] = []
  const seen = new Set<string>()
  let qs = ep.provider === 'anthropic' ? '?limit=1000' : ''
  for (let page = 0; page < 20; page++) {
    const r = await fetchModels(base, headers, qs)
    if (!r.ok) {
      const hint =
        r.status === 401
          ? ': check the API key env'
          : r.status === 404
            ? ": this provider doesn't publish a model catalog; type model ids from its docs"
            : ''
      throw new Error(`provider answered ${r.status}${hint}`)
    }
    const j = (await r.json()) as
      | RawCatalogEntry[] // Together returns a bare array
      | { data?: RawCatalogEntry[]; models?: RawCatalogEntry[]; has_more?: boolean; last_id?: string }
    const list = Array.isArray(j) ? j : (j.data ?? j.models ?? [])
    for (const raw of list) {
      const m = toCatalogModel(raw, normalize)
      // First entry wins, matching the old `new Set(ids)` dedupe: the provider's
      // own ordering puts the canonical row first.
      if (m && !seen.has(m.id)) {
        seen.add(m.id)
        out.push(m)
      }
    }
    if (Array.isArray(j) || !j.has_more || !j.last_id) break
    qs = `${ep.provider === 'anthropic' ? '?limit=1000&' : '?'}after_id=${encodeURIComponent(j.last_id)}`
  }
  return out
}

/** The model IDS a provider reports right now. Kept as the narrow answer for the
 *  three callers that only ever wanted a picker list. */
export async function availableModels(ep: LlmEndpoint): Promise<string[]> {
  return (await catalogModels(ep)).map((m) => m.id)
}

