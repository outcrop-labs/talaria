// `ctx.agents` (app-agents.ts): what the host tells the api about the
// caller, how a refusal and a failed turn reach the app, and the streamed
// answer read into watch events and a reply. Who may drive whom is the api's
// (talaria-app-agents tests); the host only states the actor.
import { describe, expect, it } from 'vitest'
import { HttpError } from '@/lib/fetch-json'
import { actorWire, agentsFollowing, appAgents, type AgentTurnEvent } from './app-agents'

const ndjson = (...lines: unknown[]) =>
  new Response(lines.map((l) => `${JSON.stringify(l)}\n`).join(''), { headers: { 'content-type': 'application/x-ndjson' } })

function harness(answer: () => Response) {
  const calls: Array<{ url: string; body: Record<string, unknown>; headers: Headers; signal: AbortSignal | null | undefined }> = []
  const fetchFn = (async (url: string, init: RequestInit) => {
    calls.push({ url, body: JSON.parse(String(init.body)), headers: new Headers(init.headers), signal: init.signal })
    return answer()
  }) as unknown as typeof fetch
  return { calls, edges: { base: () => 'http://127.0.0.1:5274', token: () => 'tok', fetch: fetchFn } }
}

describe('actorWire', () => {
  it('states each actor as the api reads it, and nothing the app chose', () => {
    expect(actorWire({ kind: 'request', user: { id: 'u1', role: 'member' } })).toEqual({ kind: 'request', userId: 'u1' })
    expect(actorWire({ kind: 'mcp', caller: { name: 'ada-ops', id: 'a1', proven: true }, actingUserId: 'u1' })).toEqual({
      kind: 'mcp',
      agent: 'ada-ops',
      proven: true,
    })
    expect(actorWire({ kind: 'trigger', trigger: 'nightly' })).toEqual({ kind: 'trigger', trigger: 'nightly' })
  })
})

describe('appAgents.turn', () => {
  it('streams the turn into watch events and resolves with the reply', async () => {
    const h = harness(() =>
      ndjson(
        { type: 'text', text: 'Hel' },
        { type: 'tool', name: 'terminal', status: 'running', label: 'ls' },
        { type: 'text', text: 'lo' },
        { type: 'done', text: 'Hello', tools: ['terminal'], session: 'sess-1' },
      ),
    )
    const seen: AgentTurnEvent[] = []
    const agents = appAgents({ app: 'crm', actor: { kind: 'request', user: { id: 'u1', role: 'member' } }, edges: h.edges })
    const reply = await agents.turn({ agent: 'ada-ops', message: 'hi', session: 'lead-42', watch: (e) => seen.push(e) })
    expect(reply).toEqual({ agent: 'ada-ops', text: 'Hello', tools: ['terminal'], session: 'sess-1' })
    expect(seen).toEqual([
      { type: 'text', text: 'Hel' },
      { type: 'tool', name: 'terminal', status: 'running', label: 'ls' },
      { type: 'text', text: 'lo' },
    ])
    expect(h.calls[0]!.url).toBe('http://127.0.0.1:5274/api/internal/apps/agent-turn')
    expect(h.calls[0]!.headers.get('x-talaria-internal')).toBe('tok')
    expect(h.calls[0]!.body).toEqual({
      app: 'crm',
      actor: { kind: 'request', userId: 'u1' },
      agent: 'ada-ops',
      message: 'hi',
      session: 'lead-42',
    })
  })

  it("throws the api's refusal with its status", async () => {
    const h = harness(() => Response.json({ error: 'app "crm" may not drive an agent from its trigger "nightly"' }, { status: 403 }))
    const agents = appAgents({ app: 'crm', actor: { kind: 'trigger', trigger: 'nightly' }, edges: h.edges })
    const err = await agents.turn({ agent: 'ada-ops', message: 'hi' }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(HttpError)
    expect(err).toMatchObject({ status: 403, message: 'app "crm" may not drive an agent from its trigger "nightly"' })
  })

  it('throws a failed turn as a 502 with the sentence', async () => {
    const h = harness(() => ndjson({ type: 'text', text: 'partial' }, { type: 'error', error: 'agent "ada-ops" sent nothing for 10 minutes' }))
    const agents = appAgents({ app: 'crm', actor: { kind: 'request', user: { id: 'u1', role: 'member' } }, edges: h.edges })
    await expect(agents.turn({ agent: 'ada-ops', message: 'hi' })).rejects.toMatchObject({ status: 502, message: 'agent "ada-ops" sent nothing for 10 minutes' })
  })

  it('treats an answer that ends without a verdict as a failure', async () => {
    const h = harness(() => ndjson({ type: 'text', text: 'partial' }))
    const agents = appAgents({ app: 'crm', actor: { kind: 'request', user: { id: 'u1', role: 'member' } }, edges: h.edges })
    await expect(agents.turn({ agent: 'ada-ops', message: 'hi' })).rejects.toMatchObject({ status: 502 })
  })

  it('refuses a turn with no agent or message before any call', async () => {
    const h = harness(() => ndjson())
    const agents = appAgents({ app: 'crm', actor: { kind: 'request', user: { id: 'u1', role: 'member' } }, edges: h.edges })
    await expect(agents.turn({ agent: '', message: 'hi' })).rejects.toBeInstanceOf(TypeError)
    await expect(agents.turn({ agent: 'ada-ops', message: '  ' })).rejects.toBeInstanceOf(TypeError)
    expect(h.calls).toHaveLength(0)
  })

  it("follows the context's signal and the caller's own", async () => {
    const h = harness(() => ndjson({ type: 'done', text: '', tools: [], session: null }))
    const ctx = new AbortController()
    const own = new AbortController()
    const agents = agentsFollowing(
      appAgents({ app: 'crm', actor: { kind: 'mcp', caller: { name: 'ada-ops', id: 'a1', proven: true }, actingUserId: null }, edges: h.edges }),
      ctx.signal,
    )
    await agents.turn({ agent: 'ada-ops', message: 'hi', signal: own.signal })
    const sent = h.calls[0]!.signal!
    expect(sent.aborted).toBe(false)
    ctx.abort()
    expect(sent.aborted).toBe(true)
  })
})
