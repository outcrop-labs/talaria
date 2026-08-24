// THE runner. One chokepoint, and eventually the only code in Talaria that
// puts words into a model and reads something structured back.
//
// WHY THIS FILE EXISTS
//   There are five ways to reach a model in this tree (proxyChat,
//   completeViaGateway, buildUpstream + fetchUpstream, the /api/llm route, and
//   inbox-focus's private requestJson* pair), and nine harnesses each picked
//   one, then wrote their own prompt, their own parser, their own fallback
//   chain and their own failure behavior. Everything the audit found is a
//   property of that arrangement rather than of any one site:
//
//     1.1  six extractors, three of which fail on ordinary small-model output
//     1.2  the gateway can silently drop response_format and answer in prose
//     1.3  ONE feature asking for JSON two different ways depending on which
//          model the user picked - strict json_object on one path, a prompt
//          suffix and a different temperature on the other
//     1.4  nothing, anywhere, re-asks after a malformed structured reply
//     1.5  the three highest-stakes model paths run with no guardrail at all,
//          because the guard was wired per call site and they were written
//          after the wiring
//     1.10 the fallback chain, verbatim, six times
//
//   Fixing those at nine call sites is nine chances to get it wrong and a tenth
//   site next quarter. Fixing them here is once. That is the entire argument.
//
// WHAT THE RUNNER DECIDES, so that no harness author ever decides it again:
//   - which model (harness/model.ts - the chain, expressed once, and it records
//     WHICH step won, because a subsystem limping along on 'first-routable' for
//     a month is a real finding and today it is invisible)
//   - which transport (see `pickTransport` in harness/transport.ts - the persona
//     gateway or the org gateway, never the harness's problem)
//   - whether to ask for JSON at the protocol level, and what to do when the
//     answer comes back as prose anyway
//   - whether to repair, and whether repairing is SAFE (see the guard gate in
//     the repair loop - the one place this runner could break guardrails.ts's
//     cardinal invariant, and does not)
//   - what a failure means, from the harness's own `onFailure`
//
// WHAT A HARNESS DECLARES AND THIS FILE HONORS, added after five files worked
// around its absence by hand-writing a transport apiece (`sessionTransport`,
// `planPersonaTransport`, `personaTurnWithOwnTools`, the briefing tee, the Muse
// replay - one gap wearing five coats, and three of the coats silently dropped
// `temperature` and `jsonMode` on the way). All five are DELETED; the list is
// kept because it is the argument for adding the sixth slot here rather than at
// a call site, not because any of that code is still there to read:
//   - `def.tools: 'own'`   the model's own tool loop, for the three turns whose
//                          whole feature IS the tool loop
//   - `def.holdMs`         how long to hold for a persona that is restarting
//   - `ctx.ledger`         source / refId / taskId, so a turn's spend reaches
//                          the ticket, the channel or the research run
//   - `ctx.tier`           an alias tier, routed AND priced (a routed id with a
//                          null tier misses both of recordUsage's lookups and
//                          the turn costs nothing)
//   - `runHarnessStreamed` a streaming transport, so a surface where tokens
//                          landing on a screen is the feature keeps streaming
//                          and still gets the guard pass
//   - `def.ground`         the turn's REAL tool record, from the harness's own
//                          input. Without it `ungrounded_ref` - the single
//                          highest-value rule in guardrails.ts - could not fire
//                          from any harness in the product, by construction:
//                          the record this runner derives has no backing tools,
//                          so the rule declined every time it was asked.
//
// TESTABILITY IS A DESIGN CONSTRAINT, NOT A NICETY. Every edge to the outside
// world - model resolution, capabilities, the transport, the guard config,
// findings, the ledger, the clock - is a field on `HarnessDeps`, defaulted to
// the real thing and overridable per call. `run.test.ts` exercises the whole
// runner against recorded replies with no database, no gateway and no fleet,
// which is also exactly how the model-fitness suite will replay eval fixtures.
import { extractToolRecord, getGuardConfig, groundingTextOf, needsRedaction, recordFindings, redactSecrets, RULES, runGuardrails, guardText, type Available, type Finding, type GuardConfig, type ToolRecord } from '../guardrails'
import { routingFor } from '../llm-gateway'
import { reachFor, type Reach } from '../capability-reach'
import { db } from '../db/pg'
import { capabilityKey, missingCapabilities, getCapabilities, type Capability, type CapabilitySource } from './capability'
import { promptShape, wireSchemaOf, type WireSchema } from './json-schema'
import { personaCapabilityKeys } from './persona'
import { parseJson, repairPrompt } from './json'
import { resolveHarnessModel, type ModelChainStep, type ModelSpec } from './model'
import { agentSlot, roleSlot, slotEffortForModel } from '../effort-prefs'
import { defaultTransport, type LedgerAttribution, type StreamingTransport, type Transport, type TransportKind, type TransportRequest } from './transport'
import type { Grounding, HarnessDefinition, Message, RenderContext } from './define'

// The transports moved to harness/transport.ts when `TransportRequest` grew the
// four slots the five hand-written shims existed to supply (tools, ledger
// attribution, an explicit tier, a hold deadline). They are re-exported here
// because every one of those files, plus defs/research.ts, imports `Transport`
// from this module — and because "which transport" remains a decision of the
// runner's, not of any harness author's.
export {
  defaultTransport,
  fleetStream,
  fleetTransport,
  gatewayImageTurn,
  gatewayStream,
  gatewayTransport,
  ledgerOf,
  meterPersonaTurn,
  personaPayload,
  personaProbeTurn,
  offersToolDefinitions,
  pickTransport,
  pumpPersonaStream,
  toolDefsOf,
  toolNamesOf,
  toolPolicyOf,
  toolWireMessage,
} from './transport'
export type {
  LedgerAttribution,
  PersonaPayload,
  PersonaTurn,
  StreamingTransport,
  ToolCall,
  ToolDefinition,
  ToolPolicy,
  Transport,
  TransportKind,
  TransportReply,
  TransportRequest,
} from './transport'

// ── The result ───────────────────────────────────────────────────────────────

