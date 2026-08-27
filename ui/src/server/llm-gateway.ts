// The Talaria LLM gateway: ONE OpenAI-compatible endpoint over the org's whole
// model stack. Any model registered on /models is callable as
//   <model>              (round-robins endpoints that serve it — e.g. pl-main
//                         across both Spark boxes)
//   <endpoint>/<model>   (pin a specific backend)
// Talaria holds the provider keys (agents/tools never see them), injects each
// endpoint's request_defaults (e.g. OpenRouter's US no-train provider
// allowlist), streams the reply through, and meters every call into the
// ledger with the calling key's identity. This is the litellm replacement,
// engineered into Talaria itself.
import { newVault, sealContent, type SecretVault } from './secret-vault'
import { db } from './db/pg'
import { getSetting, setSetting } from './audit'
import { listEndpoints, type LlmEndpoint } from './agent-defs'
import { NATIVE_BASE, openrouterUsPool, resolveEndpointKey } from './provider-catalog'
import { guardCompletion } from './guardrails'
import { capabilityKey, recordCapability } from './harness/capability'
import {
  activeLearnedParams,
  classifyParam,
  CONTRACT_PARAMS,
  isContractParam,
  readLearnedParams,
  rejectedParam,
  writeLearnedParams,
  type ContractDrop,
  type LearnedParamMap,
} from './harness/gateway-params'

export type { ContractDrop } from './harness/gateway-params'

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

const deepMerge = (base: Record<string, unknown>, extra: Record<string, unknown>): Record<string, unknown> => {
  const out = { ...base }
  for (const [k, v] of Object.entries(extra)) {
    const prev = out[k]
    out[k] =
      v && typeof v === 'object' && !Array.isArray(v) && prev && typeof prev === 'object' && !Array.isArray(prev)
        ? deepMerge(prev as Record<string, unknown>, v as Record<string, unknown>)
        : v
  }
  return out
}

export interface UpstreamCall {
  url: string
  headers: Record<string, string>
  body: Record<string, unknown>
  /** THE SUBSTITUTIONS MADE ON THE WAY OUT — see `secret-vault.ts`. Carried on
   *  the call so a caller that needs to put a real value back at a genuine
   *  boundary (a tool invocation, a credential helper) can, and so the audit
   *  line can say WHAT KIND of credential was sealed without ever holding one.
   *
   *  Optional for the same reason `contractDrops` is: a structurally-typed
   *  `{ url, headers, body }` still compiles. `buildUpstream` always sets it. */
  vault?: SecretVault
  /** Contract-bearing parameters that did NOT reach the model (audit 1.2) —
   *  empty on a clean call, which is the overwhelming majority.
   *
   *  Optional so that a caller holding a structurally-typed `{ url, headers,
   *  body }` still compiles; `buildUpstream` always sets it, and `fetchUpstream`
   *  MUTATES the array in place when a live 400 forces a drop mid-call. Mutation
   *  rather than a wrapped return type is the whole trick here: `fetchUpstream`
   *  hands back a `Response` that four call sites already stream, relay and
   *  meter, and none of them had to change to gain the signal. Read it AFTER the
   *  fetch resolves. */
  contractDrops?: ContractDrop[]
  /** THE CALLER'S CANCELLATION, honored all the way down to the socket.
   *
   *  WHY THIS EXISTS, and it is a bug report rather than a feature. `fetchUpstream`
   *  used to hardcode `AbortSignal.timeout(600_000)` and accept no signal at all,
   *  so a caller that gave up — a fitness case that blew its budget, a request
   *  whose client disconnected — left the HTTP request running for up to ten
   *  minutes with nobody waiting for it. The eval sweep aborts an
   *  `AbortController` on every case timeout and nothing downstream was listening
   *  to it.
   *
   *  That turns one slow reply into a cascade rather than one failure: the sweep
   *  moves on and issues the next call while the abandoned one still holds its
   *  socket, up to eight candidate sweeps do it at once, and the provider starts
   *  queueing — so calls that would have been fast now sit behind the abandoned
   *  ones and blow their budgets too. The symptom is timeouts on a model that is
   *  demonstrably fine, which is exactly the shape of the report that found it.
   *
   *  Pass it. A caller that cannot be cancelled is a caller that can only be
   *  waited out. */
  signal?: AbortSignal
  /** Hard ceiling for this one call, defaulting to `UPSTREAM_TIMEOUT_MS`. Set it
   *  from the caller's own budget so the socket dies WITH the caller rather than
   *  ten minutes after it. */
  timeoutMs?: number
}

