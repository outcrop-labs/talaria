// THE TRANSPORTS, and the request that reaches them.
//
// WHY THIS FILE EXISTS — ONE MISSING FEATURE THAT WORE FIVE COATS
//   Five files USED TO hand-write their own persona transport because
//   `runHarness` could not serve them, and four agents working independently
//   converged on the same four asks:
//
//     work-dispatch.ts     `sessionTransport`          tools + taskId + a 10-minute hold
//     briefing.ts          a tee transport             tools + streaming
//     outreach.ts          `personaTurnWithOwnTools`   tools
//     plan-persona-turn.ts `planPersonaTransport`      refId + tier + tier routing
//     routes/api/muse.ts   a replay transport          streaming
//
//   Every one of them was `fleetTransport` differing on an axis
//   `TransportRequest` had no slot for. Those slots are here now: `tools`,
//   `ledger`, `holdMs`, and (for the streaming pair) `StreamingTransport` +
//   `runHarnessStreamed` in run.ts.
//
//   AND THE SIXTH ASK, which is `toolDefs` + `TransportReply.toolCalls`. It came
//   from the fitness suite rather than from a shim, and it is the same shape of
//   gap: `TransportRequest` carried a tool POLICY ("may this model use its OWN
//   tools") and had no slot for tool DEFINITIONS, so the `tool-select` probe —
//   four tools, four prompts, one right answer each — could not be run, could
//   not write its fact, and the capability-gated widening it gates (audit 1.8,
//   the Inbox handing a capable model the item's whole action list) could never
//   fire in production. The honest workaround would have been none: asking a
//   model in prose which tool it WOULD call measures instruction following, and
//   recording that as `tools: true` is the false positive the probe suite exists
//   to avoid. So the slot is here, and the three transports that cannot serve it
//   refuse the call.
//
//   ALL FIVE ARE DELETED — plan-persona-turn.ts as a whole file, the other four
//   as declarations plus a plain runner call. The table above is history, kept
//   because it is the argument for putting the sixth ask HERE: this is a list of
//   what happens when a capability has no home, not a list of code to go and
//   read. `scripts/check-invariants.mjs` no longer carries a shim category, so
//   nothing outside this file may reach a model on the fleet's behalf again.
//
// AND THE REASON THE SHIMS WERE A PROBLEM RATHER THAN A STYLE COMPLAINT: THREE
// OF THE FIVE SILENTLY DROPPED `req.temperature` AND `req.jsonMode`. A harness
// that declared `temperature: 0` ran at the provider's default and nothing
// anywhere said so — the exact class of failure the whole harness layer exists
// to end, reintroduced by the workaround for the harness layer. So the mapping
// from a request onto a persona payload lives in ONE function here
// (`personaPayload`), every persona transport in the tree goes through it, and a
// transport that genuinely CANNOT honor a field says so by throwing
// (`gatewayTransport` and `tools: 'own'`) rather than by ignoring it.
import { parseAgentStream } from '@/lib/sse-parse'
import { buildUpstream, completeViaGateway, contractDropsOf, fetchUpstream, gatewayModels, recordGatewayUsage, resolveRoute, type ResponseFormat } from '../llm-gateway'
import type { WireSchema } from './json-schema'
import { CANNED_STREAM_HEADER, listAgents, proxyChat } from '../gateway'
import { estimateTokens, recordUsage } from '../usage'
import type { Message } from './define'

export type TransportKind = 'gateway' | 'fleet'

/** Whether the model may use ITS OWN tools on this turn.
 *
 *  'none' is the default and is right for every single-shot structured harness:
 *  the runner asks one question and parses one answer, so a tool call can only
 *  be the model wandering off. It is sent as an OpenAI-level `tools: []` /
 *  `tool_choice: 'none'` rather than left to the prompt, which is what
 *  `inbox-focus-assistant.ts` did by hand for exactly that reason.
 *
 *  'own' is for the three turns whose whole FEATURE is the tool loop — an agent
 *  working a ticket, an outreach check-in that acts through `message_user`, a
 *  briefing follow-up the owner expects to answer "what's blocking t-41?" from
 *  live data. On those, 'none' does not merely weaken the answer: it disarms the
 *  agent and then trips `zero_tool_claim` for having called no tool.
 *
 *  IT IS NOT THE SAME QUESTION AS `TransportRequest.toolDefs`, and conflating
 *  the two would have been the easy mistake. This policy is about tools WE DO
 *  NOT HOLD — a persona's loop runs inside the agent container, so all we can do
 *  is suppress it at the protocol level and read the names the stream reports.
 *  `toolDefs` is about tools we hand over ourselves and watch being called. A
 *  turn may legitimately offer definitions while still refusing the model its
 *  own loop; that is two sentences about two different sets of tools. */
export type ToolPolicy = 'none' | 'own'

/** One tool we OFFER the model for a single turn: the OpenAI function shape
 *  without the envelope, which every transport that can serve it puts back on. */
export interface ToolDefinition {
  name: string
  description: string
  /** JSON Schema for the arguments, exactly as the provider expects it. */
  parameters: Record<string, unknown>
}

/** A tool call the model MADE, as the provider reported it. */
export interface ToolCall {
  name: string
  /** THE PROVIDER'S OWN ID FOR THIS CALL, when it gave one. A tool RESULT has to
   *  name the call it answers (`tool_call_id`), and a loop that invents an id —
   *  or reuses the tool name — desynchronises the moment a model calls the same
   *  tool twice in one turn. Optional because not every provider emits one and a
   *  missing id is better than a fabricated one. */
  id?: string
  /** The raw JSON arguments string, verbatim. Kept unparsed because "the model
   *  called the right tool with arguments that are not JSON" is a distinct and
   *  real observation, and parsing here would turn it into a throw. */
  args: string
}