export interface HarnessResult<O> {
  value: O | null
  model: string | null
  step: ModelChainStep | null
  widened: boolean
  repairs: number
  /** The output contract held: a `json` harness parsed and validated, a `text`
   *  harness survived its `clean`, and — either way — the harness's own
   *  `output.verify` accepted the value against the run's INPUT.
   *
   *  DELIBERATELY NOT COMPARABLE ACROSS HARNESSES — a titler's
   *  non-empty-string check and the judge's zod parse are both `true` here and
   *  mean wildly different things. The fitness page aggregates it PER HARNESS
   *  ("contract rate"), which is the only reading that means anything;
   *  `harness_runs.harness` is in every index for that reason, and a global
   *  average over this column would be a number with no referent.
   *
   *  `verify` is in here rather than beside it because this column is the
   *  OBSERVED half of the fitness matrix and it has to agree with the offline
   *  eval fixtures. It did not: blurb-writer's `checkBatch` rejects invented
   *  ids, while the schema — which cannot see the input and therefore cannot
   *  constrain the keys — passed a reply containing zero usable blurbs and
   *  recorded a 100% contract rate. Between the two, the production column was
   *  the optimistic liar. */
  schemaValid: boolean
  /** DID THE MODEL ACTUALLY ANSWER — a completed transport call that came back
   *  with something to apply the contract to.
   *
   *  False for every way a run ends without a reply: nothing in the chain
   *  routes, the floor refuses, `render` produced no messages, the transport
   *  threw, or the model returned an empty string. True the moment a reply
   *  arrives, whatever happens to it afterwards — a parse failure, a verify
   *  failure, a refused repair and a redaction that broke the value are all
   *  `answered: true`, because in every one of them a model spoke.
   *
   *  IT HAS A NAME BECAUSE IT WAS ALREADY THE TEST. `raw !== null` had become
   *  the de-facto "did the model answer" check in three adapters — channel-plan
   *  and comms-decay to tell a restarting agent container apart from a
   *  conversation with nothing in it, plan-doc to choose between "the agent
   *  returned an empty document" and "the agent could not be reached" — and
   *  `raw` is a DRILL-DOWN field, bounded, trimmed and nullable for reasons that
   *  have nothing to do with that question. It also got the answer wrong in one
   *  case each of them cares about: a stream that dies after three tokens leaves
   *  a `raw` behind, so a transport failure read as a model that answered
   *  badly. */
  answered: boolean
  /** THE HARNESS DECLINED TO ASK — the capability floor refused this model, so
   *  no question reached it and nothing here is a fact about it.
   *
   *  IT NEEDS ITS OWN FIELD because `answered: false` cannot tell "we asked and
   *  got nothing" apart from "we never asked", and the fitness sweep has to.
   *  A refusal recorded as an error becomes a case the model FAILED: the health
   *  view charged `research-search` five fixture failures to glm-5.2 for a floor
   *  refusal it never saw, which is precisely the category error the floor
   *  exists to make visible. A refusal is a SKIP — the absence of evidence. */
  refused: boolean
  findings: Finding[]
  /** The model's last raw reply, before `clean`/parse — trimmed to a bound.
   *
   *  Part 3's whole trust story is the drill-down: an operator looking at a red
   *  cell in the fitness matrix wants the actual prompt and the actual response,
   *  not a score. Nothing downstream can show that if the runner throws the
   *  reply away, and widening this type later is one edit while retrofitting it
   *  through every ported call site is not. Null when no model was reached. */
  raw: string | null
  latencyMs: number
  /** The harness declared `onFailure: { escalate: true }` and the contract
   *  failed. A FLAG rather than a phrase in `error`, because the caller is the
   *  only thing that knows who to tell (judge.ts's `tellHumansTheGateStopped`)
   *  and a caller that has to string-match an error message to find that out is
   *  a caller that will silently stop escalating the day the wording changes. */
  escalate: boolean
  error?: string
}

// ── Injected edges ───────────────────────────────────────────────────────────

/** One row of the production ground truth the model-fitness UI reads (audit
 *  Part 3): contract rate and repair rate per harness per model over time. */
export interface HarnessRunRow {
  harness: string
  model: string | null
  step: ModelChainStep | null
  widened: boolean
  repairs: number
  schemaValid: boolean
  latencyMs: number
  /** How many guard findings this run is EVIDENCE for — grounded ones excluded,
   *  exactly as `recordFindings` excludes them from `guard_findings`. The
   *  fitness page reads the two side by side. */
  findings: number
  caller: string
  /** WHY the run failed, in one sentence, or null when it did not. Without it a
   *  red cell in the fitness matrix is a number with nothing behind it, and the
   *  first question anyone asks of a red cell is "failed how?".
   *
   *  `raw` deliberately does NOT come with it. The reply can be large and it is
   *  model output; it belongs in the live `HarnessResult` for a drill-down, not
   *  in a row that accumulates forever. */
  error: string | null
}

export interface HarnessDeps {
  resolveModel: (spec: ModelSpec) => Promise<{ model: string; step: ModelChainStep } | null>
  /** The admin's slot-level effort preference (Models → Roles / Platform),
   *  validated against the model's live published levels — or null. Injected
   *  for the same reason every other edge here is: the policy is the part
   *  worth testing without a database. */
  slotEffort: (slot: string, model: string) => Promise<string | null>
  /** Where a model CAN land, without advancing the round-robin cursor. */
  routing: (model: string) => Promise<{ endpoints: string[]; upstreamModel: string }>
  /** Capability keys a FLEET PERSONA inherits from the model behind it — see
   *  `harness/persona.ts` and the note at the derivation site below. Empty for
   *  anything that is not a live persona. */
  personaKeys: (model: string) => Promise<string[]>
  missingCapabilities: (key: string, required: Capability[]) => Promise<Capability[]>
  capabilities: (key: string) => Promise<Partial<Record<Capability, { value: boolean; source?: CapabilitySource }>>>
  /** CAN THE RUN reach these capabilities — natively, or through a tool this
   *  install has registered. Consulted only for capabilities a harness declares
   *  `suppliable` (see `RoleFloor`), and only when the floor is otherwise about
   *  to refuse, so the registry read never lands on a path that would not have
   *  used it. */
  reach: (keys: readonly string[], wanted: readonly Capability[]) => Promise<Record<string, Reach>>
  transport: Transport
  guardConfig: () => Promise<GuardConfig>
  /** `input` is the turn's own grounding material — see the `grounding` block
   *  below the render step. Optional so a test stub of one argument stays
   *  assignable; the real `guardText` has always taken it. */
  guardText: (text: string, input?: string) => Promise<Finding[]>
  recordFindings: (findings: Finding[], meta: { caller: string; model: string; endpoint: string | null; mode: GuardConfig['mode'] }) => Promise<void>
  recordRun: (row: HarnessRunRow) => Promise<void>
  now: () => number
}

const REAL_DEPS: HarnessDeps = {
  resolveModel: resolveHarnessModel,
  slotEffort: (slot, model) => slotEffortForModel(slot, model),
  routing: async (model) => {
    const r = await routingFor(model)
    return { endpoints: r.endpoints.map((e) => e.name), upstreamModel: r.upstreamModel }
  },
  personaKeys: personaCapabilityKeys,
  missingCapabilities,
  capabilities: getCapabilities,
  reach: reachFor,
  transport: defaultTransport,
  guardConfig: getGuardConfig,
  guardText,
  recordFindings,
  recordRun: async (row) => {
    const sql = await db()
    await sql`
      insert into harness_runs (harness, model, chain_step, widened, repairs, schema_valid, latency_ms, findings, caller, error)
      values (${row.harness}, ${row.model}, ${row.step}, ${row.widened}, ${row.repairs}, ${row.schemaValid}, ${row.latencyMs}, ${row.findings}, ${row.caller}, ${row.error})
    `
  },
  now: () => Date.now(),
}

/** THE CAPABILITY KEYS A ROUTED MODEL ANSWERS FOR — one per endpoint that could
 *  take the call, because a bare model name may be served by a POOL and we
 *  cannot know which member will take this one without advancing the round-robin
 *  cursor.
 *
 *  Exported because `research.ts` has to ask `capability-reach.ts` the same
 *  question the floor asks — "can this run reach search" — and had to derive the
 *  same keys to do it. Two spellings of a key derivation is how a stage and the
 *  floor that guards it come to disagree about which model they are talking
 *  about, and the whole point of this pass is that they never do. */
export const capabilityKeysOf = (route: { endpoints: string[]; upstreamModel: string }): string[] =>
  route.endpoints.map((e) => capabilityKey(e, route.upstreamModel))

/** The keys for a model id, resolved through the REAL routing — the convenience
 *  form for callers outside the runner, which have no `HarnessDeps` in hand.
 *  Falls back to a persona's inherited keys exactly as the runner does. */
export async function capabilityKeysFor(model: string): Promise<string[]> {
  const route = await REAL_DEPS.routing(model).catch(() => ({ endpoints: [] as string[], upstreamModel: model }))
  const keys = capabilityKeysOf(route)
  return keys.length > 0 ? keys : await REAL_DEPS.personaKeys(model).catch(() => [])
}

/** What the CALLER knows about where this turn's spend belongs. The runner
 *  fills in the rest of `LedgerAttribution` — `agentModel` is the resolved base
 *  model and `tier` is `RunContext.tier`, neither of which a caller should have
 *  to restate. Omitted entirely means the default a harness turn on a persona
 *  has always had: `source: 'chat'`, belonging to no conversation. */
