// WHAT THE PROVIDER ALREADY TOLD US ABOUT EACH MODEL, cached and read back.
//
// THE PROBLEM THIS SOLVES, and it was costing real verdicts. Talaria asked four
// hundred models' worth of catalog from OpenRouter on every endpoint edit,
// parsed it down to `string[]`, and threw the rest away. So the fitness page
// said "nothing advertises a context window" about a model whose catalog entry
// says 1,048,576 tokens, and left `tools`, `json`, `json-strict` and `vision`
// unmeasured — capping every slot at `workable` and asking an admin to buy
// probes — for four facts the provider publishes for free.
//
// It is a CACHE, not a source of truth. `provider-catalog.ts` does the fetching
// and stays the only thing that talks to a provider; this module decides how
// long an answer is good for, where it lives, and what a capability reader may
// conclude from it. A stale entry is served while a refresh is in flight, and a
// failed refresh serves the last good answer — the same posture `openrouterUsPool`
// takes, for the same reason: a catalog blip must not empty the model picker.
//
// ── THE ONE RULE THAT MATTERS ────────────────────────────────────────────────
// A CATALOG MAY SAY YES. IT MAY NEVER SAY NO.
//
// `supported_parameters` listing `tools` is a provider committing to something,
// and Talaria records it as a declared fact. `supported_parameters` NOT listing
// `tools` is not a denial — it is a field a hundred OpenAI-compatible servers
// never populate at all, plus vLLM, Ollama, LM Studio and every self-host that
// answers `/models` with `{id}` and nothing else.
//
// And the asymmetry is not merely cautious, it is load-bearing: `run.ts` step 2
// refuses a run outright on a capability recorded false from a `declared`
// source. Writing "false" from an absent field would take a model the platform
// has never tested, decide it cannot call tools because a self-hosted server did
// not fill in an optional array, and REFUSE the judge on it. Absence stays
// unknown, `capability.ts`'s cardinal rule (UNKNOWN IS NOT FALSE) keeps the
// model running, and a probe is what turns unknown into a no.
import { getSetting, setSetting } from './audit'
import { listEndpoints, type LlmEndpoint } from './agent-defs'
import { capabilityKey, mergeCapabilities, type Capability, type CapabilityFact } from './harness/capability'
import { catalogModels, type CatalogModel } from './provider-catalog'

/** One endpoint's catalog, as of `at`. Keyed by endpoint NAME because that is
 *  what `capabilityKey` and the gateway's routing both speak. */
export interface EndpointCatalog {
  endpoint: string
  at: string
  models: CatalogModel[]
}

export type CatalogStore = Record<string, EndpointCatalog>

const KEY = 'model_catalog'

/** A CATALOG IS NOT VOLATILE, and a day is the honest number. Providers add
 *  models continuously and change a model's advertised window almost never, so
 *  a shorter TTL buys nothing but request volume against four hundred rows. The
 *  admin-facing refresh button exists for the case where it matters. */
export const CATALOG_TTL_MS = 24 * 60 * 60 * 1000

export interface CatalogDeps {
  endpoints: () => Promise<LlmEndpoint[]>
  fetchCatalog: (ep: LlmEndpoint) => Promise<CatalogModel[]>
  read: () => Promise<CatalogStore>
  write: (store: CatalogStore) => Promise<void>
  /** Merged into `model_capabilities` in ONE write, respecting source
   *  precedence. Returns how many facts were actually taken — a catalog claim
   *  that loses to an existing probe result is not a write. */
  record: (batch: Array<{ key: string; facts: Partial<Record<Capability, CapabilityFact>> }>) => Promise<number>
  now: () => number
}

const REAL: CatalogDeps = {
  endpoints: listEndpoints,
  fetchCatalog: catalogModels,
  read: () => getSetting<CatalogStore>(KEY, {}),
  write: (store) => setSetting(KEY, store),
  record: mergeCapabilities,
  now: () => Date.now(),
}

const withDeps = (over?: Partial<CatalogDeps>): CatalogDeps => ({ ...REAL, ...over })

// ── Reading ──────────────────────────────────────────────────────────────────

/** Everything known about one model id, across every endpoint that serves it.
 *
 *  MORE THAN ONE ENTRY IS NORMAL and is why this returns a list rather than a
 *  row: a bare id can land on any endpoint in the pool, and the two can differ
 *  in exactly the way that matters (a quantized local build next to the vendor's
 *  own API). Callers reduce it themselves — `advertisedWindow` takes the
 *  smallest, because a claim has to hold for the worst member. */
export async function catalogEntriesFor(model: string, deps?: Partial<CatalogDeps>): Promise<Array<{ endpoint: string; model: CatalogModel }>> {
  const store = await withDeps(deps).read().catch((): CatalogStore => ({}))
  const out: Array<{ endpoint: string; model: CatalogModel }> = []
  for (const cat of Object.values(store)) {
    const hit = cat.models.find((m) => m.id === model)
    if (hit) out.push({ endpoint: cat.endpoint, model: hit })
  }
  return out
}

/** THE SMALLEST advertised window across the pool, or null when nothing
 *  advertises one. Same rule the probe suite already applied to the endpoint
 *  row; the difference is that this reads the number the provider published
 *  ABOUT THIS MODEL rather than one integer stamped on the endpoint. */
export async function advertisedWindow(model: string, deps?: Partial<CatalogDeps>): Promise<number | null> {
  const windows = (await catalogEntriesFor(model, deps)).map((e) => e.model.contextLength).filter((n): n is number => typeof n === 'number' && n > 0)
  return windows.length ? Math.min(...windows) : null
}

// ── Deriving capabilities ────────────────────────────────────────────────────

/** Which advertised parameter proves which capability. Only entries whose
 *  PRESENCE is a real commitment belong here — see the file header on why
 *  absence proves nothing. */
