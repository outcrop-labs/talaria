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
import { catalogEntriesFor, catalogEntriesForTargets, effortLevelsOf, type CatalogStore } from './model-catalog'
import { personaTargetsFor } from './harness/persona'
import type { ModelTarget } from './agent-defs'

export interface EffortDeps {
  /** The stored catalog, same row `model-catalog.ts` reads. */
  read: () => Promise<CatalogStore>
  /** The persona index edge — injectable so a test needs no database. */
  personaTargets: (model: string) => Promise<ModelTarget[]>
}

const REAL: EffortDeps = {
  read: () => getSetting<CatalogStore>('model_catalog', {}),
  personaTargets: personaTargetsFor,
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
