// THE TRANSPORT RULE — the one decision in run.ts that had no test, and the one
// that shipped a regression because of it.
//
// `pickTransport` answers "gateway or persona?" for a model id. Every other test
// in run.test.ts injects a transport and therefore never exercises it, which is
// exactly why a tier id ("dex-developer-opus") could stop routing without a
// single assertion noticing: the Plan modal and the Inbox delegate picker both
// produce one, `listAgents` deliberately hides tier entries from its list, and
// the fall-through classified them as gateway models that the gateway does not
// serve.
//
// This file is separate from run.test.ts on purpose. It is the only place that
// mocks the two transport modules, so run.test.ts stays a pure dependency-
// injected exercise of the runner with nothing hoisted underneath it.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const gatewayModels = vi.fn()
const listAgents = vi.fn()
const completeViaGateway = vi.fn()
const proxyChat = vi.fn()
const resolveRoute = vi.fn()
const fetchUpstream = vi.fn()
const recordGatewayUsage = vi.fn(async () => {})

/** One upstream call, as `buildUpstream` builds it — the body is what the test
 *  reads to prove the definitions went out, and `contractDrops` is how the
 *  gateway reports that a parameter never reached the model (audit 1.2). */
interface FakeCall {
  url: string
  headers: Record<string, string>
  body: Record<string, unknown>
  contractDrops: Array<{ capability: string }>
}
let built: FakeCall[] = []
let drops: Array<{ capability: string }> = []

vi.mock('@/server/llm-gateway', () => ({
  gatewayModels: () => gatewayModels(),
  routingFor: async () => ({ endpoints: [], upstreamModel: '' }),
  completeViaGateway: (...args: unknown[]) => completeViaGateway(...args),
  resolveRoute: (...args: unknown[]) => resolveRoute(...args),
  buildUpstream: (_route: unknown, body: Record<string, unknown>) => {
    const call: FakeCall = { url: 'https://upstream.test/v1/chat/completions', headers: {}, body, contractDrops: drops }
    built.push(call)
    return Promise.resolve(call)
  },
  fetchUpstream: (...args: unknown[]) => fetchUpstream(...args),
  recordGatewayUsage: (...args: unknown[]) => recordGatewayUsage(...(args as [])),
  // The real one, inlined: a caller reads drops off the call it was handed.
  contractDropsOf: (call: FakeCall) => call.contractDrops ?? [],
}))
vi.mock('@/server/gateway', () => ({
  CANNED_STREAM_HEADER: 'x-talaria-canned',
  listAgents: () => listAgents(),
  proxyChat: (...args: unknown[]) => proxyChat(...args),
}))
vi.mock('@/server/usage', () => ({ estimateTokens: () => 0, recordUsage: async () => {} }))

const { fleetTransport, fleetStream, gatewayStream, gatewayTransport, offersToolDefinitions, pickTransport, toolWireMessage } = await import(
  '@/server/harness/run'
)

/** A rendered fleet: base ids only, which is what `listAgents` returns. */
const rendered = (...ids: string[]) => ({ agents: ids.map((id) => ({ id, label: id, role: '' })), source: 'gateway' as const })

/** Which transport came back, without calling it: the gateway transport reaches
 *  for `completeViaGateway` and the fleet transport for `proxyChat`, so one
 *  throwing call each identifies it unambiguously. */
async function kindOf(model: string): Promise<'gateway' | 'fleet'> {
  completeViaGateway.mockRejectedValue(new Error('GATEWAY'))
  proxyChat.mockRejectedValue(new Error('FLEET'))
  const transport = await pickTransport(model)
  const err = await transport({ model, messages: [], jsonMode: false, caller: 't' }).then(
    () => null,
    (e: Error) => e.message,
  )
  return err === 'FLEET' ? 'fleet' : 'gateway'
}

beforeEach(() => {
  vi.clearAllMocks()
  gatewayModels.mockResolvedValue([])
  listAgents.mockResolvedValue({ agents: [], source: 'mock' as const })
  built = []
  drops = []
  resolveRoute.mockResolvedValue({ endpoint: { name: 'spark' }, upstreamModel: 'qwen3-14b' })
})