/** Contract parameters this call lost, for a caller that would rather not think
 *  about the optionality above. Empty means the model saw exactly what you
 *  asked for. */
export const contractDropsOf = (call: UpstreamCall): ContractDrop[] => call.contractDrops ?? []

/** What this call sealed on the way out — kinds only, never values. */
export const sealedSecretsOf = (call: UpstreamCall): ReadonlyArray<{ handle: string; label: string }> => call.vault?.sealed ?? []

/** Build the outbound request: provider base/key + request_defaults merged
 *  UNDER the client body (client wins), model swapped to the upstream id. */
export async function buildUpstream(route: ResolvedRoute, clientBody: Record<string, unknown>): Promise<UpstreamCall> {
  const ep = route.endpoint
  const base = (ep.baseUrl ?? NATIVE_BASE[ep.provider])?.replace(/\/$/, '')
  if (!base) throw new Error(`endpoint "${ep.name}" has no base URL`)
  const key = await resolveEndpointKey(ep)

  const headers: Record<string, string> = { 'content-type': 'application/json' }
  if (key) {
    headers['authorization'] = `Bearer ${key}`
    // Anthropic's OpenAI-compat layer accepts x-api-key; harmless elsewhere.
    if (ep.provider === 'anthropic') headers['x-api-key'] = key
  }

  let defaults = ((ep as unknown as { requestDefaults?: Record<string, unknown> }).requestDefaults ?? {}) as Record<
    string,
    unknown
  >
  // No-train routing on an OpenRouter endpoint: the US pool is fetched LIVE
  // from OpenRouter's provider catalog on every call (briefly cached), never
  // maintained by hand — a stored `only` list is only the offline fallback.
  const prov = defaults.provider as { data_collection?: string; only?: string[] } | undefined
  if (ep.provider === 'openrouter' && prov?.data_collection === 'deny') {
    const pool = await openrouterUsPool()
    if (pool) defaults = { ...defaults, provider: { ...prov, only: pool } }
  }
  const body: Record<string, unknown> = { ...deepMerge(defaults, clientBody), model: route.upstreamModel }
  if (body.stream) body.stream_options = { include_usage: true, ...(body.stream_options as object | undefined) }

  // ── CREDENTIALS DO NOT LEAVE THIS PROCESS ──────────────────────────────────
  //
  // THE LAST POINT AT WHICH WE STILL OWN THE BYTES. Every gateway call in the
  // tree is assembled here — the blocking transport, the streamed one, the
  // tool turn, the image turn, `completeViaGateway` — which is the same reason
  // the learned-parameter ratchet lives here. One chokepoint, so a new caller
  // cannot forget.
  //
  // WHY THE GUARD IS NOT ENOUGH, stated once where the substitution happens:
  // `secret_leak` is an OUTPUT check. By the time it fires the model has read
  // the credential, and the provider has logged the prompt on its own
  // infrastructure. Redaction cleans what we KEEP; it cannot clean what we SENT.
  // The adversarial tier is the measurement behind that — the four strongest
  // models on this install each file `secret_leak` on two of four seeds, after
  // grounding — and a business asking whether its keys are safe here deserves an
  // answer that does not turn on a model's judgement.
  //
  // So what travels is a handle. See `secret-vault.ts` for why it is opaque and
  // why the vault is per-request and stored nowhere.
  const vault = newVault()
  if (Array.isArray(body.messages)) {
    // sealContent, not a string-only map: an image turn's content is an array
    // of parts, and its text part is as credential-prone as any prose turn.
    body.messages = (body.messages as Array<Record<string, unknown>>).map((m) => ({ ...m, content: sealContent(m.content, vault) }))
  }

  // Pre-strip parameters this endpoint+model has already rejected (learned live
  // from the 400s below — dynamic specs with no maintained tables). A drop of a
  // CONTRACT parameter is reported here too, not only on the call that first
  // learned it: after the first 400 this is the ONLY path that removes it, so a
  // caller that trusted the pre-strip path would go right back to handing prose
  // to a JSON parser for the next thirty days.
  await loadLearnedParams()
  const learned = capabilityKey(ep.name, route.upstreamModel)
  const { params, expired } = activeLearnedParams(learnedParams, learned, Date.now())
  if (expired) persistLearnedParams()
  const contractDrops: ContractDrop[] = []
  for (const p of params) {
    if (!(p in body)) continue // never sent it — nothing was dropped
    delete body[p]
    if (!isContractParam(p)) continue
    const drop: ContractDrop = {
      param: p,
      capability: CONTRACT_PARAMS[p],
      endpoint: ep.name,
      model: route.upstreamModel,
      source: 'remembered',
    }
    contractDrops.push(drop)
    warnContractDrop(drop)
  }
  return { url: `${base}/chat/completions`, headers, body, contractDrops, vault }
}

