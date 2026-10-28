// THE TRANSPORT RULE — the one decision in run.ts that had no test, and the one
// that shipped a regression because of it.
//
// `pickTransport` answers "gateway or persona?" for a model id. Every other test
// in run.test.ts injects a transport and therefore never exercises it, which is
// exactly why a tier id ("engineer-engineering-opus") could stop routing without a
// single assertion noticing: the Plan modal and the Inbox delegate picker both
// produce one, `listAgents` deliberately hides tier entries from its list, and
// the fall-through classified them as gateway models that the gateway does not
// serve.
//
// AND THE ONE DOOR. Both kinds of call now go to the Rust relay
// (`/api/llm/v1/chat/completions`) on the gateway key of the app whose code is
// running; the relay is a stubbed `fetch` here, and every assertion about the
// request reads the body and headers it was sent.
//
// This file is separate from run.test.ts on purpose. It is the only place that
// mocks the transport's edges, so run.test.ts stays a pure dependency-injected
// exercise of the runner with nothing hoisted underneath it.
import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest'

const gatewayModels = vi.fn()
const listAgents = vi.fn()
const scope = { slug: 'crm' as string | null }
const keys = { current: 'tlk_first', asked: 0, forgotten: 0 }

vi.mock('@/server/llm-gateway', () => ({
  gatewayModels: () => gatewayModels(),
  routingFor: async () => ({ endpoints: [], upstreamModel: '' }),
}))
vi.mock('@/server/gateway', () => ({ listAgents: () => listAgents() }))
vi.mock('@/server/app-scope', () => ({
  appInScope: () => scope.slug,
  withApp: <T,>(_slug: string, fn: () => T): T => fn(),
}))
vi.mock('@/server/app-llm-key', () => ({
  appGatewayKey: async () => {
    keys.asked++
    return keys.current
  },
  forgetAppGatewayKey: () => {
    keys.forgotten++
  },
}))
vi.mock('@/server/rust-proxy', () => ({ rustApiUrl: () => 'http://api.test' }))

/** One call the relay received: the url, its headers and the parsed body. */
interface Sent {
  url: string
  headers: Record<string, string>
  body: Record<string, unknown>
}
let sent: Sent[] = []
/** What the relay answers the next call with; `fresh()` per call because a
 *  Response body reads once. */
let answers: Array<() => Response> = []
let fallback: () => Response = () => json({ choices: [{ message: { content: '' } }] })

const json = (body: unknown, init: { status?: number; headers?: Record<string, string> } = {}): Response =>
  new Response(JSON.stringify(body), { status: init.status ?? 200, headers: { 'content-type': 'application/json', ...init.headers } })

const relay = vi.fn(async (url: string, init: { headers: Record<string, string>; body: string }) => {
  sent.push({ url, headers: init.headers, body: JSON.parse(init.body) as Record<string, unknown> })
  return (answers.shift() ?? fallback)()
})
const realFetch = globalThis.fetch
globalThis.fetch = relay as unknown as typeof fetch
afterAll(() => {
  globalThis.fetch = realFetch
})

const {
  fleetTransport,
  fleetStream,
  gatewayImageTurn,
  gatewayStream,
  gatewayTransport,
  offersToolDefinitions,
  pickTransport,
  toolWireMessage,
} = await import('@/server/harness/run')

/** A rendered fleet: base ids only, which is what `listAgents` returns. */
const rendered = (...ids: string[]) => ids.map((id) => ({ id, label: id, role: '' }))

/** Which transport came back, read off the request it sent: a gateway call
 *  carries an OpenAI body (`stream` set), a persona turn the persona payload
 *  (no `stream` — the door adds it). */
async function kindOf(model: string): Promise<'gateway' | 'fleet'> {
  answers.push(() => json({ error: { message: 'stop' } }, { status: 500 }))
  const transport = await pickTransport(model)
  await transport({ model, messages: [], jsonMode: false, caller: 't' }).catch(() => null)
  return 'stream' in (sent.at(-1)?.body ?? {}) ? 'gateway' : 'fleet'
}

