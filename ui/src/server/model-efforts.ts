// Which reasoning-effort levels a model id may be asked for.
//
// THE ONE QUESTION THIS FILE ANSWERS, asked by two surfaces and enforced by two
// routes: "may the composer offer an effort picker for THIS id, and which
// levels may it list?" The answer comes from the per-model metadata the catalog
// refresh already extracts and stores (`model-catalog.ts`, filled when an admin
// adds models on /models) — the provider's own `supported_efforts` — and from
// nowhere else. A model nobody has published levels for answers `[]`, and `[]`
// means the picker does not render and the request body carries no effort,
// which is the whole feature contract: effort is offered only where the model
// metadata vouches for it.
//
// TWO SPELLINGS OF A MODEL ID, both real, both arriving here:
//   a catalog id ("openrouter/deepseek/…", bare or endpoint-qualified) — the
//     gateway pools it across every endpoint that serves it;
//   a FLEET PERSONA id ("dex-ops", "dex-ops-opus" for a tier) — not a catalog
//     id at all, but a pointer at the endpoint:upstream targets its agent
//     config names (main first, fallbacks behind it — see `harness/persona.ts`
//     for why the fallbacks belong in the pool). Resolved here, once, so no
//     caller re-derives it and the two spellings cannot drift apart.
import { getSetting } from './audit'
import { listEndpoints, type LlmEndpoint, type ModelTarget } from './agent-defs'
import {
  catalogEntriesFor,
  catalogEntriesForTargets,
  effortLevelsOf,
  refreshEndpointCatalog,
  type CatalogStore,
} from './model-catalog'
import { personaTargetsFor } from './harness/persona'

export interface EffortDeps {
  /** The stored catalog, same row `model-catalog.ts` reads. */
  read: () => Promise<CatalogStore>
  /** The persona index edge — injectable so a test needs no database. */
  personaTargets: (model: string) => Promise<ModelTarget[]>
  /** The endpoint roster, for the backfill's endpoint resolution. */
  endpoints: () => Promise<LlmEndpoint[]>
  /** Refresh one endpoint's catalog (live provider fetch + store). Injectable
   *  so a test needs no network. */
  refreshEndpoint: (ep: LlmEndpoint) => Promise<unknown>
}

const REAL: EffortDeps = {
  read: () => getSetting<CatalogStore>('model_catalog', {}),
  personaTargets: personaTargetsFor,
  endpoints: listEndpoints,
  refreshEndpoint: refreshEndpointCatalog,
}

/** The effort levels THIS model id supports, or `[]` when nothing vouches for
 *  any. Never throws: an unreadable catalog answers the same `[]` a fresh
 *  self-host does, and a chat turn must not fail because a picker could not
 *  decide whether to appear. */
export async function effortsForModel(model: string, deps?: Partial<EffortDeps>): Promise<string[]> {
  const d = { ...REAL, ...deps }
  // A persona resolves to explicit targets; everything else is a catalog id.
  // `personaTargetsFor` answers [] for ids it does not know, which is also the
  // cached-cheap path for the gateway spelling.
  const targets = await d.personaTargets(model).catch((): ModelTarget[] => [])
  const entries = targets.length
    ? await catalogEntriesForTargets(targets, { read: d.read })
    : await catalogEntriesFor(model, { read: d.read })
  return effortLevelsOf(entries)
}

// ── The backfill ─────────────────────────────────────────────────────────────
//
// THE STALE-CATALOG PROBLEM, which is why a picker shipped and did not appear:
// the ONLY production writer of the stored catalog is the model-adder modal
// (`fleet.endpoints.$id.available`), so a deployment's catalog is only ever as
// new as the last time an admin opened it. A catalog written by a build before
// the effort extraction has no `efforts` field on any model — every question
// answers `[]`, the picker never renders, and nothing an admin short of
// re-opening the model modal heals it.
//
// `ensureEffortsCatalog` is that healing: for the endpoints serving the asked
// model, any stored catalog whose models predate the field is refreshed live,
// ONCE — a refresh written by this build stamps `efforts` (null or a list) on
// every model, so the pre-feature shape never re-triggers. A failed refresh
// (provider unreachable) retries no more than every five minutes per endpoint.

/** Endpoints whose stored catalog predates the effort extraction: models
 *  present, none carrying the `efforts` key. A refresh written by the current
 *  build always writes the key, so this is exactly the set worth re-fetching. */
function preEffortsEntry(store: CatalogStore, endpoint: string): boolean {
  const entry = store[endpoint]
  if (!entry || entry.models.length === 0) return false
  return entry.models.every((m) => (m as { efforts?: string[] | null }).efforts === undefined)
}

/** One refresh attempt per endpoint per window; a provider that is down must
 *  not turn every picker question into a 10-second timeout. */
const RETRY_MS = 5 * 60_000
const attemptedAt = new Map<string, number>()
/** Refreshes in flight, so two surfaces asking about the same model share one
 *  live fetch rather than racing two. */
const inflight = new Map<string, Promise<unknown>>()

/** Drop the backfill's bookkeeping. For tests, which share one module instance
 *  across cases — the same reason `persona.ts` exports `clearPersonaCache`. */
export function resetEffortsBackfill(): void {
  attemptedAt.clear()
  inflight.clear()
}

/** Refresh the pre-efforts catalogs behind this model and answer with the
 *  levels the refreshed store now vouches for. Safe to call on every empty
 *  read: post-feature entries (efforts present, even null) and unknown models
 *  fetch nothing, so the live call happens once per endpoint per install (or
 *  per five minutes while its provider is unreachable). */
export async function ensureEffortsCatalog(model: string, deps?: Partial<EffortDeps>): Promise<string[]> {
  const d = { ...REAL, ...deps }
  try {
    const targets = await d.personaTargets(model).catch((): ModelTarget[] => [])
    const store = await d.read().catch((): CatalogStore => ({}))
    // The endpoints that could serve this id: a persona's own targets, or the
    // pool a catalog id lands on. Same resolution rule as the read above.
    const names = targets.length
      ? [...new Set(targets.map((t) => t.endpoint))]
      : [...new Set((await catalogEntriesFor(model, { read: async () => store })).map((e) => e.endpoint))]
    const stale = names.filter((name) => preEffortsEntry(store, name) && Date.now() - (attemptedAt.get(name) ?? 0) >= RETRY_MS)
    if (stale.length > 0) {
      const roster = await d.endpoints().catch((): LlmEndpoint[] => [])
      for (const name of stale) {
        const ep = roster.find((e) => e.name === name)
        if (!ep) continue
        attemptedAt.set(name, Date.now())
        let pending = inflight.get(name)
        if (!pending) {
          pending = d.refreshEndpoint(ep).finally(() => inflight.delete(name))
          inflight.set(name, pending)
        }
        await pending.catch(() => undefined)
      }
    }
  } catch {
    // The backfill is best-effort by construction: a broken edge answers the
    // stored [] and the throttle decides when to try again.
  }
  return effortsForModel(model, deps)
}
