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
import { listEndpoints, type LlmEndpoint } from './agent-defs'
import { DEFAULT_KEY_ENV, NATIVE_BASE, resolveKey } from './provider-catalog'
import { guardCompletion } from './guardrails'

export interface GatewayModel {
  id: string
  endpoints: string[]
}

/** All callable model ids: bare names + endpoint-qualified names. */
export async function gatewayModels(): Promise<GatewayModel[]> {
  const eps = await listEndpoints()
  const bare = new Map<string, string[]>()
  const out: GatewayModel[] = []
  for (const ep of eps) {
    for (const m of ep.models) {
      out.push({ id: `${ep.name}/${m}`, endpoints: [ep.name] })
      bare.set(m, [...(bare.get(m) ?? []), ep.name])
    }
  }
  for (const [id, endpoints] of bare) out.push({ id, endpoints })
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
  const key = await resolveKey(ep.apiKeyEnv ?? DEFAULT_KEY_ENV[ep.provider] ?? null)

  const headers: Record<string, string> = { 'content-type': 'application/json' }
  if (key) {
    headers['authorization'] = `Bearer ${key}`
    // Anthropic's OpenAI-compat layer accepts x-api-key; harmless elsewhere.
    if (ep.provider === 'anthropic') headers['x-api-key'] = key
  }

  const defaults = ((ep as unknown as { requestDefaults?: Record<string, unknown> }).requestDefaults ?? {}) as Record<
    string,
    unknown
  >
  const body: Record<string, unknown> = { ...deepMerge(defaults, clientBody), model: route.upstreamModel }
  if (body.stream) body.stream_options = { include_usage: true, ...(body.stream_options as object | undefined) }
  return { url: `${base}/chat/completions`, headers, body }
}

/** POST to the upstream with the dev-mode fallback: docker-internal hostnames
 *  (bare names like inference-router) don't resolve from the host — retry on
 *  localhost with the same port, where the compose stacks publish. */
export async function fetchUpstream(call: UpstreamCall): Promise<Response> {
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
  const res = await fetchUpstream(call)
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