/** A persona reply stream, as `pumpPersonaStream` reads it. */
const sse = (chunks: string[]) =>
  new ReadableStream<Uint8Array>({
    start(c) {
      for (const chunk of chunks) c.enqueue(new TextEncoder().encode(chunk))
      c.close()
    },
  })
const stream = (chunks: string[], headers: Record<string, string> = {}) => () =>
  new Response(sse(chunks), { headers: { 'content-type': 'text/event-stream', ...headers } })

beforeEach(() => {
  vi.clearAllMocks()
  gatewayModels.mockResolvedValue([])
  listAgents.mockResolvedValue([])
  sent = []
  answers = []
  fallback = () => json({ choices: [{ message: { content: '' } }] })
  scope.slug = 'crm'
  Object.assign(keys, { current: 'tlk_first', asked: 0, forgotten: 0 })
})

describe('pickTransport', () => {
  it('sends a gateway catalog model to the gateway', async () => {
    gatewayModels.mockResolvedValue([{ id: 'qwen3-14b' }])
    listAgents.mockResolvedValue(rendered('engineer-engineering'))
    expect(await kindOf('qwen3-14b')).toBe('gateway')
  })

  it('sends a live persona to the persona gateway', async () => {
    listAgents.mockResolvedValue(rendered('engineer-engineering'))
    expect(await kindOf('engineer-engineering')).toBe('fleet')
  })

  it('sends a TIER of a live persona to the persona gateway', async () => {
    // THE REGRESSION. `listAgents` hides "engineer-engineering-opus", but the manifest
    // has it and the persona door matches the full id — so this must route as a
    // persona, exactly as the hand-written calls did before the port. Reached
    // from the Plan modal's tier dropdown and from `validDelegate`.
    listAgents.mockResolvedValue(rendered('engineer-engineering'))
    expect(await kindOf('engineer-engineering-opus')).toBe('fleet')
  })

  it('does not treat an unrelated hyphenated id as a tier', async () => {
    // The split is at the LAST hyphen and the prefix must be a LIVE agent, so a
    // model that merely contains hyphens is not adopted into the fleet.
    listAgents.mockResolvedValue(rendered('engineer-engineering'))
    expect(await kindOf('llama-3-70b')).toBe('gateway')
  })

  it('never treats an UNRENDERED fleet as a persona', async () => {
    // `listAgents` used to answer with three invented agents when the fleet had
    // never been rendered, and the persona door answers an unknown model with a
    // canned "this is a mock" stream — which would have handed a harness a
    // chatty English sentence to parse as its verdict. The invented agents are
    // gone; an unrendered fleet is now an empty list, and nothing in it can
    // match, so every model falls through to the gateway.
    listAgents.mockResolvedValue([])
    expect(await kindOf('engineer-engineering')).toBe('gateway')
    expect(await kindOf('engineer-engineering-opus')).toBe('gateway')
  })

  it('lets the gateway win when a model is somehow both', async () => {
    // The gateway is the fully-inspectable path: it knows the endpoint, and it
    // hands the runner the whole message history to guard against.
    gatewayModels.mockResolvedValue([{ id: 'engineer-engineering' }])
    listAgents.mockResolvedValue(rendered('engineer-engineering'))
    expect(await kindOf('engineer-engineering')).toBe('gateway')
  })

  it('lets the gateway report an unknown model rather than mocking one', async () => {
    listAgents.mockResolvedValue(rendered('engineer-engineering'))
    expect(await kindOf('nothing-serves-this')).toBe('gateway')
  })
})

// ── The canned stream, which is an outage wearing a 200 ──────────────────────