// Parameter support, learned from the source: when an upstream 400 names a
// parameter we sent ("`temperature` is deprecated", "Unsupported parameter:
// 'top_p'"), we strip it, retry, and remember — the next call pre-strips.
// Newer models routinely retire tunables (sonnet-5 rejects temperature); no
// spec table could keep up, but the provider itself is always current.
// Learnings persist in app_settings so a restart doesn't re-pay the 400s.
//
// The classification, the 30-day TTL and the stored shape live in
// harness/gateway-params.ts, where they can be tested without a database. What
// stays here is the I/O: when to read, when to write, and what to shout about.
const SETTINGS_KEY = 'gateway_unsupported_params'
const learnedParams: LearnedParamMap = new Map()
let learnedLoaded: Promise<void> | null = null
const loadLearnedParams = (): Promise<void> =>
  (learnedLoaded ??= (async () => {
    const { byKey, changed } = readLearnedParams(await getSetting<unknown>(SETTINGS_KEY, {}), Date.now())
    for (const [key, params] of byKey) {
      // A 400 can land between the first call and this load resolving. The
      // in-memory stamp is by definition the newer one, so it wins.
      const into = learnedParams.get(key) ?? new Map<string, number>()
      for (const [param, at] of params) if (!into.has(param)) into.set(param, at)
      learnedParams.set(key, into)
    }
    // The read normalized a legacy or stale entry: write it back once so the
    // TTL clock actually starts ticking rather than restarting every boot.
    if (changed) persistLearnedParams()
  })().catch(() => {}))

const persistLearnedParams = (): void => {
  void setSetting(SETTINGS_KEY, writeLearnedParams(learnedParams, Date.now())).catch(() => {})
}

/** Clear learned parameter strips — one endpoint:model, or all of them.
 *
 *  The release valve on the ratchet, for an admin who has just fixed a provider
 *  or re-pointed a model id at different weights and does not want to wait out
 *  the TTL. Awaits its write, so the route that will call it can report whether
 *  the reset landed.
 *
 *  This forgets what we stopped SENDING. The matching "what that told us about
 *  the model" lives in `forgetCapabilities` (harness/capability.ts) under the
 *  same key — an admin reset should call both, or the UI will keep reporting a
 *  model as incapable of JSON while the gateway has cheerfully resumed asking
 *  for it. */
export async function forgetLearnedParams(key?: string): Promise<void> {
  // Load first: an unresolved lazy load would otherwise merge the very entries
  // we just deleted back in, and the strip would come back from the dead.
  await loadLearnedParams()
  if (key === undefined) learnedParams.clear()
  else learnedParams.delete(key)
  warnedContractDrops.clear()
  await setSetting(SETTINGS_KEY, writeLearnedParams(learnedParams, Date.now()))
}