export interface RunLedger {
  source?: LedgerAttribution['source']
  /** The conversation, channel or research run. */
  refId?: string | null
  /** The ticket, so the turn reaches the ticket's cost and not just the ledger. */
  taskId?: string | null
}

export interface RunContext {
  /** Ledger + findings attribution, e.g. 'platform:titler', 'ticket:<id>'. */
  caller: string
  /** For user-scoped harnesses: enables the chain's 'preferred' step and the
   *  member model allowlist (see harness/model.ts). */
  userId?: string
  /** Pin the model, skipping resolution entirely. This is how the fitness
   *  suite replays a harness against a candidate model, how every harness whose
   *  model comes from the SUBJECT of the call names it (the owner's assistant,
   *  the agent on the ticket), and the only supported way to bypass the chain. */
  model?: string
  /** Which chain step produced `ctx.model`, when the caller ran the chain
   *  ITSELF and is only handing the answer over — `routes/api/muse.ts` must have
   *  the model before it opens the stream, for `x-muse-model` and so that "no
   *  model routes" is a 400 rather than a stream that opens empty.
   *
   *  It exists so that pre-resolving does not silently erase the step from the
   *  `harness_runs` row: an install limping along on 'first-routable' for a
   *  month is a real finding and the fitness page reads exactly this column.
   *  Before this field the muse route kept the step by overriding
   *  `deps.resolveModel`, which made a production path depend on the testing
   *  seam and gave "the model is already known" two spellings.
   *
   *  Ignored unless `model` is set. A caller that genuinely pinned (the fitness
   *  suite) leaves it off and the row honestly records no step. */
  step?: ModelChainStep
  /** Route this turn to an ALIAS TIER of the resolved model — the alias NAME
   *  ('opus'), not the routed id.
   *
   *  `pickTransport` can already INFER a tier from a routed id by splitting at
   *  the last hyphen, and it still does, because a caller may arrive with an id
   *  it built itself. But inference cannot recover the two facts the ledger
   *  needs: `recordUsage` prices a row by finding `agent_defs.model =
   *  agentModel` and then the alias named by `tier`, so a routed id handed over
   *  with a null tier misses BOTH lookups and the turn is priced at nothing.
   *  Naming the tier here is how a caller says it deliberately, once, and gets
   *  the routing and the price together. */
  tier?: string
  /** THE REASONING EFFORT THIS TURN RUNS AT, when the caller's surface let a
   *  human pick one. A model id, not a harness, decides whether the level is
   *  honored — the transports send it as `reasoning_effort` and the provider
   *  rejects what it rejects — so callers are expected to have asked
   *  `effortsForModel` first (the chat routes validate against it). Absent
   *  means the model's own default, which is the only honest default there
   *  is: an unrequested effort is a request the user did not make. */
  effort?: string
  /** Where this turn's spend belongs. See `RunLedger`. */
  ledger?: RunLedger
  signal?: AbortSignal
  /** Override any edge. Tests supply a `transport` and no-op recorders; the
   *  fitness suite supplies a transport that records prompts and replies. */
  deps?: Partial<HarnessDeps>
}

/** What `runHarnessStreamed` needs beyond a normal run. */
export interface StreamOptions {
  /** The streaming transport. See `StreamingTransport` in harness/transport.ts:
   *  it gets the same request every transport gets and must call `emit` with
   *  each delta as the delta arrives. */
  stream: StreamingTransport
  /** Where the deltas go — the browser's chunks. Called on the RAW reply, before
   *  any guard pass has run, because by then the bytes are already on the wire.
   *  A surface that must scrub what it relays (strict-mode Muse) redacts here,
   *  on the way out, which is the only place that can work: `guardrails.ts`
   *  cleans "what Talaria persists or hasn't yet relayed", and on a streamed
   *  draft the relayed copy IS the saved one. */
  onDelta?: (delta: string) => void
}

// ── Small pure helpers ───────────────────────────────────────────────────────

/** The instruction every structured call carries. ONE wording, so the two
 *  halves of a feature can never disagree the way inbox-focus's two request
 *  helpers do today (audit 1.3 — strict `json_object` and temperature 0.1 on
 *  the persona path, a prompt suffix and temperature 0.2 on the gateway path,
 *  chosen by which model the user picked), and so a change to it is measurable
 *  across every harness at once. */
const JSON_ANCHOR = 'Reply with exactly one JSON value and nothing else - no explanation before or after it, and no markdown code fence.'

/** The anchor, plus THE SHAPE when this build can render one.
 *
 *  The anchor alone said "reply with JSON" and never said WHAT JSON, while the
 *  harness sitting one line away held the schema. A frontier model infers the
 *  shape from the surrounding prose and looks fine — which is exactly why it went
 *  unnoticed — and a 7-14B model does not. This layer exists so that difference
 *  is engineered away rather than left to the model, and telling it the shape is
 *  the cheapest possible way to do that: one line of prompt, no extra call.
 *
 *  Sent even when `response_format` carries the schema at the protocol level,
 *  for the reason the anchor itself is: a provider can drop the parameter, and
 *  the prompt survives it. */
const jsonAnchorFor = (wire: WireSchema | null): string => {
  const shape = wire ? promptShape(wire.schema) : null
  return shape ? `${JSON_ANCHOR}\n\nIt must match this shape exactly:\n${shape}` : JSON_ANCHOR
}

/** Appended to the LAST USER TURN rather than sent as a new message: a small
 *  model weights the end of its prompt most heavily, and a trailing standalone
 *  instruction reads as something to acknowledge ("Understood - I'll return
 *  JSON.") instead of as a constraint on the answer. */
function anchorJson(messages: Message[], anchor: string = JSON_ANCHOR): Message[] {
  let last = -1
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i]?.role === 'user') {
      last = i
      break
    }
  }
  if (last === -1) return [...messages, { role: 'user', content: anchor }]
  return messages.map((m, i) => (i === last ? { ...m, content: `${m.content}\n\n${anchor}` } : m))
}

const lastUserMessage = (messages: Message[]): string => {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]
    if (m?.role === 'user') return m.content
  }
  return ''
}

type Applied<O> = { ok: true; value: O } | { ok: false; error: string }

/** Raw reply -> the harness's value. Shared by the main loop and the
 *  redaction re-run below, so a redacted value is held to exactly the same
 *  contract as the original.
 *
 *  `clean` and `verify` are harness-author code running on model output, which
 *  is to say a regex meeting a string somebody else wrote. A throw out of either
 *  is not in the contract `define.ts` states (return the value, or null / a
 *  sentence to fail it), and it must not be the one thing that escapes a runner
 *  whose whole promise is that a bad model produces a RESULT rather than an
 *  exception - least of all from the redaction re-run, which happens after the
 *  guard pass and outside the call's try block. A throw is a failed contract,
 *  spelled the same as a null. */
function applyOutput<I, O>(def: HarnessDefinition<I, O>, raw: string, input: I, ctx: RenderContext): Applied<O> {
  const parsed = parseValue(def, raw)
  if (!parsed.ok) return parsed
  return verified(def, parsed.value, input, ctx)
}

function parseValue<I, O>(def: HarnessDefinition<I, O>, raw: string): Applied<O> {
  if (def.output.kind === 'json') return parseJson(raw, def.output.schema)
  const clean = def.output.clean
  if (clean) {
    let value: O | null
    try {
      value = clean(raw)
    } catch (err) {
      return { ok: false, error: `the harness clean step threw on the reply: ${err instanceof Error ? err.message : String(err)}` }
    }
    return value === null ? { ok: false, error: 'the reply did not survive the harness clean step' } : { ok: true, value }
  }
  // A text harness that declares no `clean` is by construction O = string.
  // This is the ONLY place that assumption lives, and `define.ts` states it as
  // part of the contract.
  if (!raw.trim()) return { ok: false, error: 'the model returned nothing' }
  return { ok: true, value: raw as unknown as O }
}