describe('pickTransport', () => {
  it('sends a gateway catalog model to the gateway', async () => {
    gatewayModels.mockResolvedValue([{ id: 'qwen3-14b' }])
    listAgents.mockResolvedValue(rendered('dex-developer'))
    expect(await kindOf('qwen3-14b')).toBe('gateway')
  })

  it('sends a live persona to the persona gateway', async () => {
    listAgents.mockResolvedValue(rendered('dex-developer'))
    expect(await kindOf('dex-developer')).toBe('fleet')
  })

  it('sends a TIER of a live persona to the persona gateway', async () => {
    // THE REGRESSION. `listAgents` hides "dex-developer-opus", but the manifest
    // has it and `proxyChat` matches the full id — so this must route as a
    // persona, exactly as the hand-written calls did before the port. Reached
    // from the Plan modal's tier dropdown and from `validDelegate`.
    listAgents.mockResolvedValue(rendered('dex-developer'))
    expect(await kindOf('dex-developer-opus')).toBe('fleet')
  })

  it('does not treat an unrelated hyphenated id as a tier', async () => {
    // The split is at the LAST hyphen and the prefix must be a LIVE agent, so a
    // model that merely contains hyphens is not adopted into the fleet.
    listAgents.mockResolvedValue(rendered('dex-developer'))
    expect(await kindOf('llama-3-70b')).toBe('gateway')
  })

  it('never treats a MOCK fleet as a persona', async () => {
    // `listAgents` answers with three mock agents when the fleet has never been
    // rendered, and `proxyChat` answers an unknown model with a canned "this is
    // a mock" stream. Handing that to a harness would give it a chatty English
    // sentence to parse as its verdict.
    listAgents.mockResolvedValue({ agents: [{ id: 'dex-developer', label: 'Dex', role: 'developer' }], source: 'mock' as const })
    expect(await kindOf('dex-developer')).toBe('gateway')
    expect(await kindOf('dex-developer-opus')).toBe('gateway')
  })

  it('lets the gateway win when a model is somehow both', async () => {
    // The gateway is the metered, fully-inspectable path: it knows the endpoint,
    // writes its own ledger row, and hands the runner the whole message history
    // to guard against.
    gatewayModels.mockResolvedValue([{ id: 'dex-developer' }])
    listAgents.mockResolvedValue(rendered('dex-developer'))
    expect(await kindOf('dex-developer')).toBe('gateway')
  })

  it('lets the gateway report an unknown model rather than mocking one', async () => {
    listAgents.mockResolvedValue(rendered('dex-developer'))
    expect(await kindOf('nothing-serves-this')).toBe('gateway')
  })
})

// ── The canned stream, which is an outage wearing a 200 ──────────────────────

describe('the persona transport', () => {
  /** What `proxyChat` answers with when an agent never came back: a 200 SSE
   *  stream of ordinary English, in OpenAI chunk format. */
  const canned = (text: string, kind: string): Response => {
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(new TextEncoder().encode(`data: ${JSON.stringify({ choices: [{ delta: { content: text } }] })}\n\ndata: [DONE]\n\n`))
        c.close()
      },
    })
    return new Response(body, { headers: { 'Content-Type': 'text/event-stream', 'x-talaria-canned': kind } })
  }

  it('fails the run rather than parsing the "agent is restarting" sentence as an answer', async () => {
    // `upstream.ok` was the only check, so this sentence was cleaned by every
    // text harness, recorded in `harness_runs` as a HELD CONTRACT, metered as a
    // turn that never happened, and then persisted as the agent's work: a
    // briefing summary, an `outreach_events` note that the next check-in prompt
    // reads back, twelve `task_activity` lines. An outage recorded as a perfect
    // contract rate is the exact number the fitness page is built to read.
    proxyChat.mockResolvedValue(canned('Penny is restarting (or down) and did not come back.', 'unavailable'))
    await expect(fleetTransport({ model: 'penny-assistant', messages: [], jsonMode: false, caller: 't' })).rejects.toThrow(/did not come back/)
  })

  it('fails the run on a mock-mode reply too', async () => {
    proxyChat.mockResolvedValue(canned("Hi — this is Dex (mock mode: the fleet isn't rendered yet).", 'mock'))
    await expect(fleetTransport({ model: 'dex-developer', messages: [], jsonMode: false, caller: 't' })).rejects.toThrow(/not a rendered agent/)
  })
})

// ── Tool DEFINITIONS: the slot that arms `tool-select` ───────────────────────
//
// `TransportRequest` carried a tool POLICY and nothing else, so the probe that
// gates the Inbox widening (audit 1.8) could never run: there was nowhere to put
// four tool definitions and no way to see which one came back. These are the
// assertions for the slot and, more importantly, for the three refusals — a
// transport that answered WITHOUT the definitions would hand `scoreTools` a turn
// where no tool was ever offered, and that scores as `tools: false` on a
// capability record that never expires.

const WEATHER = {
  name: 'get_weather',
  description: 'Current weather for a city.',
  parameters: { type: 'object', properties: { city: { type: 'string' } }, required: ['city'] },
}

/** An OpenAI completion body carrying one tool call. */
const withToolCall = (name: string, args: string): Response =>
  new Response(
    JSON.stringify({
      choices: [{ message: { content: '', tool_calls: [{ id: 'c1', type: 'function', function: { name, arguments: args } }] } }],
      usage: { prompt_tokens: 40, completion_tokens: 12 },
    }),
    { status: 200, headers: { 'content-type': 'application/json' } },
  )