// One line per endpoint:model:param per process. A contract drop is a standing
// condition, not an event — it recurs on every single call for as long as the
// learning lives, and a log line per call would bury the one that matters.
const warnedContractDrops = new Set<string>()

/** Say it out loud, once: this reply is not the reply that was asked for.
 *  Silence here is the failure mode of audit 1.2 — the call succeeds, the
 *  response parses as a chat completion, and only the shape of the CONTENT is
 *  wrong, which is the one thing no HTTP status can tell you. */
function warnContractDrop(drop: ContractDrop): void {
  const id = `${capabilityKey(drop.endpoint, drop.model)}:${drop.param}`
  if (warnedContractDrops.has(id)) return
  warnedContractDrops.add(id)
  console.warn(
    `[gateway] ${drop.endpoint} rejected "${drop.param}" for model ${drop.model} — requests are being sent WITHOUT it, ` +
      `so replies are no longer constrained (capability ${CONTRACT_PARAMS[drop.param]} recorded false). ` +
      `Callers expecting structured output must repair or fall back. Clear with forgetLearnedParams("${capabilityKey(drop.endpoint, drop.model)}").`,
  )
}

/** POST to the upstream with two adaptations: the dev-mode hostname fallback
 *  (docker-internal bare names don't resolve from the host → retry localhost),
 *  and parameter-rejection recovery (a 400 naming a parameter we sent strips
 *  it and retries, remembering per endpoint:model).
 *
 *  If what got stripped was CONTRACT-bearing, `call.contractDrops` says so by
 *  the time this resolves — read it (via `contractDropsOf`) before trusting a
 *  200 to be the shape you asked for. */
export async function fetchUpstream(call: UpstreamCall, route?: ResolvedRoute): Promise<Response> {
  const started = Date.now()
  try {
    const res = await fetchUpstreamInner(call, route)
    // Time-to-first-byte for streams (headers received), full time otherwise.
    recordGatewayStat({ ms: Date.now() - started, ok: res.ok, model: String(call.body.model ?? route?.upstreamModel ?? '?') })
    return res
  } catch (err) {
    recordGatewayStat({ ms: Date.now() - started, ok: false, model: String(call.body.model ?? route?.upstreamModel ?? '?') })
    throw err
  }
}

/** The ceiling on ONE upstream call when the caller names no budget of its own.
 *  Ten minutes is right for a long completion a human is waiting on and far too
 *  long for anything with a deadline — which is why `UpstreamCall.timeoutMs`
 *  exists and why every harness call now sets it. */
export const UPSTREAM_TIMEOUT_MS = 600_000

/** The caller's cancellation AND the wall clock, whichever fires first. A call
 *  with neither still gets the default ceiling, so no path is unbounded. */
const abortFor = (call: UpstreamCall): AbortSignal => {
  const timeout = AbortSignal.timeout(call.timeoutMs ?? UPSTREAM_TIMEOUT_MS)
  return call.signal ? AbortSignal.any([call.signal, timeout]) : timeout
}