describe('the persona transport', () => {
  /** What the relay's persona door answers with when an agent never came
   *  back: a 200 SSE stream of ordinary English, in OpenAI chunk format, with
   *  the canned header saying so. */
  const canned = (text: string, kind: string) =>
    stream([`data: ${JSON.stringify({ choices: [{ delta: { content: text } }] })}\n\ndata: [DONE]\n\n`], { 'x-talaria-canned': kind })

  it('fails the run rather than parsing the "agent is restarting" sentence as an answer', async () => {
    // `upstream.ok` was the only check, so this sentence was cleaned by every
    // text harness, recorded in `harness_runs` as a HELD CONTRACT, metered as a
    // turn that never happened, and then persisted as the agent's work: a
    // briefing summary, an `outreach_events` note that the next check-in prompt
    // reads back, twelve `task_activity` lines. An outage recorded as a perfect
    // contract rate is the exact number the fitness page is built to read.
    answers.push(canned('Penny is restarting (or down) and did not come back.', 'hold'))
    await expect(fleetTransport({ model: 'assistant-operations', messages: [], jsonMode: false, caller: 't' })).rejects.toThrow(/did not come back/)
  })

  it('fails the run on a mock-mode reply too', async () => {
    answers.push(canned("Hi — this is Dex (mock mode: the fleet isn't rendered yet).", 'mock'))
    await expect(fleetTransport({ model: 'engineer-engineering', messages: [], jsonMode: false, caller: 't' })).rejects.toThrow(/not a rendered agent/)
  })

  it('goes to the relay as the app, with the hold and the run\'s refs, and writes no row of its own', async () => {
    // THE DOUBLE METER. This side used to drive the agent's container itself
    // and write a ledger row for the turn, while the agent's own calls reached
    // the relay on its key with no turn recorded and were booked again as
    // `unattended`. Now the relay drives the turn through its persona door with
    // the turn recorded as the app's, and the agent's calls are the one meter.
    answers.push(stream(['data: {"choices":[{"delta":{"content":"done"}}]}\n\n', 'data: {"usage":{"prompt_tokens":7,"completion_tokens":3}}\n\n', 'data: [DONE]\n\n']))
    const reply = await fleetTransport({
      model: 'engineer-engineering',
      messages: [{ role: 'user', content: 'hi' }],
      jsonMode: false,
      caller: 'app:crm:triage',
      holdMs: 600_000,
      ledger: { refId: 'r-1', taskId: 't-1' },
    })
    expect(reply).toMatchObject({ kind: 'fleet', text: 'done', usage: { promptTokens: 7, completionTokens: 3 } })
    const call = sent[0]!
    expect(call.url).toBe('http://api.test/api/llm/v1/chat/completions')
    expect(call.headers.authorization).toBe('Bearer tlk_first')
    expect(call.headers['x-talaria-caller']).toBe('app:crm:triage')
    expect(call.headers['x-talaria-hold-ms']).toBe('600000')
    expect(call.headers['x-talaria-ref-id']).toBe('r-1')
    expect(call.headers['x-talaria-task-id']).toBe('t-1')
    expect(call.body.model).toBe('engineer-engineering')
  })
})

// ── The one door ─────────────────────────────────────────────────────────────

