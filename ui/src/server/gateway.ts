// The app talks to each agent's persona gateway directly, using the fleet
// manifest Talaria renders (fleet/fleet.json: { model, url, key } per agent and
// per model tier). No separate bridge/multiplexer — these helpers run
// server-side, so the URLs/keys stay off the client and every call is route-gated.
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { FLEET_DIR } from './fleet-render'

export interface AgentModel {
  id: string
  /** Friendly first-name label ("Dex"). */
  label: string
  /** Role remainder of the id ("developer"). */
  role: string
}

interface ManifestEntry {
  model: string
  url: string
  key: string
}

/** Read the rendered fleet manifest. Empty if the fleet hasn't been rendered. */
async function readManifest(): Promise<ManifestEntry[]> {
  const raw = await readFile(join(FLEET_DIR(), 'fleet.json'), 'utf8').catch(() => '')
  if (!raw) return []
  try {
    return JSON.parse(raw) as ManifestEntry[]
  } catch {
    return []
  }
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

/** The fleet from the manifest — base models only (tier entries like
 *  "dex-developer-opus" are hidden from the picker). */
export async function listAgents(): Promise<{ agents: AgentModel[]; source: 'gateway' | 'mock' }> {
  const manifest = await readManifest()
  if (!manifest.length) return { agents: MOCK_AGENTS, source: 'mock' }
  const ids = new Set(manifest.map((m) => m.model))
  const isTier = (id: string) => {
    const i = id.lastIndexOf('-')
    return i > 0 && ids.has(id.slice(0, i))
  }
  const bases = [...new Set(manifest.map((m) => m.model).filter((id) => !isTier(id)))]
  return { agents: bases.map(describeAgent), source: 'gateway' }
}

interface ChatPayload {
  model: string
  // Content is either a plain string or OpenAI-style content parts (text +
  // image_url data URLs) — passed through to the persona gateway untouched.
  messages: Array<{ role: string; content: string | Array<{ type: string; [k: string]: unknown }> }>
  [k: string]: unknown
}

/** Proxy a streaming chat straight to the agent's persona gateway.
 *
 *  A restarting agent (org/config propagation, an edit being applied) refuses
 *  connections for tens of seconds. Instead of failing the turn — or worse,
 *  answering with the mock — HOLD it and retry until the deadline: the manifest
 *  is re-read on every attempt (a re-render may have moved the agent's port),
 *  and the completion streams the moment the agent is back. The user just sees
 *  a slightly longer thinking state; nobody's work is lost to an edit. */
export async function proxyChat(payload: ChatPayload, opts: { waitMs?: number } = {}): Promise<Response> {
  const deadline = Date.now() + (opts.waitMs ?? 120_000)
  let attempt = 0
  for (;;) {
    const manifest = await readManifest()
    const entry = manifest.find((m) => m.model === payload.model)
    if (!entry && attempt >= 2) return mockChatStream(payload) // never rendered — not a restart

    if (entry) {
      const upstream = await fetch(`${entry.url}/v1/chat/completions`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(entry.key ? { Authorization: `Bearer ${entry.key}` } : {}),
        },
        // include_usage: the final chunk reports token counts for the ledger
        // (gateways that don't support it just ignore the option).
        body: JSON.stringify({ ...payload, stream: true, stream_options: { include_usage: true } }),
      }).catch(() => null)
      // 502/503/504 = the gateway process is up but not serving yet — keep waiting.
      if (upstream && ![502, 503, 504].includes(upstream.status)) {
        return new Response(upstream.body, {
          status: upstream.status,
          headers: {
            'Content-Type': upstream.headers.get('content-type') ?? 'text/event-stream',
            'Cache-Control': 'no-cache',
          },
        })
      }
    }

    if (Date.now() >= deadline) return unavailableChatStream(payload)
    await new Promise((r) => setTimeout(r, Math.min(5_000, 1_500 * ++attempt)))
  }
}

/** The agent stayed down past the hold window: say so honestly (streamed as a
 *  normal reply so history shows what happened, not a silent failure). */
function unavailableChatStream(payload: ChatPayload): Response {
  const who = describeAgent(payload.model).label
  return cannedChatStream(
    `${who} is restarting (or down) and didn't come back within two minutes — your message is saved; send it again in a moment.`,
  )
}

/** Offline fallback: stream a canned SSE reply in OpenAI chunk format. */
function mockChatStream(payload: ChatPayload): Response {
  const who = describeAgent(payload.model).label
  return cannedChatStream(
    `Hi — this is ${who} (mock mode: the fleet isn't rendered yet). Create and start an agent to chat for real.`,
  )
}

/** Stream fixed text as OpenAI-format SSE chunks (mock + unavailable paths). */
function cannedChatStream(text: string): Response {
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