/** Where this turn's spend lands in `usage_events`.
 *
 *  `runHarness` used to meter every persona turn as `{ source: 'chat', refId:
 *  null, tier: null }`, because no other harness had anything to attribute to.
 *  Three call sites did, and each lost something real in the port:
 *
 *    taskId   a work-session turn's spend lands in the ledger but never in the
 *             TICKET'S cost, which is the number a ticket owner reads.
 *    refId +  research's persona stages metered as `source: 'chat'` with no run
 *    source   id, so a run's cost stopped being answerable at all.
 *    tier     `recordUsage` prices a row by looking up `agent_defs.model =
 *             agentModel` and then the alias named by `tier`. Hand it the routed
 *             id "dex-developer-opus" with a null tier and BOTH lookups miss:
 *             the row is attributed to an agent that does not exist, with no
 *             endpoint class, which means no price. A tier draft becomes free.
 *
 *  Hence `agentModel` is the BASE persona id and `tier` is the alias NAME, never
 *  the routed id — `TransportRequest.model` is the id actually called. */
export interface LedgerAttribution {
  /** The agent as the ledger names it: the base persona id, never a tier id. */
  agentModel: string
  source: 'chat' | 'channel' | 'ticket' | 'research'
  /** The conversation, channel or research run this spend belongs to. */
  refId: string | null
  /** The ticket this spend belongs to, so it reaches the ticket's cost. */
  taskId: string | null
  /** The alias NAME (`opus`), as `classifyAgent` wants it. */
  tier: string | null
}

export interface TransportRequest {
  /** The id actually called — a tier id when a tier was routed. */
  model: string
  messages: Message[]
  temperature?: number
  /** Ask for structured output at the PROTOCOL level (response_format). False
   *  means the runner has already anchored the instruction in the prompt
   *  instead - see `anchorJson`. */
  jsonMode: boolean
  /** THE HARNESS'S OWN SCHEMA, when this build could render one. Present, the
   *  request carries `response_format: { type: 'json_schema', ... }` and the
   *  provider constrains decoding to the shape; absent, it falls back to the
   *  loose `{ type: 'json_object' }`.
   *
   *  Only the fallback is optional — every JSON harness Talaria ships renders a
   *  schema (`json-schema.test.ts` asserts it), so the loose form is reserved
   *  for a definition this build cannot express. It matters because the loose
   *  form is not merely weaker: Anthropic's compat layer REJECTS it. */
  jsonSchema?: WireSchema
  /** Absent means 'none'. Read it through `toolPolicyOf`, never by hand. */
  tools?: ToolPolicy
  /** Tool DEFINITIONS this turn offers the model — OURS, not its own. See the
   *  note on `ToolPolicy` for why this is a separate slot rather than a wider
   *  policy: the policy governs a loop we do not hold, this governs one we do.
   *
   *  WHY IT EXISTS AT ALL. `tool-select` — four tools, four prompts, one right
   *  answer each — is the fact that widens the Inbox command harness from a
   *  regex-chosen single action to the item's whole action list (audit 1.8), and
   *  it was unrecordable on every build before this slot: there was nowhere to
   *  put a tool definition, so the probe skipped forever and the widening
   *  feature could never fire in production. A prompt-level imitation ("reply
   *  with the name of the tool you would call") measures instruction following,
   *  not tool calling, and recording its result would have been exactly the
   *  false `true` the probe suite is built to avoid.
   *
   *  A TRANSPORT THAT CANNOT OFFER THEM FAILS THE CALL — `fleetToolDefsRefusal`
   *  when the loop is the agent's, `toolDefsDroppedRefusal` when the gateway
   *  stripped the parameter on the way out. Same rule as `gatewayToolsRefusal`,
   *  for a sharper reason: a probe scored on a question the model was never
   *  asked writes a capability fact that never expires.
   *
   *  Read it through `toolDefsOf`, never by hand. */
  toolDefs?: ToolDefinition[]
  /** Absent means the default persona attribution. Read it through `ledgerOf`. */
  ledger?: LedgerAttribution
  /** How long a persona transport may HOLD for an agent that is not answering
   *  yet, in ms. `proxyChat` defaults to two minutes; a restarting agent under a
   *  config propagation refuses connections for tens of seconds, and a work
   *  session must survive a fleet re-render mid-session, so `work-session`
   *  declares ten minutes. Meaningless on the gateway path — there is no
   *  container to wait for — which is why ignoring it there is not a dropped
   *  field. */
  holdMs?: number
  caller: string
  signal?: AbortSignal
}

export interface TransportReply {
  kind: TransportKind
  text: string
  /** The tool NAMES this turn produced. On the fleet path that is all there is:
   *  the persona's tool loop runs inside the agent and the stream reports names.
   *  On the gateway path it is `toolCalls` with the arguments dropped, derived
   *  through `toolNamesOf` so the two cannot drift apart. */
  toolNames: string[]
  /** The tool calls the model made, WITH their arguments — filled only by a
   *  transport that ran the loop ITSELF, which today means the gateway answering
   *  a request that carried `toolDefs`.
   *
   *  ABSENT IS NOT EMPTY, and the difference is what the tool probes score.
   *  Undefined means nobody was in a position to observe a call (a persona's
   *  loop, a plain completion that offered nothing); `[]` means we offered tools
   *  and the model called none, which is a failed trial rather than a missing
   *  measurement. `toolNames` above is the same information minus the arguments,
   *  so a transport fills both or neither. */
  toolCalls?: ToolCall[]
  usage: { promptTokens: number; completionTokens: number } | null
  /** The call asked for JSON at the protocol level and the constraint did not
   *  survive to the upstream (audit 1.2). Honored, not ignored: the runner
   *  stops trusting json mode for the rest of the run and anchors the
   *  instruction in the prompt instead. */
  contractDropped: boolean
}

