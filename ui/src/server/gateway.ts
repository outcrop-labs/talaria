// BFF to Talaria's gateway plane (the fleet multiplexer). The UI never talks to
// the gateway directly — these helpers run server-side, so the fleet URL/keys
// stay off the client and every call is auth-gated by the route.
//
//   TALARIA_GATEWAY_URL   base of the gateway plane (e.g. http://127.0.0.1:8642)
//                         unset ⇒ a tiny mock so the UI still works offline.

export interface AgentModel {
  id: string
  /** Friendly first-name label ("Dex"). */
  label: string
  /** Role remainder of the id ("developer"). */
  role: string
}

export function getGatewayUrl(): string | null {
  return process.env.TALARIA_GATEWAY_URL?.replace(/\/$/, '') || null
}

/** "dex-developer" → { label: "Dex", role: "developer" } */
export function describeAgent(id: string): AgentModel {
  const [first, ...rest] = id.split('-')
  const label = first ? first.charAt(0).toUpperCase() + first.slice(1) : id
  return { id, label, role: rest.join(' ') }
}

const MOCK_AGENTS: AgentModel[] = ['dex-developer', 'sam-support', 'penny-administrative-assistant'].map(
  describeAgent,
)

/** GET the fleet from the gateway plane's /v1/models (falls back to mock). */
export async function listAgents(): Promise<{ agents: AgentModel[]; source: 'gateway' | 'mock' }> {
  const url = getGatewayUrl()
  if (!url) return { agents: MOCK_AGENTS, source: 'mock' }
  try {
    const r = await fetch(`${url}/v1/models`, { signal: AbortSignal.timeout(8000) })
    if (!r.ok) throw new Error(`models ${r.status}`)
    const j = (await r.json()) as { data?: Array<{ id: string }> }
    return { agents: (j.data ?? []).map((m) => describeAgent(m.id)), source: 'gateway' }
  } catch {
    return { agents: MOCK_AGENTS, source: 'mock' }
  }
}

interface ChatPayload {
  model: string
  messages: Array<{ role: string; content: string }>
  [k: string]: unknown
}

/** Proxy a streaming chat to the gateway plane, piping SSE straight back. */
export async function proxyChat(payload: ChatPayload): Promise<Response> {
  const url = getGatewayUrl()
  if (!url) return mockChatStream(payload)

  const upstream = await fetch(`${url}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    // include_usage: the final chunk reports token counts for the ledger
    // (gateways that don't support it just ignore the option).
    body: JSON.stringify({ ...payload, stream: true, stream_options: { include_usage: true } }),
  })
  return new Response(upstream.body, {
    status: upstream.status,
    headers: {
      'Content-Type': upstream.headers.get('content-type') ?? 'text/event-stream',
      'Cache-Control': 'no-cache',
    },
  })
}

/** Offline fallback: stream a canned SSE reply in OpenAI chunk format. */
function mockChatStream(payload: ChatPayload): Response {
  const who = describeAgent(payload.model).label
  const text = `Hi — this is ${who} (mock mode: no live gateway configured). ` +
    `Set TALARIA_GATEWAY_URL to reach the real fleet.`
  const words = text.split(' ')
  const enc = new TextEncoder()
  const stream = new ReadableStream<Uint8Array>({
    async start(controller) {
      for (const w of words) {
        const chunk = { choices: [{ delta: { content: w + ' ' } }] }
        controller.enqueue(enc.encode(`data: ${JSON.stringify(chunk)}\n\n`))
        await new Promise((r) => setTimeout(r, 35))
      }
      controller.enqueue(enc.encode('data: [DONE]\n\n'))
      controller.close()
    },
  })
  return new Response(stream, {
    headers: { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' },
  })
}
