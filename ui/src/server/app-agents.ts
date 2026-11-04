// `ctx.agents` — an app's code driving a fleet agent: send one message, on a
// session the agent keeps or as a fresh turn, and watch or await the reply.
//
// The host never drives the agent itself. It states WHO started the app's
// code — the actor it authenticated: a person's request, an agent's tool
// call, a trigger — and the api decides whether that actor may drive the
// agent named, then runs the turn through the fleet's own chat door
// (talaria-app-agents, `POST /api/internal/apps/agent-turn`):
//
//   request   only an agent the signed-in person may use;
//   mcp       only the agent calling the tool, proven by its own key;
//   trigger   no agent: a triggered run acts as the app, with no user.
//
// The app cannot widen any of it: the actor comes from the host, and the
// session a key names is scoped by the api to this app and this person (or
// calling agent), so a key reaches no one else's conversation. Hermes keeps
// the transcript; nothing here stores one.
import { HttpError } from '@/lib/fetch-json'
import type { AppActor } from './app-context'
import { INTERNAL_HEADER } from './internal-api'
import { rustApiUrl } from './rust-proxy'
import { internalToken } from './secretbox'

/** The actor as the api's agent-turn route reads it. */
export type AgentActorWire =
  | { kind: 'request'; userId: string }
  | { kind: 'mcp'; agent: string; proven: boolean }
  | { kind: 'trigger'; trigger: string }

export function actorWire(actor: AppActor): AgentActorWire {
  switch (actor.kind) {
    case 'request':
      return { kind: 'request', userId: actor.user.id }
    case 'mcp':
      return { kind: 'mcp', agent: actor.caller.name, proven: actor.caller.proven }
    case 'trigger':
      return { kind: 'trigger', trigger: actor.trigger }
  }
}

/** One thing the agent did, as it lands. */
export type AgentTurnEvent =
  /** Some of the agent's words. */
  | { type: 'text'; text: string }
  /** A tool the agent started or finished, with its display line. */
  | { type: 'tool'; name: string; status: string; label: string }

export interface AgentTurnOptions {
  /** The agent's fleet id — an id `/api/agents` lists. */
  agent: string
  /** What to say to it. */
  message: string
  /** This turn's system prompt, on top of the agent's own. */
  system?: string
  /** A key of yours for a conversation: turns with the same key continue one
   *  session, whose transcript the agent keeps. The key is scoped to your
   *  app and to the person (or calling agent) — it never reaches another's
   *  conversation. Omit it for a turn that starts fresh. */
  session?: string
  /** A reasoning effort the agent's model publishes (`low`, `high`, …). */
  effort?: string
  /** Called with each event as it lands — the agent's words and its tool
   *  calls — to watch the turn live. The reply resolves either way. */
  watch?: (event: AgentTurnEvent) => void
  /** Your own signal: aborting it stops the turn, in the agent too. */
  signal?: AbortSignal
}

/** A finished turn. */
export interface AgentTurnReply {
  agent: string
  /** Everything the agent said. */
  text: string
  /** The tools it used, by name, in first-use order. */
  tools: string[]
  /** The session the turn landed on (an id the agent knows it by), or null
   *  for a turn without a key. */
  session: string | null
}

/** `ctx.agents`. */
export interface AppAgents {
  /** Send one turn to an agent and resolve with its reply. Throws an
   *  `HttpError`: 403 with the reason when this caller may not drive that
   *  agent, 502 when the agent failed or went quiet. */
  turn(opts: AgentTurnOptions): Promise<AgentTurnReply>
}

export interface AppAgentsBinding {
  app: string
  actor: AppActor
  /** The context's own signal: the request's disconnect, the tool's
   *  deadline, the trigger's. Every turn follows it. */
  signal?: AbortSignal
  /** Test seam: the api's base url and token, and fetch. */
  edges?: Partial<{ base: () => string | undefined; token: () => string; fetch: typeof fetch }>
}

const signalOf = (a: AbortSignal | undefined, b: AbortSignal | undefined): AbortSignal | undefined =>
  a && b ? AbortSignal.any([a, b]) : (a ?? b)