export type Transport = (req: TransportRequest) => Promise<TransportReply>

/** A transport that STREAMS. Same request every transport gets, plus an `emit`
 *  it must call with each delta AS THE DELTA ARRIVES — that call is the whole
 *  difference between a stream and a slow blocking call — resolving with the
 *  completed reply when the stream ends.
 *
 *  The runner accumulates what it is emitted, so a transport that pumps into a
 *  browser and never assembles the text itself may resolve with `text: ''` and
 *  the run still gets its guard pass over the whole reply. */
export type StreamingTransport = (req: TransportRequest, emit: (delta: string) => void) => Promise<TransportReply>

// ── Reading a request without dropping half of it ────────────────────────────

export const toolPolicyOf = (req: TransportRequest): ToolPolicy => req.tools ?? 'none'

/** The definitions this turn offers, never null. An empty list and an absent one
 *  are the same request — nothing was offered — which is why every refusal below
 *  is written against `.length` rather than against the field. */
export const toolDefsOf = (req: TransportRequest): ToolDefinition[] => req.toolDefs ?? []

/** `toolNames` from `toolCalls`, in one place, so a reply cannot report a call
 *  under one field and not the other. */
export const toolNamesOf = (calls: readonly ToolCall[]): string[] => calls.map((c) => c.name)

/** The default attribution, which is exactly what the shared fleet transport
 *  metered before this slot existed: a harness turn on a persona IS a chat turn
 *  with that persona, belonging to no conversation. */
export const ledgerOf = (req: TransportRequest): LedgerAttribution =>
  req.ledger ?? { agentModel: req.model, source: 'chat', refId: null, taskId: null, tier: null }

/** `proxyChat`'s payload shape. Structural rather than imported because
 *  `gateway.ts` keeps `ChatPayload` private and widening its export surface for
 *  one caller is not worth it; the index signature is the same one it has. */
export interface PersonaPayload {
  model: string
  messages: Array<{ role: string; content: string }>
  [k: string]: unknown
}

/** THE ONE MAPPING from a `TransportRequest` onto a persona-gateway payload.
 *
 *  Every field of the request is spent here, and that is the point: three of the
 *  five hand-written persona transports dropped `temperature` and `jsonMode`
 *  because each was written by copying `fleetTransport` and then editing the one
 *  axis its author cared about. A harness that declares `temperature: 0` and
 *  runs at the provider's 0.7 is a harness whose declaration is decorative, and
 *  nothing in the result, the row or the logs says so. There is now one place to
 *  get this right and one place to change it. */
export function personaPayload(req: TransportRequest): PersonaPayload {
  const payload: PersonaPayload = {
    model: req.model,
    messages: req.messages.map((m) => ({ role: m.role, content: m.content })),
  }
  // Suppressing the tools WE offer cannot reach the persona's own internal loop
  // — which is exactly why the guard pass still runs `zero_tool_claim` over the
  // tool names the stream reported — but it is real, and it belongs here rather
  // than in one caller.
  if (toolPolicyOf(req) === 'none') {
    payload.tools = []
    payload.tool_choice = 'none'
  }
  if (req.temperature !== undefined) payload.temperature = req.temperature
  if (req.jsonMode) payload.response_format = { type: 'json_object' }
  return payload
}

// ── Pumping and metering a persona turn ──────────────────────────────────────

export interface PersonaTurn {
  text: string
  toolNames: string[]
  usage: { promptTokens: number; completionTokens: number } | null
}

/** Drain a persona stream into text, tool names and usage, emitting each delta
 *  on the way past. Five files spelled this loop out; the `emit` parameter is
 *  the only thing the streaming ones did differently. */
export async function pumpPersonaStream(body: ReadableStream<Uint8Array>, emit?: (delta: string) => void): Promise<PersonaTurn> {
  let text = ''
  const toolNames: string[] = []
  let usage: { promptTokens: number; completionTokens: number } | null = null
  for await (const ev of parseAgentStream(body)) {
    if (ev.type === 'content') {
      text += ev.text
      emit?.(ev.text)
    } else if (ev.type === 'tool') toolNames.push(ev.name)
    else if (ev.type === 'usage') usage = { promptTokens: ev.promptTokens, completionTokens: ev.completionTokens }
  }
  return { text, toolNames, usage }
}

/** The ledger row for a persona turn. It meters because nothing else will: a
 *  harness turn on a persona writes no chat, channel or ticket row, so this is
 *  the only place the spend can enter. */
export async function meterPersonaTurn(req: TransportRequest, turn: PersonaTurn): Promise<void> {
  const led = ledgerOf(req)
  const promptChars = req.messages.reduce((n, m) => n + m.content.length, 0)
  await recordUsage({
    agentModel: led.agentModel,
    source: led.source,
    refId: led.refId,
    taskId: led.taskId,
    tier: led.tier,
    promptTokens: turn.usage?.promptTokens ?? estimateTokens(promptChars),
    completionTokens: turn.usage?.completionTokens ?? estimateTokens(turn.text.length),
    estimated: !turn.usage,
  }).catch(() => {})
}

// ── The two real transports ──────────────────────────────────────────────────