describe('the relay', () => {
  it('refuses a call made outside any app, before anything is sent', async () => {
    // No app in scope means no key to sign with and no budget to charge: a
    // call on nobody's account is the second gateway this replaced.
    scope.slug = null
    await expect(gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 'app:other' })).rejects.toThrow(/outside any app's code/)
    expect(relay).not.toHaveBeenCalled()
  })

  it('signs with the key of the app in scope, whatever caller the app named', async () => {
    // The caller stamp rides as a header the relay keeps inside the app's own
    // namespace; the key — the app's identity — is the host's to choose.
    await gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 'app:billing' })
    expect(sent[0]!.headers.authorization).toBe('Bearer tlk_first')
    expect(sent[0]!.headers['x-talaria-caller']).toBe('app:billing')
  })

  it('asks for the key again once when the relay no longer knows it', async () => {
    // A disable and re-enable rotates the app's key; the host's copy is stale
    // until it asks.
    answers.push(() => json({ error: { message: 'invalid API key' } }, { status: 401 }))
    fallback = () => json({ choices: [{ message: { content: 'ok' } }] })
    keys.current = 'tlk_second'
    const reply = await gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't' })
    expect(reply.text).toBe('ok')
    expect(keys.forgotten).toBe(1)
    expect(sent).toHaveLength(2)
    // A second refusal is not retried forever.
    answers.push(() => json({ error: { message: 'invalid API key' } }, { status: 401 }))
    answers.push(() => json({ error: { message: 'invalid API key' } }, { status: 401 }))
    await expect(gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't' })).rejects.toThrow(/401: invalid API key/)
  })

  it("fails with the relay's own sentence — a budget, an unknown model", async () => {
    answers.push(() => json({ error: { message: '"app:crm" has reached its LLM budget: 1,200 of 1,000 tokens in the last 24h.' } }, { status: 429 }))
    await expect(gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't' })).rejects.toThrow(/429: "app:crm" has reached its LLM budget/)
  })

  it("reports the provider's own token counts on a plain completion", async () => {
    // What a fitness replay scores: the gateway's blocking path used to hand
    // back `usage: null`, and every case's tokens were a chars/4 guess.
    answers.push(() => json({ choices: [{ message: { content: 'a title' } }], usage: { prompt_tokens: 120, completion_tokens: 9 } }))
    const reply = await gatewayTransport({ model: 'qwen3-14b', messages: [{ role: 'user', content: 'name it' }], jsonMode: false, caller: 't' })
    expect(reply.usage).toEqual({ promptTokens: 120, completionTokens: 9 })
    expect(sent[0]!.body).toMatchObject({ model: 'qwen3-14b', stream: false, messages: [{ role: 'user', content: 'name it' }] })
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

/** The relay's report that a contract parameter never reached the model. */
const dropped = (res: Response, caps: string): Response => {
  res.headers.set('x-talaria-contract-drops', caps)
  return res
}

const toolRequest = { model: 'qwen3-14b', messages: [{ role: 'user' as const, content: 'weather in Lisbon?' }], jsonMode: false, caller: 'fitness:probe:tools', toolDefs: [WEATHER] }

describe('offering tool definitions', () => {
  it('puts them on the wire and reports the call back with its arguments', async () => {
    answers.push(() => withToolCall('get_weather', '{"city":"Lisbon"}'))
    const reply = await gatewayTransport(toolRequest)

    const body = sent[0]?.body ?? {}
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
  })

  it('reports an EMPTY list when the model answered in prose, which is a failed trial and not a missing one', async () => {
    answers.push(() => json({ choices: [{ message: { content: 'It is sunny in Lisbon.' } }] }))
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
    answers.push(() => dropped(withToolCall('get_weather', '{}'), 'tools'))
    await expect(gatewayTransport(toolRequest)).rejects.toThrow(/rejects the "tools" parameter/)
  })

  it('still reports a dropped response_format, and does not confuse the two', async () => {
    answers.push(() => dropped(withToolCall('get_weather', '{}'), 'json'))
    const reply = await gatewayTransport({ ...toolRequest, jsonMode: true })
    expect(reply.contractDropped).toBe(true)
    expect(sent[0]?.body.response_format).toEqual({ type: 'json_object' })
  })

  it('renders the tool channel on a turn that REPLAYS one but offers no tools', async () => {
    // THE FOURTH REQUEST. `toolSearchTransport` ends with a closing turn — the
    // model has searched, now it writes the answer — which offers no tools and
    // carries the whole tool conversation behind it. With no defs it fell
    // through to the plain completion, which flattened every message to
    // `{role, content}` and drops `tool_call_id`. Anthropic then refused the
    // request it had itself produced two turns earlier:
    //
    //   messages.3.tool.tool_call_id: Field required
    //
    // Three tool turns came back 200 and the fourth 400'd, which is exactly why
    // this read as "the model cannot reach the search tool".
    answers.push(() => withToolCall('get_weather', '{}'))
    await gatewayTransport({
      model: 'claude-sonnet-5',
      caller: 't',
      jsonMode: false,
      messages: [
        { role: 'user', content: 'q' },
        { role: 'assistant', content: '', toolCalls: [{ name: 'web_search', args: '{}', id: 'toolu_1' }] },
        { role: 'tool', content: 'results', toolCallId: 'toolu_1' },
        { role: 'user', content: 'now answer' },
      ],
    })
    const msgs = sent[0]?.body.messages as Array<Record<string, unknown>>
    expect(msgs.find((m) => m.role === 'tool')?.tool_call_id).toBe('toolu_1')
    // AND IT OFFERS NOTHING, because nothing was asked for. `tools: []` with
    // `tool_choice: 'auto'` is a request some providers reject and none needs.
    expect('tools' in (sent[0]?.body ?? {})).toBe(false)
    expect('tool_choice' in (sent[0]?.body ?? {})).toBe(false)
    // `reasoning_effort` belongs to a turn that OFFERS tools, not to every replay.
    expect('reasoning_effort' in (sent[0]?.body ?? {})).toBe(false)
  })

  it('tells the provider to switch reasoning effort off when it offers tools', async () => {
    // THE RUN THAT FOUND THIS. gpt-5.6-terra refused every tool turn with
    // "Function tools with reasoning_effort are not supported ... or set
    // reasoning_effort to 'none'", and 27 cases across four tool-loop harnesses
    // were filed as "could not reach this model". We were not SENDING the
    // parameter — the model's own default effort is what conflicts — so the
    // strip-and-retry ratchet had nothing to remove and bailed.
    answers.push(() => withToolCall('get_weather', '{}'))
    await gatewayTransport(toolRequest)
    expect(sent[0]?.body.reasoning_effort).toBe('none')
  })

  it('does not send it on a turn that offers no tools', async () => {
    // The incompatibility is with FUNCTION TOOLS. A plain completion has no
    // reason to touch the model's reasoning, and quietly turning it off
    // everywhere would be a capability change smuggled in as a bug fix.
    await gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't' })
    expect(sent.some((b) => 'reasoning_effort' in b.body)).toBe(false)
  })

  it('does NOT ask for json_object when the contract is rooted at an ARRAY', async () => {
    // WHAT THIS COST. `json_object` is not "reply in JSON" — it is "return a JSON
    // OBJECT", enforced by constrained decoding. `channel-plan` returns a
    // top-level array of ticket proposals and is non-strict (nullable enums, a
    // nested array), so it took the loose fallback on every single call: the
    // wire format forbade the only answer its contract would accept. Five
    // fixtures in one sweep failed `expected array, got object`, the repair turn
    // failed the same way, and all five were charged to the model.
    answers.push(() => withToolCall('get_weather', '{}'))
    await gatewayTransport({
      ...toolRequest,
      jsonMode: true,
      jsonSchema: { name: 'channel_plan', strict: false, schema: { type: 'array', items: { type: 'object' } } },
    })
    expect(sent[0]?.body.response_format).toBeUndefined()
  })

  it('still asks for json_object when the contract is rooted at an object', async () => {
    // The array rule must not become "stop asking for structured output". An
    // object root is exactly what `json_object` was built for and still gets it.
    answers.push(() => withToolCall('get_weather', '{}'))
    await gatewayTransport({
      ...toolRequest,
      jsonMode: true,
      jsonSchema: { name: 'titler', strict: false, schema: { type: 'object', properties: { title: { type: 'string' } } } },
    })
    expect(sent[0]?.body.response_format).toEqual({ type: 'json_object' })
  })

  it('leaves toolCalls ABSENT on a plain completion — nobody was in a position to observe one', async () => {
    const reply = await gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't' })
    expect(reply.toolCalls).toBeUndefined()
    expect(reply.toolNames).toEqual([])
  })

  it('a FLEET PERSONA refuses them: the tool loop belongs to the agent, and the stream reports bare names', async () => {
    await expect(fleetTransport({ ...toolRequest, model: 'assistant-operations' })).rejects.toThrow(/tool loop runs inside the agent/)
    await expect(fleetStream({ ...toolRequest, model: 'assistant-operations' }, () => {})).rejects.toThrow(/tool loop runs inside the agent/)
    expect(relay).not.toHaveBeenCalled()
  })

  it('a STREAMING gateway turn refuses them too, rather than half-assembling a call out of deltas', async () => {
    await expect(gatewayStream(toolRequest, () => {})).rejects.toThrow(/STREAMING transport/)
    expect(relay).not.toHaveBeenCalled()
  })
})