async function fetchUpstreamInner(call: UpstreamCall, route?: ResolvedRoute): Promise<Response> {
  const send = async (): Promise<Response> => {
    const init = () => ({
      method: 'POST' as const,
      headers: call.headers,
      body: JSON.stringify(call.body),
      // A FRESH SIGNAL PER ATTEMPT, and per hostname fallback: reusing one
      // already-fired signal across the retry loop would abort the retry the
      // instant it started, which is a stranger bug than the one this replaced.
      signal: abortFor(call),
    })
    try {
      return await fetch(call.url, init())
    } catch (err) {
      const m = /^(https?):\/\/([^/:]+)(:\d+)?(\/.*)?$/.exec(call.url)
      const host = m?.[2] ?? ''
      if (m && host && !host.includes('.') && host !== 'localhost') {
        return await fetch(`${m[1]}://localhost${m[3] ?? ''}${m[4] ?? ''}`, init())
      }
      throw err
    }
  }

  for (let attempt = 0; attempt < 4; attempt++) {
    const res = await send()
    if (res.status !== 400) return res
    const text = await res.text().catch(() => '')
    const param = rejectedParam(text)
    if (!param || !(param in call.body) || classifyParam(param) === 'protected') {
      // Not a strippable-parameter rejection — hand back the original error.
      return new Response(text, {
        status: res.status,
        headers: { 'content-type': res.headers.get('content-type') ?? 'application/json' },
      })
    }
    delete call.body[param]
    // A contract parameter is still stripped — a completed call the caller knows
    // is unconstrained beats a 400 it can do nothing with — but never quietly.
    // The fact goes to three places: the caller (typed, below), the capability
    // store (so role assignment and the model self-test can read it), and the
    // log (once).
    if (isContractParam(param)) {
      const drop: ContractDrop = {
        param,
        capability: CONTRACT_PARAMS[param],
        endpoint: route?.endpoint.name ?? '?',
        model: String(call.body.model ?? route?.upstreamModel ?? '?'),
        source: 'rejected',
      }
      ;(call.contractDrops ??= []).push(drop)
      if (route) {
        // Only on a LIVE 400, never on the pre-strip path: re-recording the fact
        // from a remembered strip would restamp `at` on every process start and
        // the capability's own TTL would never fire.
        void recordCapability(capabilityKey(route.endpoint.name, route.upstreamModel), CONTRACT_PARAMS[param], {
          value: false,
          source: 'learned',
          at: new Date().toISOString(),
          detail: `${route.endpoint.name} rejected "${param}" with a 400; the call was retried without it.`,
        }).catch(() => {})
        warnContractDrop(drop)
      }
    }
    if (route) {
      const key = capabilityKey(route.endpoint.name, route.upstreamModel)
      const entry = learnedParams.get(key) ?? new Map<string, number>()
      if (!entry.has(param)) {
        entry.set(param, Date.now())
        learnedParams.set(key, entry)
        persistLearnedParams()
      }
    }
  }
  return send()
}

// ── Live gateway stats (in-memory, per app process) ─────────────────────────
// A small ring of recent upstream calls: enough for a live dashboard's
// request rate, error count, and TTFB percentiles. Deliberately not a table —
// this is a pulse, not a ledger (usage_events is the durable record).
interface GatewayStat {
  ts: number
  ms: number
  ok: boolean
  model: string
}
const STATS_MAX = 500
const gatewayStatRing: GatewayStat[] = []
function recordGatewayStat(s: Omit<GatewayStat, 'ts'>): void {
  gatewayStatRing.push({ ts: Date.now(), ...s })
  if (gatewayStatRing.length > STATS_MAX) gatewayStatRing.splice(0, gatewayStatRing.length - STATS_MAX)
}

export interface GatewayPulse {
  /** Upstream calls in the last 15 minutes (this app process). */
  requests: number
  errors: number
  /** Time-to-first-byte percentiles over those calls, ms. */
  p50: number | null
  p95: number | null
}

export function gatewayPulse(): GatewayPulse {
  const cutoff = Date.now() - 15 * 60_000
  const recent = gatewayStatRing.filter((s) => s.ts > cutoff)
  const times = recent.map((s) => s.ms).sort((a, b) => a - b)
  const pct = (p: number) => (times.length ? times[Math.min(times.length - 1, Math.floor((p / 100) * times.length))]! : null)
  return { requests: recent.length, errors: recent.filter((s) => !s.ok).length, p50: pct(50), p95: pct(95) }
}