/** Non-streaming completion over the ORG gateway: routing, provider keys and
 *  the ledger row are all its business, so this transport does not meter.
 *
 *  Three things travel on this call and each one closes an audit finding:
 *    `responseFormat` constrains decoding at the protocol level (1.3 - the slot
 *                     whose absence made inbox-focus grow a second, weaker
 *                     request helper). The prompt anchor goes out too; they are
 *                     belt and braces, not alternatives.
 *    `guard: false`   this runner does the guard pass itself, with the
 *                     harness's narrowed rule set and an honest `Available` for
 *                     the transport that ran. Leaving `guardCompletion` on
 *                     underneath would file two guard_findings rows for one
 *                     reply and inflate the per-model confabulation rate the
 *                     fitness page reads.
 *    `contractDrops`  whether the constraint actually reached the model (1.2).
 *                     Only a JSON drop concerns THIS call: a dropped `tools`
 *                     parameter is a real fact about the model and the gateway
 *                     has recorded it as one, but it cannot have changed the
 *                     shape of a completion that requested no tools. A turn that
 *                     DID offer tools goes through `gatewayToolTurn` below,
 *                     where the same drop fails the call outright. */
/** AN ERROR, NOT A SILENT NO-OP, and this is the rule the whole request type is
 *  built on: a field that cannot be honored fails the call. Dropping `tools:
 *  'own'` would run a tool-loop harness as a single-shot completion, which does
 *  not read as broken — it reads as an agent that decided not to act, and then
 *  gets flagged `zero_tool_claim` for saying it did something. */
export const gatewayToolsRefusal = (model: string): string =>
  `harness turn on "${model}" asked for the model's own tools, but that model is served by the ORG GATEWAY, ` +
  'which runs no tool loop. Pin a fleet persona for this harness, or declare tools: none.'

/** THE SECOND REFUSAL, and it is the one the audit's rule was written for: the
 *  gateway learned (or has just been told by a 400) that this endpoint rejects
 *  `tools`, stripped the parameter, and answered anyway. The call SUCCEEDED and
 *  the reply is a perfectly ordinary completion — of a question that no longer
 *  mentioned any of the tools we were asking the model to choose between.
 *
 *  Answering that quietly is precisely audit 1.2 in the tool-calling clothes:
 *  `scoreTools` would read "called no tool" off a turn where no tool was ever
 *  offered and write `tools: false` — permanently, since probe facts do not
 *  expire — about a model that may well call tools perfectly. */
export const toolDefsDroppedRefusal = (model: string, endpoint: string): string =>
  `harness turn on "${model}" offered the model tool definitions, but endpoint "${endpoint}" rejects the "tools" parameter, ` +
  'so the call would have reached the model without them. Refusing rather than answering a question the model was never asked.'

/** A streamed turn cannot offer tools either, for a duller reason than the other
 *  three: nothing assembles tool-call deltas on this path and nothing needs to. */
export const streamToolDefsRefusal = (model: string): string =>
  `harness turn on "${model}" offered tool definitions on a STREAMING transport, which assembles text deltas only. ` +
  'Run a tool-offering turn through the blocking transport.'

/** THE THIRD REFUSAL. A persona's tool loop belongs to the agent: `proxyChat`
 *  hands the payload to the agent's own gateway, which runs Hermes' tool loop
 *  with the agent's own tools, and `parseAgentStream` reports tool NAMES with no
 *  arguments and no way to tell ours from its own. So definitions we sent are
 *  neither guaranteed to reach the model nor observable when they do, which is
 *  the exact shape of "a field that cannot be honored". The tool probes read
 *  this through `offersToolDefinitions` and SKIP a persona candidate rather than
 *  scoring one — a skip is honest, a `false` here would not be. */
export const fleetToolDefsRefusal = (model: string): string =>
  `harness turn on "${model}" offered tool definitions, but that model is a FLEET PERSONA: its tool loop runs inside the agent ` +
  'container, so tools we offer cannot be guaranteed to reach it and the calls it makes come back as bare names. ' +
  'Offer tool definitions to a gateway model, or declare none.'

/** The OpenAI wire shape for one offered tool. Kept here, in the file that owns
 *  the request, rather than at the two call sites that build a body. */
const toolWireShape = (def: ToolDefinition): Record<string, unknown> => ({
  type: 'function',
  function: { name: def.name, description: def.description, parameters: def.parameters },
})

interface WireToolCall {
  id?: string
  function?: { name?: string; arguments?: string }
}

/** What the model called, out of a completion body. A call with no name is not a
 *  call we can score or guard on, so it is dropped rather than carried as an
 *  empty string that every reader then has to defend against. */
const readToolCalls = (raw: WireToolCall[] | undefined): ToolCall[] =>
  (raw ?? []).flatMap((tc) => (tc.function?.name ? [{ name: tc.function.name, args: tc.function.arguments ?? '', ...(tc.id ? { id: tc.id } : {}) }] : []))

/** The gateway call that OFFERS TOOLS, and the reason it is not
 *  `completeViaGateway`: that helper's signature has no slot for tool
 *  definitions and its return type is a string, so it can neither ask the
 *  question nor report the answer. This walks the same route the same way
 *  `gatewayStream` does — `buildUpstream` for the provider key and the
 *  request_defaults merge, `fetchUpstream` for the 400-recovery loop and the
 *  contract-drop record, `recordGatewayUsage` for the ledger row that helper
 *  writes itself.
 *
 *  `tool_choice: 'auto'` rather than the policy's `'none'`: `ToolPolicy` governs
 *  the model's OWN tools and this turn is about ours, so suppressing them here
 *  would send four definitions and forbid calling any of them. */