const PARAM_PROVES: ReadonlyArray<{ param: string; capability: Capability; detail: string }> = [
  { param: 'tools', capability: 'tools', detail: 'the provider catalog advertises tool calling' },
  { param: 'response_format', capability: 'json', detail: 'the provider catalog advertises response_format' },
  { param: 'structured_outputs', capability: 'json-strict', detail: 'the provider catalog advertises structured outputs' },
  { param: 'web_search_options', capability: 'search', detail: 'the provider catalog advertises native web search' },
]

/** WHAT A CATALOG ENTRY PROVES, and nothing more.
 *
 *  Pure, exported, and tested directly, because this function is where the
 *  yes-only rule either holds or quietly stops holding. Every branch below
 *  writes `true` or writes NOTHING. There is no path that writes `false`. */
export function capabilitiesFromCatalog(m: CatalogModel, at: string): Partial<Record<Capability, CapabilityFact>> {
  const facts: Partial<Record<Capability, CapabilityFact>> = {}
  // `source: 'catalog'`, never 'declared'. 'declared' means a HUMAN said so and
  // outranks a provider's own claim — see `SOURCE_RANK` in capability.ts. Writing
  // these as 'declared' would let a nightly refresh overrule an admin.
  const declare = (capability: Capability, detail: string): void => {
    facts[capability] = { value: true, source: 'catalog', at, detail }
  }

  const params = m.supportedParameters
  if (params) {
    for (const rule of PARAM_PROVES) {
      if (params.includes(rule.param)) declare(rule.capability, rule.detail)
    }
    // `tool_choice` without `tools` is not a thing any catalog publishes, but a
    // model that accepts it can certainly be handed tools.
    if (params.includes('tool_choice') && !facts.tools) declare('tools', 'the provider catalog advertises tool_choice')
  }

  // VISION IS THE ONE PLACE ABSENCE IS ALMOST INFORMATIVE and still is not
  // recorded as a no: a catalog that lists `input_modalities: ['text']` is
  // describing the model precisely, but plenty of catalogs list nothing at all,
  // and one shape must not be read as the other. A probe closes it.
  if (m.inputModalities?.includes('image')) declare('vision', 'the provider catalog advertises image input')

  // `long-context` IS DELIBERATELY NOT DECLARED HERE, and the temptation to do
  // it was the whole reason to write this down. `context_length` is the only
  // catalog field that looks like a capability and is not one: it says what the
  // model will ACCEPT, and the probe measures whether the model can still find
  // a fact planted in the middle of that window. Advertising 1M tokens and
  // retrieving from 1M tokens are different claims, and the gap between them is
  // exactly what an admin is trying to find out.
  //
  // The window is not wasted — `advertisedWindow` serves it to the probe, which
  // is what decides how big a haystack to build. That is the honest use of a
  // spec sheet: it sizes the measurement, it does not stand in for it.
  return facts
}

// ── Refreshing ───────────────────────────────────────────────────────────────

export interface RefreshResult {
  endpoint: string
  models: number
  /** Capability facts written across all of this endpoint's models. */
  facts: number
  error: string | null
}

/** Re-fetch one endpoint's catalog, store it, and declare what it proves.
 *
 *  A FAILED FETCH KEEPS THE OLD ENTRY. An endpoint that is briefly unreachable
 *  must not empty the picker or un-declare facts an admin is looking at; the
 *  error is reported and the last good catalog stays. */
export async function refreshEndpointCatalog(ep: LlmEndpoint, deps?: Partial<CatalogDeps>): Promise<RefreshResult> {
  const d = withDeps(deps)
  const at = new Date(d.now()).toISOString()
  let models: CatalogModel[]
  try {
    models = await d.fetchCatalog(ep)
  } catch (err) {
    return { endpoint: ep.name, models: 0, facts: 0, error: err instanceof Error ? err.message : String(err) }
  }

  const store = await d.read().catch((): CatalogStore => ({}))
  await d.write({ ...store, [ep.name]: { endpoint: ep.name, at, models } })

  // Per endpoint:model, exactly as every other capability writer keys it — a
  // fact learned about one endpoint's build is never credited to another's. The
  // whole endpoint goes in ONE merge: four hundred separate read-modify-writes
  // of a row that grows with each one is not a thing to do on a cadence.
  const batch = models
    .map((m) => ({ key: capabilityKey(ep.name, m.id), facts: capabilitiesFromCatalog(m, at) }))
    .filter((e) => Object.keys(e.facts).length > 0)
  const facts = await d.record(batch).catch(() => 0)
  return { endpoint: ep.name, models: models.length, facts, error: null }
}

/** Every endpoint whose catalog is older than the TTL (or absent). Called on a
 *  cadence and by the admin's refresh button; `force` ignores the TTL. */
export async function refreshCatalogs(opts: { force?: boolean } = {}, deps?: Partial<CatalogDeps>): Promise<RefreshResult[]> {
  const d = withDeps(deps)
  const [endpoints, store] = await Promise.all([d.endpoints().catch((): LlmEndpoint[] => []), d.read().catch((): CatalogStore => ({}))])
  const now = d.now()
  const stale = endpoints.filter((ep) => {
    if (opts.force) return true
    const at = Date.parse(store[ep.name]?.at ?? '')
    return !Number.isFinite(at) || now - at > CATALOG_TTL_MS
  })
  // SEQUENTIAL. Four hundred rows per provider is a big response and several
  // providers rate-limit the catalog endpoint harder than they rate-limit
  // completions; a fan-out here buys nothing an admin can perceive.
  const out: RefreshResult[] = []
  for (const ep of stale) out.push(await refreshEndpointCatalog(ep, deps))
  return out
}