/** Server-side non-streaming completion through the org gateway (routing +
 *  provider keys + metering). For internal callers like the QA judge — no tlk_
 *  key needed. Throws on an unknown model or an upstream error.
 *
 *  `responseFormat` is the structured-output slot, and its absence was itself an
 *  audit finding: `inbox-focus-assistant` grew a SECOND request helper — prompt
 *  suffix, different temperature, no protocol constraint — purely because this
 *  signature had nowhere to put `response_format` (audit 1.3). So the same
 *  command on the same item was a strict-JSON request or a prompt-and-pray
 *  request depending on which model the user had picked. One slot, one strategy.
 *
 *  `guard` exists because this helper is no longer always the outermost caller.
 *  `runHarness` runs its OWN guard pass with the harness's narrowed rule set and
 *  an honest `Available` for the transport that ran; leaving `guardCompletion`
 *  on underneath it would file two guard_findings rows for one reply and inflate
 *  the per-model confabulation rate that the model-fitness page reads. Defaults
 *  to true, so every existing caller keeps exactly the guard it has today.
 *
 * `contractDrops` says whether `responseFormat` actually survived to the model.
 *  A caller that asked for JSON and got an empty drop list may parse; one that
 *  sees a `json` drop has been handed prose and must repair or fall back.
 *
 *  `effort` is the caller's reasoning-effort pick, forwarded as
 *  `reasoning_effort` verbatim. Callers are expected to have checked the
 *  model's supported levels first (`server/model-efforts.ts`); an unsupported
 *  value here is the provider's to reject, which the parameter ratchet then
 *  remembers. */
export async function completeViaGateway(
  model: string,
  messages: Array<{ role: string; content: string }>,
  opts: { temperature?: number; caller: string; responseFormat?: ResponseFormat; effort?: string; guard?: boolean; signal?: AbortSignal; timeoutMs?: number },
): Promise<{ text: string; contractDrops: ContractDrop[] }> {
  const route = await resolveRoute(model)
  if (!route) throw new Error(`model "${model}" is not on the gateway`)
  const clientBody: Record<string, unknown> = { model, messages, stream: false }
  if (opts.temperature !== undefined) clientBody.temperature = opts.temperature
  if (opts.responseFormat) clientBody.response_format = opts.responseFormat
  if (opts.effort) clientBody.reasoning_effort = opts.effort
  const call = await buildUpstream(route, clientBody)
  if (opts.signal) call.signal = opts.signal
  if (opts.timeoutMs !== undefined) call.timeoutMs = opts.timeoutMs
  const res = await fetchUpstream(call, route)
  // THE STATUS AND THE BODY, BOTH. A bare "gateway completion 429" tells an
  // admin reading a red cell nothing about whether they are rate-limited, out of
  // credit or sending something the provider rejects — and those are the three
  // answers worth having when a sweep starts timing out.
  if (!res.ok) throw new Error(`gateway completion ${res.status}: ${(await res.text().catch(() => '')).slice(0, 400)}`)
  const j = (await res.json()) as {
    choices?: Array<{ message?: { content?: string } }>
    usage?: { prompt_tokens?: number; completion_tokens?: number }
  }
  if (j.usage) {
    await recordGatewayUsage({
      caller: opts.caller,
      endpoint: route.endpoint,
      upstreamModel: route.upstreamModel,
      promptTokens: j.usage.prompt_tokens ?? 0,
      completionTokens: j.usage.completion_tokens ?? 0,
      estimated: false,
    }).catch(() => {})
  }
  const text = j.choices?.[0]?.message?.content ?? ''
  // Confab guard (structural, no extra model call) — fire-and-forget so it can
  // never block or break a completion. Records findings out-of-band (observe).
  if (opts.guard !== false) {
    void guardCompletion({ answer: text, messages, caller: opts.caller, model, endpoint: route.endpoint.name }).catch(() => {})
  }
  return { text, contractDrops: contractDropsOf(call) }
}

/** Ledger row for a gateway call — attribution is direct (we KNOW the
 *  endpoint), no agent-def classification involved. */
export async function recordGatewayUsage(u: {
  caller: string // "api:<key name>" or "user:<email>"
  endpoint: LlmEndpoint
  upstreamModel: string
  promptTokens: number
  completionTokens: number
  estimated: boolean
}): Promise<void> {
  const sql = await db()
  await sql`
    insert into usage_events (agent_model, source, prompt_tokens, completion_tokens, estimated, endpoint_class, llm_model, endpoint)
    values (${u.caller}, 'gateway',
            ${Math.max(0, Math.round(u.promptTokens))}, ${Math.max(0, Math.round(u.completionTokens))},
            ${u.estimated}, ${u.endpoint.class}, ${u.upstreamModel}, ${u.endpoint.name})
  `
}
