// Which reasoning-effort levels a model id may be asked for.
//
// THE ONE QUESTION THIS FILE ANSWERS, asked by two surfaces and enforced by two
// routes: "may the composer offer an effort picker for THIS id, and which
// levels may it list?" Two voices can vouch, and only these two: the per-model
// metadata the catalog refresh already extracts and stores (`model-catalog.ts`,
// filled when an admin adds models on /models) — the provider's own
// `supported_efforts` — and an admin's declaration (`llm_endpoints.model_efforts`,
// edited on the endpoint modal) for providers whose catalog says nothing. A
// model neither voice has published levels for answers `[]`, and `[]` means
// the picker does not render and the request body carries no effort, which is
// the whole feature contract: effort is offered only where somebody who can
// know — the provider, or the endpoint's own operator — vouches for it.
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
import { catalogEntriesFor, catalogEntriesForTargets, effortLevelsOf, type CatalogModel, type CatalogStore } from './model-catalog'
import { personaTargetsFor } from './harness/persona'

export interface EffortDeps {
  /** The stored catalog, same row `model-catalog.ts` reads. */
  read: () => Promise<CatalogStore>
  /** The persona index edge — injectable so a test needs no database. */
  personaTargets: (model: string) => Promise<ModelTarget[]>
  /** The endpoint roster — for the admin-declared ladders (`modelEfforts`)
   *  that stand in where a provider's catalog is silent. */
  endpoints: () => Promise<LlmEndpoint[]>
}

const REAL: EffortDeps = {
  read: () => getSetting<CatalogStore>('model_catalog', {}),
  personaTargets: personaTargetsFor,
  endpoints: listEndpoints,
}

/** THE SECOND VOICE THAT CAN VOUCH. The feature shipped with one: the
 *  provider's catalog. That made the picker structurally unreachable for every
 *  minimal OpenAI-compatible self-host — vLLM, Ollama, a hand-rolled gateway
 *  answering `/models` with `{id}` and nothing else — no matter what the
 *  weights behind it accept, which is how an endpoint its own operator KNOWS
 *  takes effort levels showed no picker at all. An admin's declaration
 *  (`llm_endpoints.model_efforts`, edited on the endpoint's modal) fills that
 *  silence: it REPLACES the catalog's ladder for that endpoint's build of the
 *  model — a human's word outranks a provider's, the same standing a declared
 *  capability fact has over a catalog one — and never merges with it, because
 *  a union would offer levels one of the two voices never vouched for.
 *
 *  The POOL RULE is untouched: a declaration speaks for its endpoint's member
 *  of the pool only, and `effortLevelsOf` still intersects across members, so
 *  a level is offered only where every member that speaks at all accepts it. */
function withDeclaredEfforts(
  entries: ReadonlyArray<{ endpoint: string; model: CatalogModel }>,
  roster: ReadonlyArray<LlmEndpoint>,
): Array<{ endpoint: string; model: CatalogModel }> {
  return entries.map(({ endpoint, model }) => {
    // Defensive on purpose: the column is admin-typed JSON that outlives the
    // build that wrote it, and a malformed entry must degrade to the catalog's
    // answer rather than crash a chat turn's validation.
    const declared = roster.find((e) => e.name === endpoint)?.modelEfforts?.[model.id]
    if (!Array.isArray(declared) || declared.length === 0 || !declared.every((l) => typeof l === 'string' && l.length > 0)) {
      return { endpoint, model }
    }
    return { endpoint, model: { ...model, efforts: declared } }
  })
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
  if (entries.length === 0) return []
  const roster = await d.endpoints().catch((): LlmEndpoint[] => [])
  return effortLevelsOf(withDeclaredEfforts(entries, roster))
}

// THE BACKFILL IS THE API'S. A catalog stored before the effort field existed
// answers `[]` for every model until it is fetched again, and fetching is a
// call to the provider on its key — so it happens in the api, whose
// `/api/models/efforts` refreshes such a catalog once (talaria-model-efforts
// `ensure_efforts_catalog`) when the composer asks. This process reads what
// that left in the store.