async function gatewayToolTurn(req: TransportRequest, defs: ToolDefinition[]): Promise<TransportReply> {
  const route = await resolveRoute(req.model)
  if (!route) throw new Error(`model "${req.model}" is not on the gateway`)
  const body: Record<string, unknown> = {
    model: req.model,
    // `toolWireMessage`, NOT a flatten to `{role, content}`. THIS IS THE TURN
    // THAT OFFERS TOOLS, so it is the one turn guaranteed to be replaying a
    // conversation that CONTAINS tool calls — and flattening dropped
    // `toolCalls`/`toolCallId` on the floor, leaving a caller no way to show the
    // model what it had already called except by narrating it into prose. Models
    // imitate whatever the transcript shows them: `research-search` narrated
    // `Called web_search({...})` and got that string back as the model's final
    // answer on a live sweep. See the note on `Message.toolCalls`.
    messages: req.messages.map(toolWireMessage),
    stream: false,
    tools: defs.map(toolWireShape),
    tool_choice: 'auto',
  }
  if (req.temperature !== undefined) body.temperature = req.temperature
  const toolFormat = responseFormatOf(req)
  if (toolFormat) body.response_format = toolFormat
  const call = await buildUpstream(route, body)
  const res = await fetchUpstream(call, route)
  if (!res.ok) throw new Error(`gateway completion ${res.status}: ${(await res.text().catch(() => '')).slice(0, 300)}`)
  // BEFORE THE BODY IS READ, because a dropped `tools` parameter makes the body
  // an answer to a different question. `buildUpstream` pre-strips a remembered
  // rejection and `fetchUpstream` strips a live one, and both record the drop on
  // the call — so this one check covers the remembered case and the 400 case.
  const drops = contractDropsOf(call)
  if (drops.some((d) => d.capability === 'tools')) throw new Error(toolDefsDroppedRefusal(req.model, route.endpoint.name))

  const j = (await res.json()) as {
    choices?: Array<{ message?: { content?: string; tool_calls?: WireToolCall[] } }>
    usage?: { prompt_tokens?: number; completion_tokens?: number }
  }
  const message = j.choices?.[0]?.message
  const text = message?.content ?? ''
  const toolCalls = readToolCalls(message?.tool_calls)
  const usage = j.usage ? { promptTokens: j.usage.prompt_tokens ?? 0, completionTokens: j.usage.completion_tokens ?? 0 } : null
  const promptChars = req.messages.reduce((n, m) => n + m.content.length, 0)
  await recordGatewayUsage({
    caller: req.caller,
    endpoint: route.endpoint,
    upstreamModel: route.upstreamModel,
    promptTokens: usage?.promptTokens ?? estimateTokens(promptChars),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  }).catch(() => {})
  return {
    kind: 'gateway',
    text,
    toolNames: toolNamesOf(toolCalls),
    toolCalls,
    usage,
    contractDropped: drops.some((d) => d.capability === 'json'),
  }
}

export const gatewayTransport: Transport = async (req) => {
  if (toolPolicyOf(req) === 'own') throw new Error(gatewayToolsRefusal(req.model))
  const defs = toolDefsOf(req)
  if (defs.length) return gatewayToolTurn(req, defs)
  const res = await completeViaGateway(req.model, req.messages, {
    ...(req.temperature !== undefined ? { temperature: req.temperature } : {}),
    caller: req.caller,
    ...(responseFormatOf(req) ? { responseFormat: responseFormatOf(req)! } : {}),
    guard: false,
  })
  const droppedJson = res.contractDrops.some((d) => d.capability === 'json')
  return { kind: 'gateway', text: res.text, toolNames: [], usage: null, contractDropped: droppedJson }
}

/** The `response_format` this request should carry, prefering the harness's
 *  schema over the loose object form. One definition, four call sites — the
 *  four used to each hardcode `{ type: 'json_object' }`, which is how every
 *  structured call to Anthropic came to 400. */
export const responseFormatOf = (req: TransportRequest): ResponseFormat | null => {
  if (!req.jsonMode) return null
  // STRICT OR NOTHING, and this is the second thing a live run taught that no
  // amount of local testing would have. `strict: false` is not a weaker request
  // every provider accepts — Anthropic rejects it outright ("json_schema.strict:
  // Input should be True"), so a schema that cannot be sent strictly cannot be
  // sent to Anthropic AT ALL. Sending it anyway 400'd three harnesses on every
  // call, and the fitness suite scored that as the model failing its contract.
  //
  // So a non-strict-eligible schema falls back to `json_object` rather than
  // going out as a request some providers refuse. On a provider that also
  // refuses `json_object` the run still succeeds — `run.ts` carries the JSON
  // anchor in the prompt on every structured call, and the gateway's learned
  // parameter ratchet drops the format after the first refusal.
  if (req.jsonSchema?.strict) {
    return { type: 'json_schema', json_schema: { name: req.jsonSchema.name, schema: req.jsonSchema.schema, strict: true } }
  }
  // AN ARRAY CONTRACT MUST NOT ASK FOR `json_object`, and this cost a harness.
  //
  // `json_object` is not "reply in JSON" — it is defined as "the model MUST
  // return a JSON OBJECT", and providers constrain decoding to enforce it. Send
  // it alongside a schema rooted at an array and the request is self-defeating:
  // the wire format forbids the only answer the contract accepts.
  //
  // `channel-plan` is that harness — a top-level `[{...}, ...]` of ticket
  // proposals, non-strict (nullable enums, a nested array), so it took this
  // fallback on every call. A sweep of deepseek-v4-flash failed five of its
  // fixtures with `expected array, got object`, INCLUDING after the repair turn,
  // and every one of those was scored against the model. The model was right and
  // did what we asked; the request was wrong.
  //
  // So an array root sends NO `response_format` at all. That is not a downgrade
  // from a working state — it is the removal of an instruction that could only
  // ever produce a contract violation. `run.ts` anchors the JSON instruction in
  // the prompt on every structured call (`anchorJson`) and the harness parses
  // and repairs, which is exactly the path a provider that refuses the format
  // outright already takes.
  if (req.jsonSchema && rootType(req.jsonSchema.schema) !== 'object') return null
  return { type: 'json_object' }
}

/** The declared top-level type of a wire schema, when it declares one. Only
 *  'object' may be asked for as `json_object`; everything else — an array root
 *  today, a `oneOf` root tomorrow — is a shape that mode cannot express. */
