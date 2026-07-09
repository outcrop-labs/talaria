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

/** Resolve a requested model id to an endpoint + upstream model. */
export async function resolveRoute(model: string): Promise<ResolvedRoute | null> {
  const eps = await listEndpoints()
  // Endpoint-qualified: "<endpoint>/<rest>" (rest may itself contain "/").
  const slash = model.indexOf('/')
  if (slash > 0) {
    const ep = eps.find((e) => e.name === model.slice(0, slash))
    const rest = model.slice(slash + 1)
    if (ep && ep.models.includes(rest)) return { endpoint: ep, upstreamModel: rest }
  }
  const serving = eps.filter((e) => e.models.includes(model))
  if (serving.length === 0) return null
  const i = (rr.get(model) ?? 0) % serving.length
  rr.set(model, i + 1)
  return { endpoint: serving[i]!, upstreamModel: model }
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
}

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
  // Pre-strip parameters this endpoint+model has already rejected (learned
  // live from 400s below — dynamic specs with no maintained tables).
  await loadUnsupportedParams()
  for (const p of unsupportedParams.get(`${ep.name}:${route.upstreamModel}`) ?? []) delete body[p]
  return { url: `${base}/chat/completions`, headers, body }
}

// Parameter support, learned from the source: when an upstream 400 names a
// parameter we sent ("`temperature` is deprecated…", "Unsupported parameter:
// 'top_p'…"), we strip it, retry, and remember — the next call pre-strips.
// Newer models routinely retire tunables (sonnet-5 rejects temperature); no
// spec table could keep up, but the provider itself is always current.
// Learnings persist in app_settings so a restart doesn't re-pay the 400s.
const unsupportedParams = new Map<string, Set<string>>()
let unsupportedLoaded: Promise<void> | null = null
const loadUnsupportedParams = (): Promise<void> =>
  (unsupportedLoaded ??= (async () => {
    const stored = await getSetting<Record<string, string[]>>('gateway_unsupported_params', {})
    for (const [key, params] of Object.entries(stored)) {
      unsupportedParams.set(key, new Set([...(unsupportedParams.get(key) ?? []), ...params]))
    }
  })().catch(() => {}))

const persistUnsupportedParams = (): void => {
  const obj = Object.fromEntries([...unsupportedParams].map(([k, v]) => [k, [...v]]))
  void setSetting('gateway_unsupported_params', obj).catch(() => {})
}

const rejectedParam = (errText: string): string | null => {
  const m =
    /[`"']([a-z_]+)[`"'] is (?:deprecated|not supported|unsupported)/i.exec(errText) ??
    /unsupported parameter[:\s`"']+([a-z_]+)/i.exec(errText) ??
    /[`"']([a-z_]+)[`"'][^.]{0,40}(?:deprecated|not supported)/i.exec(errText)
  return m?.[1] ?? null
}

/** POST to the upstream with two adaptations: the dev-mode hostname fallback
 *  (docker-internal bare names don't resolve from the host → retry localhost),
 *  and parameter-rejection recovery (a 400 naming a parameter we sent strips
 *  it and retries, remembering per endpoint:model). */
export async function fetchUpstream(call: UpstreamCall, route?: ResolvedRoute): Promise<Response> {
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
    if (!param || !(param in call.body) || param === 'model' || param === 'messages') {
      // Not a strippable-parameter rejection — hand back the original error.
      return new Response(text, {
        status: res.status,
        headers: { 'content-type': res.headers.get('content-type') ?? 'application/json' },
      })
    }
    delete call.body[param]
    if (route) {
      const key = `${route.endpoint.name}:${route.upstreamModel}`
      const set = unsupportedParams.get(key) ?? new Set<string>()
      if (!set.has(param)) {
        set.add(param)
        unsupportedParams.set(key, set)
        persistUnsupportedParams()
      }
    }
  }
  return send()
}

/** Server-side non-streaming completion through the org gateway (routing +
 *  provider keys + metering). For internal callers like the QA judge — no tlk_
 *  key needed. Throws on an unknown model or an upstream error. */
export async function completeViaGateway(
  model: string,
  messages: Array<{ role: string; content: string }>,
  opts: { temperature?: number; caller: string },
): Promise<{ text: string }> {
  const route = await resolveRoute(model)
  if (!route) throw new Error(`model "${model}" is not on the gateway`)
  const clientBody: Record<string, unknown> = { model, messages, stream: false }
  if (opts.temperature !== undefined) clientBody.temperature = opts.temperature
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
  void guardCompletion({ answer: text, messages, caller: opts.caller, model, endpoint: route.endpoint.name }).catch(() => {})
  return { text }
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