// ── The reasoning-effort pick, from the composer to the provider ─────────────
//
// The chat surfaces offer an effort chip only when the model's catalog
// metadata vouches for levels, and the routes validate the pick against the
// same metadata. What THESE assertions hold is the second half of the
// contract: once a pick is legitimately in a `TransportRequest`, every
// transport that can honor it sends it — as `reasoning_effort` to a gateway
// model, as `model_options.reasoning_effort` to a persona (the only spelling
// Hermes reads) — and the one turn that cannot (a tool-offering gateway round)
// keeps the `'none'` the provider demands rather than the pick.

describe('the reasoning-effort pick', () => {
  /** A persona reply stream, as `pumpPersonaStream` reads it (no canned header). */
  const fleetBody = stream(['data: [DONE]\n\n'])

  it('travels to a persona gateway in model_options, and temperature does not travel at all', async () => {
    answers.push(fleetBody)
    const reply = await fleetTransport({
      model: 'engineer-engineering',
      messages: [{ role: 'user', content: 'hi' }],
      jsonMode: false,
      caller: 't',
      temperature: 0.4,
      effort: 'high',
    })
    const body = sent.at(-1)!.body
    expect(body.model_options).toEqual({ reasoning_effort: 'high' })
    expect('reasoning_effort' in body).toBe(false)
    // Hermes takes no per-request temperature: it is decided, not sent.
    expect('temperature' in body).toBe(false)
    expect(reply.contractDropped).toBe(false)
  })

  it('is absent on a persona turn that never picked one', async () => {
    answers.push(fleetBody)
    await fleetTransport({ model: 'engineer-engineering', messages: [], jsonMode: false, caller: 't' })
    const body = sent.at(-1)!.body
    expect('reasoning_effort' in body).toBe(false)
    expect('model_options' in body).toBe(false)
  })

  it('reports a persona JSON request as a dropped contract, never sending response_format', async () => {
    answers.push(fleetBody)
    const reply = await fleetTransport({ model: 'engineer-engineering', messages: [], jsonMode: true, caller: 't' })
    const body = sent.at(-1)!.body
    expect('response_format' in body).toBe(false)
    expect(reply.contractDropped).toBe(true)
    // A schema alone is a JSON contract too — the Rust twin's rule.
    answers.push(fleetBody)
    const schemaOnly = await fleetTransport({
      model: 'engineer-engineering',
      messages: [],
      jsonMode: false,
      jsonSchema: { name: 'x', schema: { type: 'object' }, strict: false },
      caller: 't',
    })
    expect(schemaOnly.contractDropped).toBe(true)
  })

  it('rides a streamed gateway turn', async () => {
    answers.push(fleetBody)
    await gatewayStream({ model: 'qwen3-14b', messages: [{ role: 'user', content: 'hi' }], jsonMode: false, caller: 't', effort: 'low' }, () => {})
    expect(sent[0]?.body.reasoning_effort).toBe('low')
    expect(sent[0]?.body.stream).toBe(true)
  })

  it('rides a blocking gateway turn', async () => {
    await gatewayTransport({ model: 'qwen3-14b', messages: [], jsonMode: false, caller: 't', effort: 'medium' })
    expect(sent[0]?.body.reasoning_effort).toBe('medium')
  })

  it('never reaches a turn that OFFERS tools — the provider accepts only none there', async () => {
    // The gpt-5.6-terra refusal names the whole conflict: function tools and a
    // reasoning effort cannot share a request on /v1/chat/completions. The
    // user's pick loses to the value that makes the tool loop work at all.
    answers.push(() => withToolCall('get_weather', '{}'))
    await gatewayTransport({ ...toolRequest, effort: 'high' })
    expect(sent[0]?.body.reasoning_effort).toBe('none')
  })
})

