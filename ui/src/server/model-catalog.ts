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
// It is a CACHE, not a source of truth, and THIS PROCESS ONLY READS IT. The
// api fetches it — talaria-gateway's `catalog_models` asks the provider, with
// the provider's key, which never leaves the api — and stores it
// (talaria-model-catalog), when an admin opens an endpoint's model picker
// (`GET /api/fleet/endpoints/{id}/available`) and when the effort backfill
// finds a catalog older than the effort field (`GET /api/models/efforts`).
// That is also where a catalog entry is read for what it proves
// (`capabilities_from_catalog`). This module used to hold a second copy of
// the fetch, which opened the endpoint's sealed key here to make it; nothing
// called that copy once the api owned the picker, and an app host holds no
// provider key. What stays is the read the harness runner's effort slot
// needs (model-efforts.ts).
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
import { getSetting } from './audit'

/** ONE MODEL AS THE PROVIDER DESCRIBES IT — the fields every OpenAI-compatible
 *  catalog that publishes them agrees on, normalized: talaria-gateway's
 *  `CatalogModel`, as the api stores it in `model_catalog` (its `to_json`).
 *
 *  EVERY FIELD IS OPTIONAL AND MEANS "THE PROVIDER DID NOT SAY". Never "no":
 *  see the rule above. */
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
  /** The reasoning-effort levels this model accepts (`reasoning_effort`),
   *  exactly as the provider spells them. Null when unpublished: a model with
   *  no list is a model nobody has vouched for, not a model that was refused.
   *  The picker only appears for a non-empty list. */
  efforts: string[] | null
  /** USD per MILLION tokens, to match how everything else in Talaria quotes a
   *  price. */
  pricing: { inPerMTok: number | null; outPerMTok: number | null } | null
}

/** One endpoint's catalog, as of `at`. Keyed by endpoint NAME because that is
 *  what `capabilityKey` and the gateway's routing both speak. */
export interface EndpointCatalog {
  endpoint: string
  at: string
  models: CatalogModel[]
}

export type CatalogStore = Record<string, EndpointCatalog>

const KEY = 'model_catalog'

export interface CatalogDeps {
  read: () => Promise<CatalogStore>
}

const REAL: CatalogDeps = {
  read: () => getSetting<CatalogStore>(KEY, {}),
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
    // BOTH SPELLINGS, and the endpoint-qualified one is why this is here at all.
    // A catalog is keyed by the id the PROVIDER publishes — OpenRouter says
    // `deepseek/deepseek-v4-flash` — while everything that asks about a model
    // now says `openrouter/deepseek/deepseek-v4-flash`, because that is the one
    // spelling the picker offers. Matching only the raw id silently missed every
    // qualified caller: `advertisedWindow` answered null for a model publishing
    // a 1,048,576-token window, and the long-context probe reported that it
    // advertises none.
    //
    // The prefix is stripped only for the endpoint whose name it is, so a bare
    // id still searches every catalog (the pooled case) and a qualified id can
    // never match another endpoint's entry.
    const prefix = `${cat.endpoint}/`
    const upstream = model.startsWith(prefix) ? model.slice(prefix.length) : model
    const hit = cat.models.find((m) => m.id === upstream)
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

/** Catalog entries for EXPLICIT endpoint:upstream pairs — the persona path.
 *
 *  A fleet persona id is not a catalog id, so it cannot be looked up the way
 *  `catalogEntriesFor` looks one up. Its agent config names the exact targets
 *  (`harness/persona.ts`), and this answers the catalog rows for those targets
 *  and nothing else: the endpoint prefix-stripping in `catalogEntriesFor`
 *  would be guessing at what the config already knows. */
export async function catalogEntriesForTargets(
  targets: ReadonlyArray<{ endpoint: string; model: string }>,
  deps?: Partial<CatalogDeps>,
): Promise<Array<{ endpoint: string; model: CatalogModel }>> {
  const store = await withDeps(deps).read().catch((): CatalogStore => ({}))
  const out: Array<{ endpoint: string; model: CatalogModel }> = []
  for (const t of targets) {
    const hit = store[t.endpoint]?.models.find((m) => m.id === t.model)
    if (hit) out.push({ endpoint: t.endpoint, model: hit })
  }
  return out
}

// ── Reasoning effort ─────────────────────────────────────────────────────────

/** The effort ladder's display order, weakest to strongest. Levels a provider
 *  coins that this list does not know still show — appended after the known
 *  ones, in the provider's own order — because the provider's spelling is the
 *  contract and the picker must never rename a level into one the model
 *  rejects. */
export const EFFORT_ORDER: readonly string[] = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

const orderedEfforts = (levels: Iterable<string>): string[] => {
  const set = new Set(levels)
  return [...EFFORT_ORDER.filter((l) => set.has(l)), ...[...set].filter((l) => !EFFORT_ORDER.includes(l))]
}

/** THE EFFORT LEVELS A POOL AGREES ON.
 *
 *  Same posture as `advertisedWindow`'s minimum: a bare id (and a persona
 *  behind fallbacks) can land on any member of the pool, so a level is only
 *  offered when every member that speaks at all accepts it. Members that say
 *  nothing do not veto — UNKNOWN IS NOT FALSE — they are skipped, exactly as
 *  an endpoint without a published window does not shrink the minimum. An
 *  empty answer means "nobody vouched for any level" and the picker stays
 *  hidden, which is the honest rendering of an unmeasured model. */
export function effortLevelsOf(entries: ReadonlyArray<{ endpoint: string; model: CatalogModel }>): string[] {
  const lists = entries.map((e) => e.model.efforts).filter((l): l is string[] => Array.isArray(l) && l.length > 0)
  if (lists.length === 0) return []
  const [first, ...rest] = lists
  const shared = rest.reduce((acc, l) => new Set([...acc].filter((x) => l.includes(x))), new Set(first))
  return orderedEfforts(shared)
}

/** The effort levels a CATALOG model id may be asked for, across every
 *  endpoint that serves it. Persona ids resolve one level up, in
 *  `model-efforts.ts` — this is the catalog-only half. */
export async function effortsFor(model: string, deps?: Partial<CatalogDeps>): Promise<string[]> {
  return effortLevelsOf(await catalogEntriesFor(model, deps))
}