const rootType = (schema: unknown): string | null => {
  if (typeof schema !== 'object' || schema === null) return null
  const t = (schema as { type?: unknown }).type
  return typeof t === 'string' ? t : null
}

/** ONE MESSAGE, on the wire, INCLUDING its tool channel.
 *
 *  An assistant turn that called tools carries `tool_calls`; a result carries
 *  `role: 'tool'` and the id it answers. This is the shape every OpenAI-compatible
 *  provider speaks and, more to the point, the shape models are TRAINED on — a
 *  tool conversation replayed as prose is a conversation they will answer in
 *  prose. See the note on `Message.toolCalls`: 34 replies in one sweep came back
 *  containing Talaria's own narration of a call, because that is what the
 *  transcript had shown them.
 *
 *  A message with neither field renders exactly as before. */
export const toolWireMessage = (m: Message): Record<string, unknown> => {
  if (m.role === 'tool') return { role: 'tool', content: m.content, tool_call_id: m.toolCallId ?? '' }
  if (!m.toolCalls?.length) return { role: m.role, content: m.content }
  return {
    role: m.role,
    content: m.content,
    tool_calls: m.toolCalls.map((c, i) => ({
      id: c.id ?? `call_${i}`,
      type: 'function',
      function: { name: c.name, arguments: c.args },
    })),
  }
}

/** ONE TURN THAT CARRIES AN IMAGE, which the ordinary transports cannot.
 *
 *  `Message.content` is a string by construction (see the note in define.ts), so
 *  there is nowhere in a `TransportRequest` to put image bytes. Rather than widen
 *  every message in the tree into a content-parts union for one caller, the image
 *  rides its own function: the runner still resolves the model, applies the
 *  capability floor and meters the call, and this sends the one payload shape
 *  that carries pixels.
 *
 *  Used by `vision.ts` — see that file for why describing an image is a harness
 *  rather than a raw call. */
export async function gatewayImageTurn(
  model: string,
  messages: Message[],
  images: readonly string[],
  caller: string,
  opts: { timeoutMs?: number } = {},
): Promise<string> {
  const route = await resolveRoute(model)
  if (!route) throw new Error(`model "${model}" is not on the gateway`)
  // The LAST user turn carries the image, because that is the turn the question
  // is in — a system prompt with pictures attached is a shape providers accept
  // and models ignore.
  const wire = messages.map((m) => ({ role: m.role, content: m.content as unknown }))
  const lastUser = wire.map((m) => m.role).lastIndexOf('user')
  const target = lastUser >= 0 ? lastUser : wire.length - 1
  if (target >= 0 && wire[target]) {
    wire[target] = {
      role: wire[target]!.role,
      content: [
        { type: 'text', text: String(wire[target]!.content ?? '') },
        ...images.map((url) => ({ type: 'image_url', image_url: { url } })),
      ],
    }
  }
  const call = {
    ...(await buildUpstream(route, { model, messages: wire, stream: false })),
    ...(opts.timeoutMs !== undefined ? { timeoutMs: opts.timeoutMs } : {}),
  }
  const res = await fetchUpstream(call, route)
  if (!res.ok) throw new Error(`gateway completion ${res.status}: ${(await res.text().catch(() => '')).slice(0, 300)}`)
  const j = (await res.json()) as {
    choices?: Array<{ message?: { content?: string } }>
    usage?: { prompt_tokens?: number; completion_tokens?: number }
  }
  const text = j.choices?.[0]?.message?.content ?? ''
  await recordGatewayUsage({
    caller,
    endpoint: route.endpoint,
    upstreamModel: route.upstreamModel,
    promptTokens: j.usage?.prompt_tokens ?? estimateTokens(messages.reduce((n, m) => n + m.content.length, 0)),
    completionTokens: j.usage?.completion_tokens ?? estimateTokens(text.length),
    estimated: !j.usage,
  }).catch(() => {})
  return text
}

/** A PERSONA TURN THE PROBE SUITE CAN DRIVE — one blocking round trip against a
 *  fleet agent, returning the assembled turn rather than a stream.
 *
 *  WHY IT IS NOT `fleetTransport`. Two reasons, and the second is the load-bearing
 *  one. A probe wants the raw turn — text AND the tool names the persona reported
 *  — without a harness's contract wrapped around it. And a probe may carry an
 *  IMAGE, which a `TransportRequest` has nowhere to put (`Message.content` is a
 *  string by construction; see the note in define.ts). So this takes messages and
 *  images directly, exactly as `gatewayImageTurn` does on the other side of the
 *  `offersToolDefinitions` fork, and the vision probe can be asked of a persona
 *  and a gateway model with the same call shape. */
export async function personaProbeTurn(
  model: string,
  messages: Message[],
  opts: { images?: readonly string[]; caller: string; timeoutMs?: number } = { caller: 'probe' },
): Promise<PersonaTurn> {
  const images = opts.images ?? []
  const wire: Array<{ role: string; content: unknown }> = messages.map((m) => ({ role: m.role, content: m.content }))
  if (images.length > 0) {
    // The last user turn carries the image: it is the turn the question is in,
    // and a system prompt with pictures attached is a shape providers accept and
    // models ignore.
    const lastUser = wire.map((m) => m.role).lastIndexOf('user')
    const target = lastUser >= 0 ? lastUser : wire.length - 1
    const at = wire[target]
    if (at) {
      wire[target] = {
        role: at.role,
        content: [{ type: 'text', text: String(at.content ?? '') }, ...images.map((url) => ({ type: 'image_url', image_url: { url } }))],
      }
    }
  }
  const upstream = await proxyChat(
    { model, messages: wire } as unknown as Parameters<typeof proxyChat>[0],
    opts.timeoutMs !== undefined ? { waitMs: opts.timeoutMs } : {},
  )
  if (!upstream.ok || !upstream.body) throw new Error(`persona gateway ${upstream.status}`)
  const canned = upstream.headers.get(CANNED_STREAM_HEADER)
  if (canned) {
    await upstream.body.cancel().catch(() => {})
    throw new Error(`"${model}" is not a rendered agent (the fleet answered in mock mode)`)
  }
  return pumpPersonaStream(upstream.body)
}