describe('offersToolDefinitions', () => {
  it('answers for the transport that would actually take the call', async () => {
    gatewayModels.mockResolvedValue([{ id: 'qwen3-14b' }])
    listAgents.mockResolvedValue(rendered('assistant-operations'))
    expect(await offersToolDefinitions('qwen3-14b')).toBe(true)
    // The probe reads this and SKIPS a persona candidate. Letting the refusal
    // throw instead would score as `errored`, which the probe suite reads as a
    // broken deployment — and a healthy persona is not one.
    expect(await offersToolDefinitions('assistant-operations')).toBe(false)
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

// ── The two turns that carry an IMAGE ────────────────────────────────────────
//
// RECONSTRUCTED CODE WITH NO TEST, which is why these exist. Both functions were
// rebuilt from transcripts after a `git checkout` destroyed the working copy,
// and nothing in the tree exercised either — the worst combination available:
// code nobody has read since it was rewritten, on a path a probe run takes.
//
// What they share is the rule worth locking. A `TransportRequest` has nowhere to
// put an image (`Message.content` is a string by construction), so both take
// images alongside the messages and both must attach them to the LAST USER TURN.
// Attaching to the system prompt is a shape providers accept and models ignore —
// it would not error, it would silently return an answer about nothing, and a
// vision probe would report the model as blind.

/** The wire body the relay last received. */
const lastBody = () => sent[sent.length - 1]!.body as { messages: Array<{ role: string; content: unknown }> }

describe('gatewayImageTurn', () => {
  beforeEach(() => {
    fallback = () => json({ choices: [{ message: { content: 'a cat' } }], usage: { prompt_tokens: 10, completion_tokens: 2 } })
  })

  it('puts the image on the last USER turn, not the system prompt', async () => {
    const out = await gatewayImageTurn(
      'gw-model',
      [
        { role: 'system', content: 'You describe images.' },
        { role: 'user', content: 'What is this?' },
      ],
      ['data:image/png;base64,AAAA'],
      'probe',
    )
    expect(out).toBe('a cat')
    const msgs = lastBody().messages
    // The system turn is untouched — still a plain string.
    expect(msgs[0]).toEqual({ role: 'system', content: 'You describe images.' })
    // The user turn became parts, text FIRST and the image after it.
    expect(msgs[1]!.role).toBe('user')
    expect(msgs[1]!.content).toEqual([
      { type: 'text', text: 'What is this?' },
      { type: 'image_url', image_url: { url: 'data:image/png;base64,AAAA' } },
    ])
  })

  it('uses the LAST user turn when there are several', async () => {
    await gatewayImageTurn(
      'gw-model',
      [
        { role: 'user', content: 'first' },
        { role: 'assistant', content: 'ok' },
        { role: 'user', content: 'second' },
      ],
      ['data:image/png;base64,BBBB'],
      'probe',
    )
    const msgs = lastBody().messages
    expect(msgs[0]!.content).toBe('first')
    expect(Array.isArray(msgs[2]!.content)).toBe(true)
  })

  it('falls back to the last message when there is no user turn at all', async () => {
    // A degenerate shape, but silently sending the image nowhere would make a
    // model look blind rather than make the caller look wrong.
    await gatewayImageTurn('gw-model', [{ role: 'system', content: 'only this' }], ['data:image/png;base64,CCCC'], 'probe')
    expect(Array.isArray(lastBody().messages[0]!.content)).toBe(true)
  })

  it('carries several images on the one turn', async () => {
    await gatewayImageTurn('gw-model', [{ role: 'user', content: 'compare' }], ['data:a', 'data:b'], 'probe')
    expect((lastBody().messages[0]!.content as unknown[]).length).toBe(3)
  })

  it('goes through the relay as the app, and reports what it refused', async () => {
    answers.push(() => json({ error: { message: 'unknown model "nope" — GET /v1/models' } }, { status: 404 }))
    await expect(gatewayImageTurn('nope', [{ role: 'user', content: 'x' }], [], 'probe')).rejects.toThrow(/404: unknown model/)
    expect(sent[0]!.headers['x-talaria-caller']).toBe('probe')
    expect(sent[0]!.body.stream).toBe(false)
  })
})