/** THE HALF OF THE CONTRACT A SCHEMA CANNOT STATE. See `Verify` in define.ts:
 *  a schema is a module constant built before the input exists, so a harness
 *  whose correctness is a RELATION between the two - "the keys are the ids I
 *  sent", "these are tickets from the transcript", "this date is one the write
 *  path accepts" - had nowhere to say it, and the runner recorded
 *  `schemaValid: true` for a value the caller then threw away.
 *
 *  It runs ONLY on a value that already parsed, so a verify never has to
 *  re-check a type, and its sentence is returned as the contract error verbatim
 *  - which is what puts it in `repairPrompt` on the next turn and on the
 *  `harness_runs` row if there is no next turn. Identical handling to a parse
 *  failure, deliberately: it IS one. */
function verified<I, O>(def: HarnessDefinition<I, O>, value: O, input: I, ctx: RenderContext): Applied<O> {
  const verify = def.output.verify
  if (!verify) return { ok: true, value }
  let message: string | null
  try {
    // The SAME `RenderContext` the prompt was built from, so a harness whose
    // contract changes when it widens can check the contract it actually
    // offered. `inbox-command` is the case: the widened list is the item's own
    // actions and the narrow one is a single regex-matched id.
    message = verify(value, input, ctx)
  } catch (err) {
    return { ok: false, error: `the harness verify step threw on the reply: ${err instanceof Error ? err.message : String(err)}` }
  }
  if (message === null) return { ok: true, value }
  // An empty sentence is a harness-author mistake, not a pass. Failing it with a
  // generic line keeps the contract honest (the value WAS rejected) without
  // sending the model a repair turn that names no problem to fix.
  return { ok: false, error: message.trim() || 'the value did not satisfy the harness output check' }
}

/** The guard registry narrowed to the ids a harness declared. An admin who
 *  turned a rule OFF still wins - both have to say yes - because the admin
 *  switch is org policy and the harness list is a relevance filter. */
function narrowGuardConfig(config: GuardConfig, rules: string[] | undefined): GuardConfig {
  if (!rules) return config
  const allowed = new Set(rules)
  const checks: Record<string, boolean> = {}
  for (const rule of RULES) checks[rule.id] = allowed.has(rule.id) && (config.checks[rule.id] ?? rule.defaultOn)
  return { ...config, checks }
}

/** Merge guard findings without double-counting.
 *
 *  The repair gate and the final guard pass scan THE SAME REPLY, and both run
 *  the gate-safe rules, so a reply that was refused a repair because it carried
 *  a credential would be recorded twice for one leak. That is not a cosmetic
 *  duplicate: `guard_findings.model` is the live per-model confabulation rate
 *  the fitness page reads next to benched scores, and a rate that doubles
 *  whenever a repair is refused would make the repair path itself look like a
 *  safety regression.
 *
 *  Identity is check + snippet, not check alone: the same rule firing on a
 *  DIFFERENT span is a second, real finding and must survive. */
/** The failure sentence as it goes into `harness_runs`.
 *
 *  REDACTED FIRST, THEN BOUNDED, and the order is not arbitrary: slicing first
 *  can cut a credential in half so no pattern matches it and the tail lands in
 *  the table verbatim. A parser error quotes the model's own rejected value
 *  ("field 'summary' must be one of ..."), so this string is model output too,
 *  and unlike `raw` it is kept forever.
 *
 *  Bounded because a run row is telemetry, not an archive. 1000 characters holds
 *  the three issues `describeIssues` reports plus a floor note, which is the
 *  longest honest failure sentence this runner produces. */
const ERROR_CAP = 1_000
const runError = (error: string | undefined): string | null => (error ? redactSecrets(error).text.slice(0, ERROR_CAP) : null)

function mergeFindings(into: Finding[], found: Finding[]): void {
  for (const f of found) {
    if (into.some((prev) => prev.check === f.check && prev.snippet === f.snippet)) continue
    into.push(f)
  }
}

/** The same bound `extractToolRecord` puts on a derived record, applied to a
 *  declared one for the same reason: past it, `ungrounded_ref` is scanning a
 *  haystack big enough to be a performance problem for a check whose whole
 *  virtue is that it is cheap. Overflowing FAILS OPEN — the rule skips — which
 *  is guardrails.ts's own choice, restated here rather than re-decided. */
const GROUND_RESULTS_CAP = 200_000

/** The harness's own account of what really ran this turn, or null.
 *
 *  NULL IS THE SAFE ANSWER AND IT IS THE DEFAULT IN THREE WAYS: no hook, a hook
 *  that returns null, and a hook that returns an EMPTY tool list. That last one
 *  is the important one — `ungrounded_ref` already declines on an empty
 *  `backingTools`, but `fabricated_outage` does not, so accepting a
 *  toolless "grounding" would let a harness with nothing to ground against
 *  assert `errorInfo: true` and start flagging outage reports it cannot check.
 *  Honesty has to be expressible; optimism must not be reachable by accident.
 *
 *  `ground` is harness-author code running over harness input, so a throw is a
 *  missing record and never an escaped exception — the same rule `render` and
 *  `clean` are held to. */
function groundingFor<I, O>(def: HarnessDefinition<I, O>, input: I): Grounding | null {
  if (!def.ground) return null
  let material: Grounding | null
  try {
    material = def.ground(input)
  } catch {
    return null
  }
  if (!material || material.tools.length === 0) return null
  return material
}

// ── The runner ───────────────────────────────────────────────────────────────

/** One harness run: resolve, floor, widen, render, call, parse, repair, guard,
 *  redact, apply the failure policy, meter. See the file header for what each of
 *  those closes.
 *
 *  `runHarnessStreamed` below is the same function with a streaming transport
 *  and the repair loop switched off; both go through `execute`. */
export async function runHarness<I, O>(def: HarnessDefinition<I, O>, input: I, ctx: RunContext): Promise<HarnessResult<O>> {
  return execute(def, input, ctx, null)
}

/** THE STREAMING ENTRY POINT — for the surfaces where tokens landing on a screen
 *  ARE the feature (the Muse's six prose kinds, the briefing panel's follow-up
 *  chat), which must not be turned into blocking calls to gain a guard pass.
 *
 *  It is the same runner. The caller supplies a transport that pumps rather than
 *  blocks and calls `emit` with each delta; the runner hands those on through
 *  `onDelta`, accumulates them, and then does everything it always does with the
 *  completed reply — the guard pass with an honest `Available`, the findings row,
 *  redaction of the VALUE, the failure policy and the `harness_runs` row. So the
 *  prompt, the model policy and the guard rules keep exactly one spelling, in the
 *  definition, rather than a second one in each streaming route.
 *
 *  TWO THINGS DIFFER FROM `runHarness`, both forced by the medium:
 *
 *    NO REPAIR. A repair turn re-asks and replaces the answer; the first answer
 *    has already reached the screen. Repairing would stream one document and
 *    hand back another, so the run is one attempt and a malformed reply is a
 *    failed contract — recorded honestly on the row, where a model that cannot
 *    hold the contract on a streamed surface becomes visible.
 *
 *    NO REDACTION OF WHAT WAS RELAYED. `def.guard.redact` still cleans the
 *    returned VALUE, but the bytes are gone. A surface that must scrub what it
 *    relays does it in `onDelta`, on the way out, which is the only place it can
 *    work. */
export async function runHarnessStreamed<I, O>(
  def: HarnessDefinition<I, O>,
  input: I,
  ctx: RunContext,
  opts: StreamOptions,
): Promise<HarnessResult<O>> {
  return execute(def, input, ctx, opts)
}

