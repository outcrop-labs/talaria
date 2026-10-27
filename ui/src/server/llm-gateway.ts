// The org's model catalog and routing, as this process reads them: which
// model ids the gateway offers (`gatewayModels`), and where an id can land
// (`routingFor`, `resolveRoute`). The harness runner reads these to resolve a
// model chain and to find the capability keys its floor is judged by.
//
// THIS IS NOT A GATEWAY ANY MORE. It used to be one — a second copy of the
// Rust relay inside this process: it decrypted provider keys, built and sent
// the upstream request, learned parameter rejections, checked a budget and
// wrote the ledger row, for every model call an app's harness made. That copy
// recorded no provider-reported cost, left a reply without usage unmetered,
// trusted the caller name the app passed and checked the budget on one call
// path of three. Every call now goes to the relay on the app's own gateway
// key (harness/transport.ts), and the relay does all of that once, for every
// caller alike. What stays is read-only: the catalog and the routing rule,
// which the runner needs before it makes a call.
import { listEndpoints, type LlmEndpoint } from './agent-defs'

/** THE STRUCTURED-OUTPUT SLOT, as the wire wants it.
 *
 *  `json_schema` WHEREVER WE HAVE A SCHEMA, and every JSON harness has one. The
 *  loose `{ type: 'json_object' }` this used to be hardcoded to is not merely
 *  weaker — Anthropic's OpenAI-compatible layer REJECTS it outright
 *  (`response_format.type: Input should be 'json_schema'`), so every structured
 *  call to a Claude model 400'd and the fitness suite scored it as the model
 *  failing to hold a contract. It survives only as the fallback for an endpoint
 *  whose schema we could not render. */
export type ResponseFormat =
  | { type: 'json_object' }
  | { type: 'json_schema'; json_schema: { name: string; schema: Record<string, unknown>; strict: boolean } }

export interface GatewayModel {
  id: string
  endpoints: string[]
  /** True for "<endpoint>/<model>" pins. Bare model ids may themselves contain
   *  "/" (OpenRouter, HF-style names) — only this flag tells the two apart. */
  qualified: boolean
}

/** EVERY MODEL, SPELLED ONE WAY: `<endpoint>/<model>`.
 *
 *  WHY THIS IS NOT "bare + qualified for everything", which it was. This list
 *  is what every picker and the fitness matrix draw from, and emitting both
 *  spellings of each model put `claude-opus-5` and `anthropic/claude-opus-5` on
 *  consecutive rows — two entries for one deployment, routing to the same
 *  endpoint under the same capability key, differing only in how they are
 *  written. On a two-endpoint install that is 26 rows for 13 models, and an
 *  admin comparing them has no way to tell that the pair is one thing.
 *
 *  THE QUALIFIED FORM IS THE CANONICAL ONE because it is the one that names
 *  where the model runs — which is the thing Talaria actually measures
 *  capability about (`capabilityKey` is `endpoint:model`, never a bare name).
 *  It also reads correctly for both kinds of provider without a special case:
 *  a router endpoint gives `openrouter/deepseek/deepseek-v4-flash`
 *  (provider/brand/model) and a direct vendor gives `anthropic/claude-opus-5`
 *  (brand/model), because the endpoint name IS the provider in one and the
 *  brand in the other.
 *
 *  A BARE ID SURVIVES ONLY WHERE IT MEANS SOMETHING ELSE: served by more than
 *  one endpoint, it is the round-robin POOL, a distinct routing target that no
 *  single qualified id can express. `ModelRow.pooled` is what the UI labels it
 *  with. One endpoint, and the bare name is not a second target — it is a
 *  second name for the first.
 *
 *  BARE IDS STAY CALLABLE regardless: `routingFor` and `resolveRoute` are
 *  untouched, so every stored assignment, agent config and API caller that
 *  spells a model bare keeps working. This function answers "what should we
 *  OFFER", not "what will we accept". */
export async function gatewayModels(): Promise<GatewayModel[]> {
  const eps = await listEndpoints()
  const bare = new Map<string, string[]>()
  const out: GatewayModel[] = []
  for (const ep of eps) {
    for (const m of ep.models) {
      out.push({ id: `${ep.name}/${m}`, endpoints: [ep.name], qualified: true })
      bare.set(m, [...(bare.get(m) ?? []), ep.name])
    }
  }
  for (const [id, endpoints] of bare) if (endpoints.length > 1) out.push({ id, endpoints, qualified: false })
  return out.sort((a, b) => a.id.localeCompare(b.id))
}

/** THE CANONICAL SPELLING of a model id, against a catalog.
 *
 *  Everything Talaria has already stored — role assignments, platform-agent
 *  pins, an archived fitness report, a member allowlist — was written when both
 *  spellings were offered, so plenty of it is bare. Those values keep routing;
 *  what they must not do is fail to LINE UP with the canonical row, or an admin
 *  sees a role assigned to a model that appears nowhere in the list and a paid
 *  run whose verdict lights up no cell.
 *
 *  So this maps a stored spelling onto the offered one instead of rewriting the
 *  database: cheaper than a migration, correct for values written after it, and
 *  it cannot corrupt anything if the catalog is momentarily wrong. Unresolvable
 *  ids come back unchanged — an id nothing serves is a fact to show, not one to
 *  guess at. */
export function canonicalModelId(id: string, catalog: readonly GatewayModel[]): string {
  if (catalog.some((m) => m.id === id)) return id
  const pins = catalog.filter((m) => m.qualified && m.id.endsWith(`/${id}`))
  // Exactly one, or it is genuinely ambiguous — two endpoints serving the same
  // model is the pooled case, and picking one of them would silently reassign
  // a role to half of what it had.
  return pins.length === 1 ? pins[0]!.id : id
}

// Round-robin cursor per bare model name (module-level; resets on reload).
const rr = new Map<string, number>()

export interface ResolvedRoute {
  endpoint: LlmEndpoint
  /** The model id the upstream expects. */
  upstreamModel: string
}

export interface ModelRouting {
  /** Every endpoint this model id can land on — one for a pin, the whole
   *  round-robin pool for a bare name, empty when nothing serves it. */
  endpoints: LlmEndpoint[]
  /** The model id the upstream expects (a pin drops the endpoint prefix). */
  upstreamModel: string
}

/** Where a model id CAN go, without picking (and without advancing the
 *  round-robin cursor) — the ledger asks this after the fact, so it must not
 *  perturb live routing. */
export async function routingFor(model: string): Promise<ModelRouting> {
  const eps = await listEndpoints()
  // Endpoint-qualified: "<endpoint>/<rest>" (rest may itself contain "/").
  const slash = model.indexOf('/')
  if (slash > 0) {
    const ep = eps.find((e) => e.name === model.slice(0, slash))
    const rest = model.slice(slash + 1)
    if (ep && ep.models.includes(rest)) return { endpoints: [ep], upstreamModel: rest }
  }
  return { endpoints: eps.filter((e) => e.models.includes(model)), upstreamModel: model }
}

/** Resolve a requested model id to an endpoint + upstream model. */
export async function resolveRoute(model: string): Promise<ResolvedRoute | null> {
  const { endpoints, upstreamModel } = await routingFor(model)
  if (endpoints.length === 0) return null
  const i = (rr.get(model) ?? 0) % endpoints.length
  rr.set(model, i + 1)
  return { endpoint: endpoints[i]!, upstreamModel }
}
