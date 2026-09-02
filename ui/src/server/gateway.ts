// The app talks to each agent's persona gateway directly, using the fleet
// manifest Talaria renders (fleet/fleet.json: { model, url, key } per agent and
// per model tier). No separate bridge/multiplexer — these helpers run
// server-side, so the URLs/keys stay off the client and every call is route-gated.
import { newVault, sealContent } from './secret-vault'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { FLEET_DIR } from './fleet-dir'

export interface AgentModel {
  id: string
  /** Display label — the leading segment of the id, capitalised. */
  label: string
  /** Role remainder of the id. */
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

/** Split an agent id into its display halves: "<slug>-<role>". */
export function describeAgent(id: string): AgentModel {
  const [first, ...rest] = id.split('-')
  const label = first ? first.charAt(0).toUpperCase() + first.slice(1) : id
  return { id, label, role: rest.join(' ') }
}

/** The fleet from the manifest — base models only (tier entries like
 *  "<base>-<alias>" are hidden from the picker).
 *
 *  An unrendered or missing manifest means an empty fleet, and says so. This
 *  used to fall back to three invented agents, which put names nobody created
 *  in front of the user on a fresh install and — worse — made "no agents" and
 *  "three agents" indistinguishable to every caller downstream. */
export async function listAgents(): Promise<AgentModel[]> {
  const manifest = await readManifest()
  if (!manifest.length) return []
  const ids = new Set(manifest.map((m) => m.model))
  const isTier = (id: string) => {
    const i = id.lastIndexOf('-')
    return i > 0 && ids.has(id.slice(0, i))
  }
  const bases = [...new Set(manifest.map((m) => m.model).filter((id) => !isTier(id)))]
  return bases.map(describeAgent)
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
export async function proxyChat(payload: ChatPayload, opts: { waitMs?: number; signal?: AbortSignal } = {}): Promise<Response> {
  // ── CREDENTIALS DO NOT LEAVE THIS PROCESS, PERSONA EDITION ─────────────────
  //
  // THE SECOND CHOKEPOINT. `buildUpstream` seals every GATEWAY call; this is the
  // other door — every Hermes persona turn in the tree is sent from here to the
  // agent's container, which then talks to a provider we do not control the
  // request assembly for.
  //
  // It is also the door that matters most. A persona holds workspace context all
  // day and is the agent most likely to have a credential in front of it — the
  // workbench PAT is handed to a dev agent by design — so an unsealed persona
  // path would mean the one place a secret is most likely to appear is the one
  // place nothing was checking.
  //
  // The vault is per-call and discarded when this returns: nothing downstream of
  // a persona turn spends a handle, because a persona's tool loop runs inside
  // its own container and reaches Talaria back through `callMcpTool`, which does
  // its own resolution against the agent's grants.
  const vault = newVault()
  if (Array.isArray((payload as { messages?: unknown }).messages)) {
    payload = {
      ...payload,
      // sealContent, not a string-only map: an image turn's content is an array
      // of parts, and its text part is as credential-prone as any prose turn.
      messages: ((payload as unknown as { messages: Array<Record<string, unknown>> }).messages ?? []).map((m) => ({
        ...m,
        content: sealContent(m.content, vault),
      })),
    } as ChatPayload
    for (const s of vault.sealed) console.warn(`[secrets] sealed ${s.label} out of a turn to ${payload.model}`)
  }
  const deadline = Date.now() + (opts.waitMs ?? 120_000)
  let attempt = 0
  for (;;) {
    opts.signal?.throwIfAborted()
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
        signal: opts.signal,
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
    await new Promise<void>((resolve, reject) => {
      const onAbort = () => {
        clearTimeout(timer)
        reject(opts.signal?.reason ?? new DOMException('Aborted', 'AbortError'))
      }
      const timer = setTimeout(() => {
        opts.signal?.removeEventListener('abort', onAbort)
        resolve()
      }, Math.min(5_000, 1_500 * ++attempt))
      opts.signal?.addEventListener('abort', onAbort, { once: true })
    })
  }
}

/** The agent stayed down past the hold window: say so honestly (streamed as a
 *  normal reply so history shows what happened, not a silent failure). */
function unavailableChatStream(payload: ChatPayload): Response {
  const who = describeAgent(payload.model).label
  return cannedChatStream(
    `${who} is restarting (or down) and didn't come back within two minutes. Your message is saved; send it again in a moment.`,
    'unavailable',
  )
}

/** Offline fallback: stream a canned SSE reply in OpenAI chunk format. */
function mockChatStream(payload: ChatPayload): Response {
  const who = describeAgent(payload.model).label
  return cannedChatStream(
    `Hi, this is ${who} (mock mode: no agents are rendered yet). Create and start an agent to chat for real.`,
    'mock',
  )
}

/** Set on every canned stream below, and on nothing else.
 *
 *  A canned stream is a 200 carrying an ordinary English sentence, which is the
 *  right answer for a HUMAN in a chat window and the wrong one for a harness:
 *  `personaTurn` checks `upstream.ok` and could not tell an outage from a model
 *  reply, so "Penny is restarting…" was cleaned by every text harness, written
 *  to `harness_runs` as a HELD CONTRACT, metered as a turn that never happened,
 *  and then persisted as the agent's work — a briefing summary, an
 *  `outreach_events` note fed back into the next check-in prompt, twelve
 *  `task_activity` lines. An infrastructure outage recorded as a perfect
 *  contract rate is the exact number the fitness page is being built to read. */
export const CANNED_STREAM_HEADER = 'x-talaria-canned'

/** Stream fixed text as OpenAI-format SSE chunks (mock + unavailable paths). */
function cannedChatStream(text: string, kind: 'unavailable' | 'mock'): Response {
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
    headers: { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', [CANNED_STREAM_HEADER]: kind },
  })
}