/** The gateway transport, STREAMED — the sibling of `fleetStream`, and the piece
 *  that makes "a harness may stream" true of both transports rather than only of
 *  personas. The Muse's six prose kinds draft against an ORG GATEWAY model, so
 *  without this they would keep a hand-written `buildUpstream` + `fetchUpstream`
 *  pair however good `runHarnessStreamed` got.
 *
 *  `completeViaGateway` cannot serve it: it asks for `stream: false` and hands
 *  back a finished string, which is the one thing a streaming surface must not
 *  wait for. So this walks the same route, asks for `stream: true`, and emits
 *  each delta as it lands — metering exactly as `completeViaGateway` does, and
 *  reporting the same `contractDropped` signal (audit 1.2) from the same
 *  `contractDropsOf`, so a dropped `response_format` is as visible here as it is
 *  on the blocking path. */
export const gatewayStream: StreamingTransport = async (req, emit) => {
  if (toolPolicyOf(req) === 'own') throw new Error(gatewayToolsRefusal(req.model))
  // A STREAMED TURN OFFERS NO TOOLS. Assembling tool calls out of deltas is a
  // second, fiddlier parser for a case no surface has: the streaming harnesses
  // are the Muse's prose kinds and the briefing follow-up, and the one caller
  // that needs tool definitions (the probe) is blocking by construction. Better
  // to refuse the field here than to grow a parser that nothing exercises and
  // that would report a half-assembled call as a real one.
  if (toolDefsOf(req).length) throw new Error(streamToolDefsRefusal(req.model))
  const route = await resolveRoute(req.model)
  if (!route) throw new Error(`model "${req.model}" is not on the gateway`)
  const body: Record<string, unknown> = {
    model: req.model,
    messages: req.messages.map((m) => ({ role: m.role, content: m.content })),
    stream: true,
  }
  if (req.temperature !== undefined) body.temperature = req.temperature
  const toolFormat = responseFormatOf(req)
  if (toolFormat) body.response_format = toolFormat
  const call = await buildUpstream(route, body)
  const res = await fetchUpstream(call, route)
  if (!res.ok || !res.body) throw new Error(`gateway completion ${res.status}: ${(await res.text().catch(() => '')).slice(0, 300)}`)

  let text = ''
  let usage: { promptTokens: number; completionTokens: number } | null = null
  let buffered = ''
  const decoder = new TextDecoder()
  const reader = res.body.getReader()
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buffered += decoder.decode(value, { stream: true })
    const lines = buffered.split('\n')
    // The last element is a partial line: keep it for the next chunk. Splitting
    // on '\n' and parsing every piece is how a JSON frame gets torn in half.
    buffered = lines.pop() ?? ''
    for (const line of lines) {
      const data = line.startsWith('data:') ? line.slice(5).trim() : null
      if (!data || data === '[DONE]') continue
      let frame: { usage?: { prompt_tokens?: number; completion_tokens?: number } | null; choices?: Array<{ delta?: { content?: string } }> }
      try {
        frame = JSON.parse(data) as typeof frame
      } catch {
        continue
      }
      if (frame.usage) usage = { promptTokens: frame.usage.prompt_tokens ?? 0, completionTokens: frame.usage.completion_tokens ?? 0 }
      const piece = frame.choices?.[0]?.delta?.content
      if (piece) {
        text += piece
        emit(piece)
      }
    }
  }

  const promptChars = req.messages.reduce((n, m) => n + m.content.length, 0)
  await recordGatewayUsage({
    caller: req.caller,
    endpoint: route.endpoint,
    upstreamModel: route.upstreamModel,
    promptTokens: usage?.promptTokens ?? estimateTokens(promptChars),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  }).catch(() => {})
  return { kind: 'gateway', text, toolNames: [], usage, contractDropped: contractDropsOf(call).some((d) => d.capability === 'json') }
}

/** Streaming completion against a FLEET PERSONA's own gateway. The persona
 *  runs a tool loop we do not control, so this transport collects the tool
 *  names the stream reports and nothing more - which is precisely what the
 *  guard pass is told about it later (`guardChatReply` is the precedent). */
async function personaTurn(req: TransportRequest, emit?: (delta: string) => void): Promise<TransportReply> {
  // See `fleetToolDefsRefusal`: the loop on the other side of `proxyChat` is the
  // agent's, so a definition we send is neither guaranteed to arrive nor visible
  // when it is called. Refused here rather than in the two exported transports
  // so the streamed persona path cannot grow a quiet exception.
  if (toolDefsOf(req).length) throw new Error(fleetToolDefsRefusal(req.model))
  const upstream = await proxyChat(personaPayload(req), {
    ...(req.holdMs !== undefined ? { waitMs: req.holdMs } : {}),
    ...(req.signal ? { signal: req.signal } : {}),
  })
  if (!upstream.ok || !upstream.body) throw new Error(`persona gateway ${upstream.status}`)
  // A CANNED STREAM IS AN OUTAGE WEARING A 200. `proxyChat` answers an agent
  // that never came back — and one that was never rendered — with an English
  // sentence streamed in OpenAI chunk format, which is the right thing to show a
  // human in a chat window and indistinguishable from a model reply to
  // everything downstream of `upstream.ok`. Text harnesses cleaned it, the
  // runner recorded a held contract, and the sentence was persisted as the
  // agent's work. It is a failed call, so it fails like one: the harness gets
  // `value: null`, the row carries the reason, and the caller keeps what it had.
  const canned = upstream.headers.get(CANNED_STREAM_HEADER)
  if (canned) {
    await upstream.body.cancel().catch(() => {})
    throw new Error(canned === 'mock' ? `"${req.model}" is not a rendered agent (the fleet answered in mock mode)` : `persona "${req.model}" did not come back within the hold window`)
  }
  const turn = await pumpPersonaStream(upstream.body, emit)
  await meterPersonaTurn(req, turn)
  return { kind: 'fleet', text: turn.text, toolNames: turn.toolNames, usage: turn.usage, contractDropped: false }
}