async function execute<I, O>(def: HarnessDefinition<I, O>, input: I, ctx: RunContext, streaming: StreamOptions | null): Promise<HarnessResult<O>> {
  const deps: HarnessDeps = { ...REAL_DEPS, ...ctx.deps }
  const started = deps.now()
  const findings: Finding[] = []

  // Every exit writes a harness_runs row, including the ones that never reach a
  // model. A harness that resolves nothing, or refuses on a capability, is
  // exactly the thing the fitness UI has to be able to see - today that failure
  // is a `return null` nobody ever hears about.
  const finish = async (r: Omit<HarnessResult<O>, 'latencyMs' | 'findings'> & { error?: string }): Promise<HarnessResult<O>> => {
    const latencyMs = Math.max(0, deps.now() - started)
    await deps
      .recordRun({
        harness: def.id,
        model: r.model,
        step: r.step,
        widened: r.widened,
        repairs: r.repairs,
        schemaValid: r.schemaValid,
        latencyMs,
        // GROUNDED FINDINGS ARE NOT COUNTED, for the reason `recordFindings`
        // does not file them: this column is the guard rate the fitness page
        // reads beside `guard_findings`, and the two must not disagree about
        // what a fact about the model is. A grounded hit survives in
        // `HarnessResult.findings` for `needsRedaction` and for a caller that
        // pins findings onto its own row; it is not evidence, so it is not a
        // number here.
        findings: findings.filter((f) => !f.grounded).length,
        caller: ctx.caller,
        error: runError(r.error),
      })
      .catch(() => {})
    return { ...r, findings, latencyMs }
  }

  /** THE FAILURE POLICY ON A RETURN PATH, which is the other half of what
   *  `onFailure` was always read to mean.
   *
   *  `runHarness` returns rather than throws for every failure that happens
   *  BEFORE or DURING the call - nothing in the chain routes, the floor refuses,
   *  `render` throws, the transport dies - so `onFailure: 'throw'` covered the
   *  contract failure and nothing else, and each caller had to restate the
   *  policy by hand. Five did. The two that did not both shipped a bug: research
   *  synthesis saved an empty report and marked the run `done` after a 502, and
   *  the channel planner reported "nothing to plan yet" on a channel full of
   *  work because its agent container was restarting. Every exit that fails to
   *  produce a value now comes through here.
   *
   *  ONLY 'throw' WIDENS. The other three policies describe what a caller gets
   *  when a model ANSWERED and the answer was unusable, and `define.ts` records
   *  why widening them would break the two callers that use them - a fallback
   *  would read a dead gateway as outreach's normal quiet pass, and an escalation
   *  would notify every board editor about every ticket for as long as the
   *  gateway is down. `answered` is how a caller asks for either deliberately. */
  const fail = async (r: Omit<HarnessResult<O>, 'latencyMs' | 'findings'> & { error: string }): Promise<HarnessResult<O>> => {
    const result = await finish(r)
    if (def.onFailure === 'throw') throw new Error(`harness "${def.id}" failed${r.model ? ` on "${r.model}"` : ''}: ${r.error}`)
    return result
  }

  const empty = { value: null, model: null, step: null, widened: false, repairs: 0, schemaValid: false, escalate: false, answered: false, refused: false, raw: null } as const
  /** What the drill-down shows. Bounded because a run row is telemetry, not an
   *  archive, and a model that answers with 200KB of prose must not be able to
   *  turn one failed run into a memory problem for whatever reads it. */
  const RAW_CAP = 8_000
  const rawOf = (t: string): string | null => (t ? t.slice(0, RAW_CAP) : null)

  // 1 ─ Resolve the model. Null is an ANSWER, not an exception: on an install
  // whose gateway serves nothing this spec can reach, the harness has no model
  // and the caller keeps what it had. Nothing throws out of here.
  let model: string
  let step: ModelChainStep | null = null
  if (ctx.model) {
    // An explicit pin has no chain step — unless the caller ran the chain itself
    // and said which step won (see `RunContext.step`).
    model = ctx.model
    step = ctx.step ?? null
  } else {
    const resolved = await deps
      .resolveModel({ ...def.model, ...(ctx.userId ? { userId: ctx.userId } : {}) })
      .catch(() => null)
    if (!resolved) {
      return fail({ ...empty, error: `no model available for harness "${def.id}" - nothing in its chain routes on this gateway` })
    }
    model = resolved.model
    step = resolved.step
  }

  // THE ID ACTUALLY CALLED. `model` stays the BASE persona — it is what the
  // ledger has to name, because `recordUsage` prices a row by finding
  // `agent_defs.model = agentModel` and then the alias named by `tier`, and a
  // routed id matches neither. `routed` is what goes on the wire, what the
  // capability lookup asks about (a tier is a different, usually larger model
  // than its agent's main, and `persona.ts` resolves it as one), and what the
  // result and the run row name, because it is the model that answered.
  //
  // The id is assembled here rather than by the caller so that "a tier id is
  // <agent>-<alias>" keeps ONE spelling. `pickTransport` still infers a tier
  // from a routed id for callers that arrive with one already built.
  const routed = ctx.tier ? `${model}-${ctx.tier}` : model

  // THE SLOT THAT PRODUCED THIS MODEL, when one did — the admin's effort
  // preference hangs off the WORK, not the model: 'role:utility' can ask for
  // low while 'role:code-heavy' on the same model asks for high. Only the pin
  // and role steps carry a slot the admin configured; env/preferred/
  // first-routable wins have no slot and no preference, which is correct: the
  // admin expressed no opinion about a model the chain found by itself.
  const slot =
    step === 'pin' && def.model.pin
      ? agentSlot(def.model.pin)
      : step === 'role' && def.model.role
        ? roleSlot(def.model.role)
        : step === 'utility'
          ? roleSlot('utility')
          : null
  // PRECEDENCE, in one line: the nearer the ask, the stronger it is — a
  // conversation pick (ctx.effort) beats the agent-configured default the
  // surface already folded into it, and both beat the slot preference. A
  // caller who never picked anything inherits the admin's dial for this class
  // of work, held against the model's live levels by the prefs module (a
  // stale level is dropped there, not sent here).
  const effort = ctx.effort ?? (slot ? await deps.slotEffort(slot, routed).catch(() => null) : null)

  // Capability facts are keyed 'endpoint:model', because capability is a
  // property of the ENDPOINT serving the model (see capability.ts). A bare
  // model name may be served by a POOL, and we cannot know which member will
  // take this call without advancing the round-robin cursor - so both questions
  // below are answered UNANIMOUSLY over the pool: a capability counts as
  // missing only if every member says missing, and counts as earned only if
  // every member says earned. Either way the runner is never surprised by the
  // member it happens to land on. A persona's pool (its tier's target plus the
  // agent's fallback providers) is the same shape and gets the same treatment.
  const route = await deps.routing(routed).catch(() => ({ endpoints: [] as string[], upstreamModel: routed }))
  let keys = capabilityKeysOf(route)
  const endpoint = route.endpoints.length === 1 ? (route.endpoints[0] ?? null) : null

  // A FLEET PERSONA is not a gateway catalog model, so `routingFor` answers with
  // NO endpoints for one and every line above produced nothing. Until this
  // lookup existed, `keys` was empty on every persona run, which made step 3's
  // `keys.length > 0` gate unpassable and `widened` a constant false there — on
  // the very path the widening feature was built for. The Inbox command harness
  // (audit 1.8) hands a capable model the item's full action list instead of a
  // regex-chosen single action, and it runs on the owner's PERSONAL ASSISTANT,
  // which is a persona. "Excel with larger models" was unreachable where it
  // mattered most, and silently so.
  //
  // A persona is BACKED by a real endpoint + upstream model (its agent version's
  // `config.main`, or the alias for the tier being called), so it inherits that
  // model's probe. `persona.ts` owns the resolution and every subtlety in it —
  // an unresolvable tier yields nothing rather than borrowing `main`'s facts,
  // and a config read that fails yields nothing rather than throwing. The
  // `.catch` here is the second belt on that: a capability lookup exists to make
  // a run better and must never be the reason one fails.
  if (keys.length === 0) keys = await deps.personaKeys(routed).catch(() => [])

  // 2 ─ The floor. UNKNOWN IS NOT MISSING (capability.ts owns that rule): an
  // untested model runs, because a fresh self-host has probed nothing and
  // Talaria cannot refuse to work until an admin gets around to benchmarking.
  const asked = [...new Set([...def.requires, ...def.floor.capabilities, ...(def.output.kind === 'json' ? (['json'] as Capability[]) : [])])]
  let missing: Capability[] = asked
  for (const key of keys) {
    const m = new Set(await deps.missingCapabilities(key, asked).catch(() => []))
    missing = missing.filter((c) => m.has(c))
  }
  if (keys.length === 0) missing = []
  const blocking = missing.filter((c) => def.floor.capabilities.includes(c))
  if (blocking.length && def.floor.refuseBelow) {
    // A LEARNED FACT SHAPES THE REQUEST. IT DOES NOT REFUSE THE RUN.
    //
    // The gateway writes `json: false` the first time an upstream 400s on
    // `response_format` — which is what audit 1.2 asked for, and it is why
    // `jsonMode` is suppressed below. But it is evidence about ONE PARAMETER on
    // one endpoint, not a measurement of whether the model can produce JSON, and
    // the harness sends the prompt anchor either way. Counted as a floor, one
    // 400 turned the QA judge — the only harness with `refuseBelow` — off for
    // every board for the 30-day learned TTL, with no notification, no
    // `judge_reviews` row and no admin surface that says so. Pre-port the judge
    // never even sent the parameter, so nothing could disable it.
    //
    // A refusal needs DELIBERATE evidence: a probe that measured the model, or a
    // HUMAN who declared it. Same unanimity rule as `missing` above — every key
    // in the pool has to carry that evidence, because refusing is the harmful
    // direction here.
    //
    // NEITHER 'learned' NOR 'catalog' IS SUCH EVIDENCE. A learned fact is one
    // upstream 400 about one parameter, for the reason above. A catalog fact is
    // a provider's published spec sheet — good enough to grant a capability
    // nobody has measured, never good enough to stop a run. (Today it cannot
    // arise: `capabilitiesFromCatalog` only ever writes `true`. The guard is
    // here so that stays a property of this refusal rather than an accident of
    // the writer.)
    const nothing: Partial<Record<Capability, { value: boolean; source?: CapabilitySource }>> = {}
    const facts = await Promise.all(keys.map((k) => deps.capabilities(k).catch(() => nothing)))
    const measured = blocking.filter((c) => facts.every((f) => f[c]?.value === false && f[c]?.source !== 'learned' && f[c]?.source !== 'catalog'))

    // THE PLATFORM MAY SUPPLY WHAT THE MODEL LACKS — `RoleFloor.suppliable`, and
    // this is where "capability of the model" stops being the question and
    // "capability of the run" becomes it. A harness that declares a capability
    // suppliable is asking: is there a registered, enabled tool for this, and
    // can this model call tools? If so the run proceeds, on the path the harness
    // wrote for exactly that case.
    //
    // ASKED ONLY WHEN IT COULD CHANGE THE ANSWER. `reach` is a registry read;
    // a harness with no `suppliable` capabilities, or one that is not about to
    // refuse anyway, never pays for it.
    const suppliable = measured.filter((c) => def.floor.suppliable?.includes(c))
    const supplied = suppliable.length > 0 ? Object.values(await deps.reach(keys, suppliable).catch(() => ({}))).filter((r) => r.reached) : []
    const unreachable = measured.filter((c) => !supplied.some((r) => r.capability === c))

    if (unreachable.length) {
      return fail({
        ...empty,
        model: routed,
        step,
        // NOT AN ERROR ABOUT THE MODEL. The floor declined to ask, so the sweep
        // records an absence rather than a failure — see `refused`.
        refused: true,
        error: `"${routed}" cannot run harness "${def.id}": it is known not to support ${unreachable.join(', ')}. ${def.floor.note}`,
      })
    }
  }

  // 3 ─ Widening. This looks inconsistent with step 2 and is not: step 2 asks
  // "is this model KNOWN to be unable", and widening asks "is this model KNOWN
  // to be able". Unknown is the answer to neither, and in both cases the safe
  // direction is the same one - keep running, on the deterministic surface. A
  // model nobody has probed gets the narrow prompt, which works everywhere,
  // rather than the wide one, which does not.
  //
  // ONLY A FACT THE PLATFORM MEASURED WIDENS, and the asymmetry with the floor
  // above is deliberate rather than an oversight to be tidied up later. The
  // floor accepts a `declared` fact as grounds to REFUSE (a human or a catalog
  // saying "this model cannot do JSON" is evidence enough to stop, and refusing
  // is recoverable in one click); widening requires `source: 'probe'`, because
  // widening is the direction that HANDS A MODEL MORE AUTHORITY. `inbox-command`
  // widened is a model choosing which action to take on somebody's ticket
  // instead of confirming the one a regex already chose. The evidence for that
  // has to be Talaria's own measurement - four tools offered, four correct picks
  // observed - and not a line in a vendor's model card. The moment anything
  // imports a catalog as `declared: true` (which is exactly what
  // `defaultDeps.advertises` reads for the vision probe), a marketing claim
  // would otherwise widen the Inbox across every install that synced it.
  //
  // `learned` is excluded by the same rule and would be even worse: the gateway
  // only ever writes `learned: false`, so a `learned: true` could only arrive
  // from a future writer nobody has designed yet.
  let widened = false
  if (def.widen && keys.length > 0) {
    const none: Partial<Record<Capability, { value: boolean; source?: CapabilitySource }>> = {}
    const facts = await Promise.all(keys.map((k) => deps.capabilities(k).catch(() => none)))
    widened = def.widen.requires.every((cap) => facts.every((f) => f[cap]?.value === true && f[cap]?.source === 'probe'))
  }

  // 4 ─ Render. The only harness-authored code that runs in here, which is
  // exactly why the call itself is inside the try: `Promise.resolve(x).catch()`
  // catches a rejected promise and NOT a synchronous throw, and a synchronous
  // throw is the likelier author mistake (a template over an input field that
  // turned out to be undefined). Either way it is a result, not an exception.
  // ONE `RenderContext` for the run: `render` builds the prompt from it and
  // `verify` checks the answer against it, so the surface a harness offered and
  // the surface it grades against can never be two different objects.
  const renderContext: RenderContext = { widened, model: routed }
  const base = await (async () => def.render(input, renderContext))().catch(() => null)
  if (!base || base.length === 0) {
    return fail({ ...empty, model: routed, step, widened, error: `harness "${def.id}" rendered no messages` })
  }

  /** THE TURN'S GROUNDING MATERIAL — everything this run put in front of the
   *  model, for `guardrails.ts`'s "was this span the model's at all" question.
   *
   *  IT WAS THE HOLE IN THE GROUNDING WORK. `groundingTextOf` landed with
   *  `agent-writes.ts` and `guardCompletion` wired to it and this runner not, so
   *  the ONE path that guards 23 harnesses was the one path that grounded
   *  nothing — and it is the path where it costs the most, because it is the
   *  only one that redacts the VALUE it hands back at every mode above off. A
   *  distiller summarizing a support chat filed `pii_leak` against the model for
   *  repeating the customer's order number, and then rewrote that number to
   *  `[redacted card number]` in the archive it wrote. Both halves are exactly
   *  what `pii_leak`'s `groundable: 'finding+redaction'` says must not happen.
   *
   *  `base`, NOT `sent`. `sent` accumulates the model's own rejected reply and
   *  the repair instruction that quotes it, so grounding attempt two against
   *  attempt one would let a model launder an invented card by emitting it
   *  twice. `groundingTextOf` drops assistant turns as a second belt; passing
   *  `base` means there is nothing for it to drop.
   *
   *  Computed ONCE: it is a join over the whole prompt and it is asked for three
   *  times below (the repair gate, the guard pass, the redaction), which have to
   *  agree with each other anyway. */
  const groundText = groundingTextOf(base)

  // 5/6 ─ Call, parse, repair.
  const structured = def.output.kind === 'json'
  // A STREAMED RUN NEVER REPAIRS. The repair turn replaces an answer, and on a
  // streamed surface the first answer already reached the screen — repairing
  // would stream one document and hand back another. A malformed reply is
  // therefore a failed contract, recorded honestly, which is also the number the
  // fitness page needs: a model that cannot hold a contract without a repair
  // round is a model you should not put on a streaming surface.
  //
  // A TEXT HARNESS STILL DOES NOT REPAIR, including on a `verify` failure, and
  // that is a limit worth naming rather than a case that was missed: the one
  // repair wording lives in json.ts and it ends "send the corrected JSON value
  // only", which is a nonsense instruction to a titler. Every harness whose
  // correctness is a relation between input and output is structured, so this
  // costs nothing today; a text harness that needs a repair turn needs a second
  // wording in json.ts first, not a branch here.
  const maxRepairs = streaming || def.output.kind !== 'json' ? 0 : Math.max(0, def.output.repair ?? 1)
  // ONE structured-output strategy, applied identically on both transports:
  // send the HARNESS'S OWN SCHEMA at the protocol level, and anchor the
  // instruction in the prompt as well. The two are not alternatives —
  // `response_format` constrains decoding, the anchor tells the model what to
  // produce and survives a provider that drops the parameter (audit 1.2) or a
  // transport with no slot for it. Belt and braces is the correct posture on a
  // 14B model and it costs one sentence of prompt.
  //
  // STILL GATED ON `missing`, and the gate now means something narrower than it
  // used to. A model MEASURED unable to do JSON never reaches this line at all:
  // a JSON harness carries `json` in its floor (see `defineHarness`) and step 2
  // refuses it above. What survives here is the LEARNED case — one upstream 400
  // about one parameter — where re-sending `response_format` would 400 every
  // call, so the parameter is suppressed and the prompt anchor carries the ask.
  // That distinction is the whole of `run.test.ts`'s "does NOT refuse on a fact
  // the gateway merely LEARNED from a 400", and it is worth keeping: the prose
  // path is no longer a fallback the platform CHOOSES for a model it knows
  // cannot comply, only the one it is left with when an endpoint rejects the
  // parameter outright.
  const wire = def.output.kind === 'json' ? wireSchemaOf(def.id, def.output.schema) : null
  let jsonMode = structured && !missing.includes('json')
  let sent = structured ? anchorJson(base, jsonAnchorFor(wire)) : base
  let repairs = 0
  let value: O | null = null
  let schemaValid = false
  let failure: string | null = null
  let text = ''
  /** See `HarnessResult.answered`. Set from the reply the contract is about to
   *  be applied to, so a repair turn that came back empty makes it false again -
   *  the result IS about that empty reply, and "the model returned nothing" is
   *  the honest failure for it. The transport-throw path below leaves it false
   *  whatever partial arrived, because that run ended at the transport and never
   *  applied a contract to anything. */
  let answered = false
  /** The streamed deltas, OUTSIDE the attempt loop on purpose. Declared inside
   *  it, a transport that died mid-stream took the accumulated reply with it —
   *  `text` was still '' when the catch below ran, so `raw` came back null and
   *  the drill-down for the failure an operator most wants to interrogate was
   *  empty. The comment in that catch has always claimed the opposite. */
  let streamed = ''
  let kind: TransportKind = 'gateway'
  let toolNames: string[] = []

  /** The ledger row this turn belongs to, resolved ONCE. `agentModel` is the
   *  base persona and `tier` the alias name, because that is the pair
   *  `recordUsage` prices from; `RunContext` carries the rest. */
  const ledger: LedgerAttribution = {
    agentModel: model,
    source: ctx.ledger?.source ?? 'chat',
    refId: ctx.ledger?.refId ?? null,
    taskId: ctx.ledger?.taskId ?? null,
    tier: ctx.tier ?? null,
  }

  try {
    for (;;) {
      const request: TransportRequest = {
        model: routed,
        messages: sent,
        ...(def.temperature !== undefined ? { temperature: def.temperature } : {}),
        jsonMode,
        ...(wire ? { jsonSchema: wire } : {}),
        ...(def.tools ? { tools: def.tools } : {}),
        // The tools this harness OFFERS, distinct from the policy above (see
        // `ToolPolicy` in transport.ts). A transport that cannot serve them
        // refuses the call, so a turn never silently becomes a plain completion
        // of a question the model was never asked.
        ...(def.toolDefs?.length ? { toolDefs: def.toolDefs } : {}),
        ledger,
        // The winning effort — caller pick, else the slot preference above.
        // Whether it can be honored is a per-transport question (a
        // tool-offering turn cannot take one — see gatewayToolTurn), not the
        // runner's.
        ...(effort ? { effort } : {}),
        ...(def.holdMs !== undefined ? { holdMs: def.holdMs } : {}),
        caller: ctx.caller,
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      }
      // The streamed deltas are accumulated (see the declaration above) as well
      // as handed on, so a transport that pumps into a browser and never
      // assembles the text itself may resolve with `text: ''` and the guard pass
      // still sees the whole reply. Reset per attempt for the same reason `text`
      // is reassigned.
      streamed = ''
      const reply = streaming
        ? await streaming.stream(request, (delta) => {
            streamed += delta
            streaming.onDelta?.(delta)
          })
        : await deps.transport(request)
      text = reply.text || streamed
      answered = text.trim().length > 0
      kind = reply.kind
      toolNames = reply.toolNames
      if (reply.contractDropped && jsonMode) {
        // The upstream refused JSON mode and the gateway dropped the parameter
        // rather than failing the call (audit 1.2). Before this signal existed
        // the call simply succeeded, came back as prose, and the caller fed
        // prose to a JSON parser believing it had asked for an object. We stop
        // asking for the rest of the run — the prompt anchor is already in
        // every structured request, so the repair turn below is a plain
        // text-mode ask, which is the deliberate fallback path the audit
        // asked for instead of a discovery from a 400.
        //
        // The capability FACT is not written here: the 400 was seen by the
        // gateway's own learner and recorded there, and two writers racing on
        // one settings row is how facts get lost.
        jsonMode = false
      }

      // Parse, then `verify` against THE ORIGINAL INPUT - never the repaired
      // message list, which by the second turn contains the model's own rejected
      // answer. A verify that graded a value against a conversation the model
      // half-wrote would drift a little further from the caller's actual request
      // on every repair, which is the opposite of what a repair is for.
      const applied = applyOutput(def, text, input, renderContext)
      if (applied.ok) {
        value = applied.value
        schemaValid = true
        failure = null
        break
      }
      failure = applied.error
      if (repairs >= maxRepairs) break

      // THE REPAIR TURN IS THE ONE PLACE THIS RUNNER PUTS MODEL OUTPUT BACK
      // INTO A MODEL'S CONTEXT, so it goes through the gate-safe rules first.
      // guardrails.ts's cardinal invariant - flagged content never re-enters a
      // model's context - is documented at the top of that file, and a runner
      // that re-asks with a reply containing a live credential would be the
      // thing that breaks it. A flagged reply is not repaired; it fails, which
      // is the correct outcome for a reply we would refuse to hand back anyway.
      // Grounded against the same material as the guard pass below: a reply
      // flagged only for quoting the order number that was in its own prompt is
      // not a leak, and refusing it a repair over one would spend the run's only
      // second chance on a finding nobody will ever file.
      const gate = await deps.guardText(text, groundText).catch(() => [] as Finding[])
      if (gate.length) {
        mergeFindings(findings, gate)
        // The REPAIR PROMPT carries the parser error and nothing else. Note what
        // is NOT interpolated here: the finding, its message, or above all its
        // `snippet` - which is a verbatim excerpt of the flagged content and
        // would put the credential straight back into the model's context while
        // ostensibly enforcing the rule against doing so.
        failure = `${applied.error} (not repaired: the reply was flagged by the guard)`
        break
      }

      repairs++
      sent = [...sent, { role: 'assistant', content: text }, { role: 'user', content: repairPrompt(applied.error) }]
    }
  } catch (err) {
    return fail({
      ...empty,
      model: routed,
      step,
      widened,
      repairs,
      // Whatever arrived before the throw. A transport that died mid-stream
      // still leaves the partial reply, and that partial IS the diagnosis.
      //
      // `answered` stays FALSE regardless, from `empty` - a partial is not an
      // answer. Three adapters used `raw !== null` as the "did the model answer"
      // test, so a stream that died after three tokens read to them as a model
      // that answered badly, and the transport's own sentence went nowhere.
      raw: rawOf(text || streamed),
      error: `harness "${def.id}" could not reach "${routed}": ${err instanceof Error ? err.message : String(err)}`,
    })
  }

  // 7 ─ Guard, with an HONEST `Available` for the transport that actually ran.
  //
  //   gateway: we hold the entire message history, so results and error info
  //            are genuinely available. (A harness turn carries no tool
  //            messages, so the record is empty - which is a complete record,
  //            not a missing one, and zero_tool_claim is right to run on it.)
  //            UNLESS the harness offered tool definitions and the model called
  //            one: nothing executed that call, so this turn has names and no
  //            results, which is the fleet's situation and gets the fleet's
  //            answer. See `namesOnly` below.
  //   fleet:   the persona's tool loop ran inside the agent and the stream gave
  //            us tool NAMES only, so ungrounded_ref and fabricated_outage are
  //            SKIPPED rather than guessed. This is guardChatReply's exact
  //            precedent, deliberately.
  //
  // The tool names are handed back through `extractToolRecord` rather than
  // filtered here, so "which tools count as a real external action" keeps
  // exactly one owner (guardrails.ts's NONBACKING set).
  //
  //   ground:  the harness handed over the turn's REAL tool record from its own
  //            input, which no transport is in a position to derive — see
  //            `Grounding` in define.ts. This is the only way `ungrounded_ref`
  //            can fire from a harness at all, and it OVERRIDES both branches
  //            above, including the fleet one: research's synthesis stage runs
  //            on a persona, and the search hits it is grounded against are
  //            external to that persona's own tool loop entirely.
  const config = await deps.guardConfig().catch(() => null)
  const material = groundingFor(def, input)
  if (config && config.mode !== 'off' && text) {
    const overflowed = material !== null && material.results.length > GROUND_RESULTS_CAP
    // NAMES WITHOUT RESULTS — true on the fleet path always, and on the gateway
    // path exactly when this harness OFFERED tool definitions and the model
    // called one. The gateway case is new with `TransportRequest.toolDefs` and
    // it gets the fleet's treatment for the same reason: a tool CALL is not a
    // tool RESULT. Nothing executed it, so there is no result text to ground a
    // citation against, and feeding the names in as backing tools with an empty
    // `resultsText` would make `ungrounded_ref` fire on every id in the reply.
    // `overflowed: true` is guardrails.ts's own "I have the names, not the
    // material" state, and it fails those rules OPEN.
    const namesOnly = kind === 'fleet' || toolNames.length > 0
    const available: Available = material
      ? { results: true, errorInfo: material.errored !== null }
      : namesOnly
        ? { results: false, errorInfo: false }
        : { results: true, errorInfo: true }
    const derived: ToolRecord = namesOnly
      ? {
          ...extractToolRecord([{ role: 'assistant', tool_calls: toolNames.map((name) => ({ function: { name } })) }]),
          overflowed: true,
        }
      : extractToolRecord(sent)
    const toolRecord: ToolRecord = material
      ? {
          backingTools: material.tools,
          resultsText: overflowed ? '' : material.results,
          anyError: material.errored === true,
          overflowed,
        }
      : derived
    const hits = runGuardrails(
      { answer: text, toolRecord, userMessage: lastUserMessage(sent), inputText: groundText, policedHosts: config.policedHosts },
      narrowGuardConfig(config, def.guard?.rules),
      available,
    )
    mergeFindings(findings, hits)
  }
  if (findings.length && config) {
    await deps.recordFindings(findings, { caller: ctx.caller, model: routed, endpoint: kind === 'fleet' ? 'fleet' : endpoint, mode: config.mode }).catch(() => {})
  }

  // Redaction happens on the RAW REPLY and the contract is then re-applied, so
  // a redacted value is a value that still satisfies the schema - never a
  // half-scrubbed object. If the redacted form no longer parses, the harness
  // gets nothing: handing back a value with a live credential in it because the
  // clean version failed to validate would be the worst of both.
  //
  // THE WHOLE CONTRACT IS RE-APPLIED, `verify` included, which is the fourth of
  // the four bugs that shared one cause: a value can survive being cut in half
  // and still parse. A schema says the field is a string; only the harness can
  // say the string still has to be the thing that was asked for.
  //
  // GROUNDED THE SAME WAY THE FINDING WAS. Without `groundText` here the guard
  // correctly declines to blame the model for an identifier out of its own
  // prompt and the redactor rewrites it anyway — which is the worse half of the
  // bug, because the finding is out-of-band telemetry and the value is the
  // artifact a human reads. Credentials come out regardless of grounding;
  // `redactSecrets` and `secret_leak` own that asymmetry between them.
  if (def.guard?.redact && value !== null && needsRedaction(findings)) {
    const safe = redactSecrets(text, groundText)
    if (safe.redacted) {
      const reapplied = applyOutput(def, safe.text, input, renderContext)
      if (reapplied.ok) {
        value = reapplied.value
      } else {
        value = null
        schemaValid = false
        failure = 'the output contained a credential and the redacted form no longer satisfies the contract'
      }
    }
  }

  // 8 ─ The declared failure policy.
  let error: string | undefined = value === null ? (failure ?? 'the harness produced no value') : undefined
  let escalate = false
  if (value === null) {
    const onFailure = def.onFailure
    if (onFailure === 'throw') {
      // Through the same `fail` as every pre-call exit, so the row is written
      // before the throw either way - a throwing harness is precisely the one an
      // operator needs to see in the fitness data - and so there is exactly one
      // sentence in the tree for "this harness failed".
      return fail({ value: null, model: routed, step, widened, repairs, schemaValid: false, escalate: false, answered, refused: false, raw: rawOf(text), error: error ?? 'the harness produced no value' })
    }
    if (typeof onFailure === 'object' && 'fallback' in onFailure) {
      // schemaValid stays FALSE. The fallback is the caller's declared safe
      // answer, not evidence that the model produced one, and conflating the
      // two would quietly inflate every contract rate in the fitness matrix.
      value = onFailure.fallback
    } else if (typeof onFailure === 'object' && onFailure.escalate) {
      escalate = true
      error = `${error} - escalate to a human`
    }
  }

  // 9 ─ Meter. The transports own the token ledger (the gateway writes its own
  // row inside completeViaGateway; the fleet transport writes one itself), so
  // all that is left here is the harness_runs row - the production ground truth
  // behind contract rate and repair rate per harness per model over time.
  return finish({ value, model: routed, step, widened, repairs, schemaValid, escalate, answered, refused: false, raw: rawOf(text), ...(error ? { error } : {}) })
}
