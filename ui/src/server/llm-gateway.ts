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

export interface GatewayModel {
  id: string
  endpoints: string[]
  /** True for "<endpoint>/<model>" pins. Bare model ids may themselves contain
   *  "/" (OpenRouter, HF-style names) — only this flag tells the two apart. */
  qualified: boolean
}

/** All callable model ids: bare names + endpoint-qualified names. */
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
  for (const [id, endpoints] of bare) out.push({ id, endpoints, qualified: false })
  return out.sort((a, b) => a.id.localeCompare(b.id))
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
}

/** Contract parameters this call lost, for a caller that would rather not think
 *  about the optionality above. Empty means the model saw exactly what you
 *  asked for. */
export const contractDropsOf = (call: UpstreamCall): ContractDrop[] => call.contractDrops ?? []

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
  return { url: `${base}/chat/completions`, headers, body, contractDrops }
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

async function fetchUpstreamInner(call: UpstreamCall, route?: ResolvedRoute): Promise<Response> {
  const send = async (): Promise<Response> => {
    const init = () => ({
      method: 'POST' as const,
      headers: call.headers,
      body: JSON.stringify(call.body),
      signal: AbortSignal.timeout(600_000),
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
 *  `contractDrops` says whether `responseFormat` actually survived to the model.
 *  A caller that asked for JSON and got an empty drop list may parse; one that
 *  sees a `json` drop has been handed prose and must repair or fall back. */
export async function completeViaGateway(
  model: string,
  messages: Array<{ role: string; content: string }>,
  opts: { temperature?: number; caller: string; responseFormat?: 'json_object'; guard?: boolean },
): Promise<{ text: string; contractDrops: ContractDrop[] }> {
  const route = await resolveRoute(model)
  if (!route) throw new Error(`model "${model}" is not on the gateway`)
  const clientBody: Record<string, unknown> = { model, messages, stream: false }
  if (opts.temperature !== undefined) clientBody.temperature = opts.temperature
  if (opts.responseFormat) clientBody.response_format = { type: opts.responseFormat }
  const call = await buildUpstream(route, clientBody)
  const res = await fetchUpstream(call, route)
  if (!res.ok) throw new Error(`gateway completion ${res.status}: ${await res.text()}`)
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