const toolRequest = { model: 'qwen3-14b', messages: [{ role: 'user' as const, content: 'weather in Lisbon?' }], jsonMode: false, caller: 'fitness:probe:tools', toolDefs: [WEATHER] }

describe('offering tool definitions', () => {
  it('puts them on the wire and reports the call back with its arguments', async () => {
    fetchUpstream.mockResolvedValue(withToolCall('get_weather', '{"city":"Lisbon"}'))
    const reply = await gatewayTransport(toolRequest)

    const body = built[0]?.body ?? {}
    expect(body.tools).toEqual([{ type: 'function', function: { name: 'get_weather', description: WEATHER.description, parameters: WEATHER.parameters } }])
    // `tool_choice: 'auto'`, never the policy's 'none': the POLICY is about the
    // model's own tools and these are ours, so suppressing them here would send
    // four definitions and forbid calling any of them.
    expect(body.tool_choice).toBe('auto')
    // The provider's call id rides along so a REPLAYED tool conversation can
    // pair a result back to its call (`Message.toolCallId`) — the dry-run loop
    // is the caller that needs it.
    expect(reply.toolCalls).toMatchObject([{ name: 'get_weather', args: '{"city":"Lisbon"}' }])
    // Same list, minus the arguments — built through `toolNamesOf` so the two
    // fields cannot report different calls.
    expect(reply.toolNames).toEqual(['get_weather'])
    expect(reply.usage).toEqual({ promptTokens: 40, completionTokens: 12 })
    expect(recordGatewayUsage).toHaveBeenCalledTimes(1)
  })

  it('reports an EMPTY list when the model answered in prose, which is a failed trial and not a missing one', async () => {
    fetchUpstream.mockResolvedValue(
      new Response(JSON.stringify({ choices: [{ message: { content: 'It is sunny in Lisbon.' } }] }), { status: 200, headers: { 'content-type': 'application/json' } }),
    )
    const reply = await gatewayTransport(toolRequest)
    expect(reply.toolCalls).toEqual([])
    expect(reply.text).toBe('It is sunny in Lisbon.')
  })

  it('REFUSES rather than answering when the endpoint rejects the tools parameter', async () => {
    // Audit 1.2 wearing tool-calling clothes: the gateway strips a parameter the
    // upstream 400'd on and the call succeeds. The reply is a perfectly ordinary
    // completion — of a question that no longer mentioned any tools. Answering
    // it quietly would write `tools: false`, permanently, about a model that was
    // never asked to call one.
    drops = [{ capability: 'tools' }]
    fetchUpstream.mockResolvedValue(withToolCall('get_weather', '{}'))
    await expect(gatewayTransport(toolRequest)).rejects.toThrow(/rejects the "tools" parameter/)
  })

  it('still reports a dropped response_format, and does not confuse the two', async () => {
    drops = [{ capability: 'json' }]
    fetchUpstream.mockResolvedValue(withToolCall('get_weather', '{}'))
    const reply = await gatewayTransport({ ...toolRequest, jsonMode: true })
    expect(reply.contractDropped).toBe(true)
    expect(built[0]?.body.response_format).toEqual({ type: 'json_object' })
  })

  it('tells the provider to switch reasoning effort off when it offers tools', async () => {
    // THE RUN THAT FOUND THIS. gpt-5.6-terra refused every tool turn with
    // "Function tools with reasoning_effort are not supported ... or set
    // reasoning_effort to 'none'", and 27 cases across four tool-loop harnesses
    // were filed as "could not reach this model". We were not SENDING the
    // parameter — the model's own default effort is what conflicts — so the
    // strip-and-retry ratchet had nothing to remove and bailed.
    drops = []
    fetchUpstream.mockResolvedValue(withToolCall('get_weather', '{}'))
    await gatewayTransport(toolRequest)
    expect(built[0]?.body.reasoning_effort).toBe('none')
  })

  it('does not send it on a turn that offers no tools', async () => {
    // The incompatibility is with FUNCTION TOOLS. A plain completion has no
    // reason to touch the model's reasoning, and quietly turning it off
    // everywhere would be a capability change smuggled in as a bug fix.
    completeViaGateway.mockResolvedValue({ text: 'a title', contractDrops: [] })
    await gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't' })
    expect(built.some((b) => 'reasoning_effort' in b.body)).toBe(false)
  })

  it('does NOT ask for json_object when the contract is rooted at an ARRAY', async () => {
    // WHAT THIS COST. `json_object` is not "reply in JSON" — it is "return a JSON
    // OBJECT", enforced by constrained decoding. `channel-plan` returns a
    // top-level array of ticket proposals and is non-strict (nullable enums, a
    // nested array), so it took the loose fallback on every single call: the
    // wire format forbade the only answer its contract would accept. Five
    // fixtures in one sweep failed `expected array, got object`, the repair turn
    // failed the same way, and all five were charged to the model.
    drops = []
    fetchUpstream.mockResolvedValue(withToolCall('get_weather', '{}'))
    await gatewayTransport({
      ...toolRequest,
      jsonMode: true,
      jsonSchema: { name: 'channel_plan', strict: false, schema: { type: 'array', items: { type: 'object' } } },
    })
    expect(built[0]?.body.response_format).toBeUndefined()
  })

  it('still asks for json_object when the contract is rooted at an object', async () => {
    // The array rule must not become "stop asking for structured output". An
    // object root is exactly what `json_object` was built for and still gets it.
    drops = []
    fetchUpstream.mockResolvedValue(withToolCall('get_weather', '{}'))
    await gatewayTransport({
      ...toolRequest,
      jsonMode: true,
      jsonSchema: { name: 'titler', strict: false, schema: { type: 'object', properties: { title: { type: 'string' } } } },
    })
    expect(built[0]?.body.response_format).toEqual({ type: 'json_object' })
  })

  it('leaves toolCalls ABSENT on a plain completion — nobody was in a position to observe one', async () => {
    completeViaGateway.mockResolvedValue({ text: 'a title', contractDrops: [] })
    const reply = await gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't' })
    expect(reply.toolCalls).toBeUndefined()
    expect(reply.toolNames).toEqual([])
  })

  it('a FLEET PERSONA refuses them: the tool loop belongs to the agent, and the stream reports bare names', async () => {
    await expect(fleetTransport({ ...toolRequest, model: 'penny-assistant' })).rejects.toThrow(/tool loop runs inside the agent/)
    await expect(fleetStream({ ...toolRequest, model: 'penny-assistant' }, () => {})).rejects.toThrow(/tool loop runs inside the agent/)
    expect(proxyChat).not.toHaveBeenCalled()
  })

  it('a STREAMING gateway turn refuses them too, rather than half-assembling a call out of deltas', async () => {
    await expect(gatewayStream(toolRequest, () => {})).rejects.toThrow(/STREAMING transport/)
    expect(fetchUpstream).not.toHaveBeenCalled()
  })
})