export const fleetTransport: Transport = (req) => personaTurn(req)

/** The same call, with each delta handed on as it lands. This is what the
 *  briefing panel's tee was: one branch to the owner's screen, one to the guard.
 *  Streaming is a property of the TRANSPORT, never of the harness contract. */
export const fleetStream: StreamingTransport = (req, emit) => personaTurn(req, emit)

/** THE TRANSPORT RULE, in one place so no harness author ever restates it:
 *
 *    a model the ORG GATEWAY serves goes through `completeViaGateway`;
 *    a model that is a LIVE FLEET PERSONA goes through `proxyChat`;
 *    the gateway wins when a model is somehow both.
 *
 *  The gateway wins because it is the metered, fully-inspectable path: it knows
 *  the endpoint, it writes the ledger row itself, and the runner gets the whole
 *  message history to guard against. A persona's tool loop runs inside the
 *  agent container, so that path can only ever offer tool names.
 *
 *  "Live" is load-bearing. `listAgents` answers with three MOCK agents and
 *  `source: 'mock'` when the fleet has never been rendered, and `proxyChat`
 *  answers an unknown model with a canned "this is a mock" stream. Treating a
 *  mock as a persona would hand a harness a chatty English sentence to parse as
 *  its verdict, so only `source: 'gateway'` counts.
 *
 *  TIERS ROUTE TOO. The Plan modal has a model-tier dropdown and `routedModelFor`
 *  turns that pick into exactly such an id; `inbox-focus.ts` `validDelegate`
 *  returns one for a delegate chosen with a tier. `listAgents` deliberately HIDES
 *  tier entries from the picker ("dex-developer-opus" is not something you
 *  assign; "dex-developer" is), so matching on its list alone classified a tier
 *  as a gateway model and failed the call with "model X is not on the gateway" —
 *  a live regression against the hand-written `proxyChat` calls this runner
 *  replaced, which passed the tier id straight through.
 *
 *  A tier id is `<live agent id>-<alias>`, split at the LAST hyphen, which is the
 *  same rule `gateway.ts` uses to hide them in the first place. That inference
 *  stays, because a caller may still arrive with a routed id it built itself —
 *  but a caller that KNOWS it is routing a tier should say so with
 *  `RunContext.tier`, which needs no inference and is the only way the ledger can
 *  price the turn (see `LedgerAttribution.tier`). `persona.ts` resolves the same
 *  ids for capability keys, and does it from the database. */
export async function pickTransport(model: string): Promise<Transport> {
  if ((await gatewayModels()).some((m) => m.id === model)) return gatewayTransport
  const fleet = await listAgents()
  if (fleet.source === 'gateway') {
    if (fleet.agents.some((a) => a.id === model)) return fleetTransport
    const cut = model.lastIndexOf('-')
    if (cut > 0 && fleet.agents.some((a) => a.id === model.slice(0, cut))) return fleetTransport
  }
  // Neither: let the gateway say so. `completeViaGateway` throws a precise
  // "model X is not on the gateway", which is a better failure than a mock
  // persona stream that looks like an answer.
  return gatewayTransport
}

export const defaultTransport: Transport = async (req) => (await pickTransport(req.model))(req)

/** CAN this model be offered tool definitions at all — asked BEFORE a call is
 *  made, and answered by the transport rule itself rather than by a second copy
 *  of it. Derived from `pickTransport` on purpose: the day a persona path can
 *  honestly run our tool loop, this answers true with no edit here, and until
 *  then it cannot disagree with the transport that would actually refuse.
 *
 *  The tool probes ask this so a fleet candidate SKIPS. Letting the refusal
 *  throw instead would score as `errored` — which by the probe suite's own rule
 *  2 means "the deployment failed", and a persona is not a broken deployment. */
export const offersToolDefinitions = async (model: string): Promise<boolean> => (await pickTransport(model)) === gatewayTransport

/** CAN this model run a turn with ITS OWN tool loop — asked BEFORE a call is
 *  made, and, like `offersToolDefinitions`, derived from `pickTransport` rather
 *  than restated, so it can never disagree with the transport that would
 *  actually refuse.
 *
 *  IT IS THE EXACT COMPLEMENT of the question above, and the pair is the whole
 *  transport rule read from both ends: a persona has a tool loop and cannot be
 *  handed tool definitions; the gateway can be handed definitions and has no
 *  loop. Neither transport does both.
 *
 *  THE FITNESS SWEEP ASKS THIS, and until it did, three harnesses whose whole
 *  feature is the tool loop (`work-session`, `outreach:check-in`,
 *  `briefer:chat`) were replayed against every ORG GATEWAY candidate, refused by
 *  `gatewayToolsRefusal` before a single token was spent, and recorded as 0%
 *  first-pass. A model that was never called scoring zero is the same class of
 *  confidently-wrong number the probe suite's `skipped` outcome exists to
 *  prevent — see `evals.ts` `harnessSkipReason`. */
export const runsOwnToolLoop = async (model: string): Promise<boolean> => (await pickTransport(model)) === fleetTransport