/** The api's newline-delimited answer, line by line. */
async function* lines(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buf = ''
  for (;;) {
    const { value, done } = await reader.read()
    if (done) break
    buf += decoder.decode(value, { stream: true })
    let nl: number
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl).trim()
      buf = buf.slice(nl + 1)
      if (line) yield line
    }
  }
  const rest = (buf + decoder.decode()).trim()
  if (rest) yield rest
}

/** THE ONE FACTORY: `ctx.agents` for a context the host authenticated. */
export function appAgents(binding: AppAgentsBinding): AppAgents {
  const base = binding.edges?.base ?? rustApiUrl
  const token = binding.edges?.token ?? internalToken
  const doFetch = binding.edges?.fetch ?? fetch
  return {
    async turn(opts: AgentTurnOptions): Promise<AgentTurnReply> {
      if (!opts || typeof opts.agent !== 'string' || !opts.agent.trim()) throw new TypeError('ctx.agents.turn needs an `agent`: a fleet agent id')
      if (typeof opts.message !== 'string' || !opts.message.trim()) throw new TypeError('ctx.agents.turn needs a `message`')
      const api = base()
      if (api === undefined) throw new HttpError(503, 'the platform API is stood down on this host')
      let credential: string
      try {
        credential = token()
      } catch {
        throw new HttpError(503, 'this host has no root secret to sign the call with')
      }
      const signal = signalOf(binding.signal, opts.signal)
      const res = await doFetch(`${api}/api/internal/apps/agent-turn`, {
        method: 'POST',
        headers: { [INTERNAL_HEADER]: credential, 'content-type': 'application/json', accept: 'application/x-ndjson' },
        body: JSON.stringify({
          app: binding.app,
          actor: actorWire(binding.actor),
          agent: opts.agent,
          message: opts.message,
          ...(opts.system !== undefined ? { system: opts.system } : {}),
          ...(opts.session !== undefined ? { session: opts.session } : {}),
          ...(opts.effort !== undefined ? { effort: opts.effort } : {}),
        }),
        redirect: 'manual',
        signal,
      })
      if (!res.ok || !res.body) {
        const said = (await res.json().catch(() => null)) as { error?: unknown } | null
        throw new HttpError(res.status, typeof said?.error === 'string' ? said.error : `the agent turn was refused (${res.status})`)
      }
      for await (const line of lines(res.body)) {
        let ev: Record<string, unknown>
        try {
          ev = JSON.parse(line) as Record<string, unknown>
        } catch {
          continue
        }
        if (ev.type === 'done') {
          return {
            agent: opts.agent,
            text: typeof ev.text === 'string' ? ev.text : '',
            tools: Array.isArray(ev.tools) ? ev.tools.filter((t): t is string => typeof t === 'string') : [],
            session: typeof ev.session === 'string' ? ev.session : null,
          }
        }
        if (ev.type === 'error') throw new HttpError(502, typeof ev.error === 'string' ? ev.error : 'the agent turn failed')
        if (ev.type === 'text' && typeof ev.text === 'string') opts.watch?.({ type: 'text', text: ev.text })
        else if (ev.type === 'tool' && typeof ev.name === 'string') {
          opts.watch?.({ type: 'tool', name: ev.name, status: String(ev.status ?? 'running'), label: String(ev.label ?? '') })
        }
      }
      throw new HttpError(502, 'the agent turn ended without an answer')
    },
  }
}

/** `ctx.agents` where no host is behind the call — a unit test that gave
 *  none. It refuses rather than reaching an api that is not there. */
export const NO_AGENTS: AppAgents = {
  turn: async () => {
    throw new Error('ctx.agents is not available here: this call has no host behind it (in a test, pass `agents` to callAppTool)')
  },
}

/** `agents`, with every turn also following `signal`. */
export function agentsFollowing(agents: AppAgents, signal: AbortSignal): AppAgents {
  return { turn: (opts) => agents.turn({ ...opts, signal: signalOf(opts.signal, signal) }) }
}