describe('offersToolDefinitions', () => {
  it('answers for the transport that would actually take the call', async () => {
    gatewayModels.mockResolvedValue([{ id: 'qwen3-14b' }])
    listAgents.mockResolvedValue(rendered('penny-assistant'))
    expect(await offersToolDefinitions('qwen3-14b')).toBe(true)
    // The probe reads this and SKIPS a persona candidate. Letting the refusal
    // throw instead would score as `errored`, which the probe suite reads as a
    // broken deployment — and a healthy persona is not one.
    expect(await offersToolDefinitions('penny-assistant')).toBe(false)
  })
})

describe('a tool conversation, replayed', () => {
  it('sends the tool channel rather than narrating calls in prose', () => {
    // THE BUG THIS ENDS. `Message` had no slot for a tool call, so the dry-run
    // loop wrote one into the assistant's TEXT — `[tool] write_file({...})`,
    // later `(called write_file)`. Models imitated whichever string they were
    // shown and answered the next turn in prose; 34 replies in one sweep came
    // back containing Talaria's own narration, so `toolCalls` was empty, the
    // loop broke, and fixtures reported work the model had done as never done.
    const assistant = toolWireMessage({
      role: 'assistant',
      content: 'Fixing the off-by-one.',
      toolCalls: [{ name: 'write_file', args: '{"path":"src/paginate.js"}', id: 'call_1' }],
    })
    expect(assistant).toMatchObject({
      role: 'assistant',
      content: 'Fixing the off-by-one.',
      tool_calls: [{ id: 'call_1', type: 'function', function: { name: 'write_file', arguments: '{"path":"src/paginate.js"}' } }],
    })
    // Nothing about the call is in the prose the model reads back.
    expect(JSON.stringify(assistant.content)).not.toContain('write_file')
  })

  it('pairs a result to the call it answers', () => {
    expect(toolWireMessage({ role: 'tool', content: 'written', toolCallId: 'call_1' })).toEqual({
      role: 'tool',
      content: 'written',
      tool_call_id: 'call_1',
    })
  })

  it('leaves an ordinary turn exactly as it was', () => {
    // Every harness `render` produces these and must be untouched by the change.
    expect(toolWireMessage({ role: 'user', content: 'hello' })).toEqual({ role: 'user', content: 'hello' })
    expect(toolWireMessage({ role: 'system', content: 'be terse' })).toEqual({ role: 'system', content: 'be terse' })
  })
})
