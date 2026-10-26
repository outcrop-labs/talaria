// THE BOUND RUNNER — `ctx.harness` on every context an app's code runs in.
//
// WHY IT EXISTS. The SDK used to hand apps the bare runner (`runHarness` with a
// `RunContext`), and `RunContext` let an app say who it was (`caller`), run any
// fleet agent — another member's personal assistant included — and swap out
// the guard and the ledger through `deps`. The app decided everything the
// platform is supposed to decide about it.
//
// So the HOST builds the runner, bound to the caller it already authenticated:
//
//   - every run is stamped `app:<slug>:<harnessId>` and booked to ledger source
//     `app` — the app cannot name itself, and its spend is findable as its own;
//   - the harness gets its own admin-assignable model setting, the slot
//     `app:<slug>/<harnessId>` (`ModelSpec.slot`, resolved before `role`);
//   - an agent may be pinned only as the caller may: from a request, an agent
//     the signed-in member may use (the api's own `usable_agent_gate`, asked
//     through `GET /api/agents` with their cookie); from an MCP tool, only the
//     agent calling the tool, and only on its own key; from a trigger, none;
//   - a gateway model pin is held to the member model allowlist
//     (`modelAllowedFor`) for the person the run acts for, and the chain's own
//     steps are gated for them as they are for every user-scoped harness;
//   - the definition is held to the platform's harness rules (the JSON floor,
//     real guard rule ids, a floor inside `requires` — `checkAppHarness`), so a
//     definition built without `defineHarness` gets the floor anyway;
//   - and there is no `deps`: nothing an app passes reaches the guard, the
//     transport or the recorders.
//
// ONE FACTORY, every context. The request context (apps.$app.$.ts), the MCP
// tool context (app-mcp.ts) and the trigger context call `appHarnessRunner`
// with the actor they authenticated; nothing else builds one.
// NOTHING HEAVY AT LOAD. The factory is called on every app request and every
// tool call, and the modules that build those contexts must not carry the
// whole harness layer to do it: the runner, the registry checks and the model
// catalog are imported on a run's first use (`realEdges`).
import type { Role } from './users'
import type { GatewayModel } from './llm-gateway'
import type { HarnessCheck } from './app-harnesses'
import type { HarnessDefinition } from './harness/define'
import type { HarnessResult, RunContext, RunLedger, runHarness as RunHarnessFn, runHarnessStreamed as RunStreamedFn } from './harness/run'
import type { StreamingTransport } from './harness/transport'
import type { LoopEdges, ToolOutcome } from './harness/run'
import type { OrgToolCall } from './app-org-tools'
import type { AppMcpCallFacts } from './app-mcp'

/** What an app may say about one run. Everything else — who is calling, where
 *  the spend goes, the guard, the recorders — the host decides. */
export interface HarnessRunOptions {
  /** Pin the model: a gateway model id, or a fleet agent's id (or one of its
   *  tier ids). Checked against the caller before anything is spent — see the
   *  file header. Omit it and the harness's own chain resolves, with its
   *  admin-assigned slot first. */
  model?: string
  /** Route an agent pin to one of its alias tiers (`'opus'`), priced as that
   *  tier. */
  tier?: string
  /** A reasoning effort the person picked, from the model's published levels. */
  effort?: string
  /** Where else this run's spend belongs: a ticket (`taskId`) or a record of
   *  yours (`refId`). Both are UUIDs — the ledger's columns are — and a value
   *  that is not one is left off the row, with a warning, rather than losing
   *  the row. The source is always `app`. */
  ledger?: { refId?: string | null; taskId?: string | null }
  /** Your own signal, on top of the one the host already passes (the request's
   *  disconnect, the tool's deadline). */
  signal?: AbortSignal
}

export interface HarnessStreamOptions extends HarnessRunOptions {
  /** Each delta as it lands, before any guard pass — see `runHarnessStreamed`. */
  onDelta?: (delta: string) => void
}

/** `ctx.harness`. */
export interface AppHarnessRunner {
  /** Run a harness as this caller: resolve, floor, render, call, parse,
   *  repair, guard, meter, `harness_runs` row. */
  run<I, O>(def: HarnessDefinition<I, O>, input: I, opts?: HarnessRunOptions): Promise<HarnessResult<O>>
  /** The same run, streamed: `onDelta` gets each piece as it lands. A streamed
   *  run makes no repair attempt — the first answer is already on screen. */
  stream<I, O>(def: HarnessDefinition<I, O>, input: I, opts?: HarnessStreamOptions): Promise<HarnessResult<O>>
}

/** Who a bound run acts as — what the host authenticated before the app ran. */
export type HarnessActor =
  /** A signed-in member, through `/api/apps/<slug>/*`. `usableAgents` lists
   *  the agent ids they may use — the api's `usable_agent_gate`, asked with
   *  their own session. */
  | { kind: 'request'; user: { id: string; role: Role }; usableAgents: () => Promise<string[]> }
  /** An agent calling one of the app's MCP tools: who it is, whether the
   *  gateway proved it, and the person it acts for. */
  | { kind: 'mcp'; caller: { name: string; id: string | null; proven: boolean }; actingUserId: string | null }
  /** A scheduled or event-triggered run: the app itself, with no user. */
  | { kind: 'trigger' }

export interface AppHarnessBinding {
  /** The app's slug. */
  app: string
  actor: HarnessActor
  /** How a loop's `org` tool is called — as the agent that called the app's
   *  tool, through the MCP gateway (app-org-tools.ts). Only an MCP context
   *  with a proven caller has one; everywhere else an org tool answers the
   *  model that it is unavailable, and why. */
  orgTools?: OrgToolCall | null
  /** The context's own signal — the request's disconnect, the tool's
   *  deadline. Every run follows it. */
  signal?: AbortSignal
  /** Test seam: the edges this module reads. Never reachable from app code. */
  edges?: Partial<RunnerEdges>
}

/** Everything the bound runner reads from the rest of the host. */
export interface RunnerEdges {
  /** The fleet agent a model id runs on, or null for a gateway model. */
  fleetAgentOf: (model: string) => Promise<string | null>
  userRole: (userId: string) => Promise<Role>
  allowlist: () => Promise<string[]>
  catalog: () => Promise<GatewayModel[]>
  /** `modelAllowedFor` — the member model allowlist rule. */
  allowed: (role: Role, model: string, allow: string[], catalog: GatewayModel[]) => boolean
  /** The platform's harness rules (`checkAppHarness`). */
  check: (def: unknown) => HarnessCheck
  /** `app:<slug>/<harnessId>` (`appHarnessSlot`). */
  slot: (app: string, harnessId: string) => string
  run: typeof RunHarnessFn
  runStreamed: typeof RunStreamedFn
  streamTransport: (model: string) => Promise<StreamingTransport>
  /** One of the app's own MCP tools, through the app MCP dispatch. */
  ownTool: (slug: string, tool: string, args: Record<string, unknown>, facts: AppMcpCallFacts) => Promise<ToolOutcome>
  warn: (message: string) => void
}

const warned = new Set<string>()
const warnOnce = (message: string): void => {
  if (warned.has(message)) return
  if (warned.size > 500) warned.clear()
  warned.add(message)
  console.warn(message)
}

let real: Promise<RunnerEdges> | null = null
/** The real edges, imported on first use and kept. */
const realEdges = (): Promise<RunnerEdges> =>
  (real ??= (async () => {
    const [users, gateway, access, harnesses, models, run, transport, mcp] = await Promise.all([
      import('./users'),
      import('./llm-gateway'),
      import('./model-access'),
      import('./app-harnesses'),
      import('./app-harness-models'),
      import('./harness/run'),
      import('./harness/transport'),
      import('./app-mcp'),
    ])
    return {
      fleetAgentOf: transport.fleetAgentOf,
      userRole: users.getUserRole,
      allowlist: access.memberModelAllowlist,
      catalog: gateway.gatewayModels,
      allowed: access.modelAllowedFor,
      check: harnesses.checkAppHarness,
      slot: models.appHarnessSlot,
      run: run.runHarness,
      runStreamed: run.runHarnessStreamed,
      streamTransport: transport.pickStreamingTransport,
      ownTool: mcp.callOwnAppTool,
      warn: warnOnce,
    }
  })())

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** The run a refusal returns: nothing was asked, nothing was spent. */
function refusedRun<O>(model: string | null, error: string): HarnessResult<O> {
  return {
    value: null,
    model,
    step: null,
    widened: false,
    repairs: 0,
    schemaValid: false,
    answered: false,
    refused: false,
    findings: [],
    raw: null,
    latencyMs: 0,
    escalate: false,
    error,
  }
}

/** Both signals, as one: the context's and the caller's own. */
function eitherSignal(a: AbortSignal | undefined, b: AbortSignal | undefined): AbortSignal | undefined {
  if (!a) return b
  if (!b) return a
  return AbortSignal.any([a, b])
}

/** THE ONE FACTORY. Build `ctx.harness` for a context the host authenticated. */
export function appHarnessRunner(binding: AppHarnessBinding): AppHarnessRunner {
  const { app, actor } = binding
  const actingUserId = actor.kind === 'request' ? actor.user.id : actor.kind === 'mcp' ? actor.actingUserId : null

  /** Why this caller may not pin `model`, or null. */
  const pinRefusal = async (edges: RunnerEdges, harnessId: string, model: string): Promise<string | null> => {
    const agent = await edges.fleetAgentOf(model).catch(() => null)
    const whose = `app "${app}" may not run harness "${harnessId}" on`
    if (agent !== null) {
      if (actor.kind === 'trigger') {
        return `${whose} agent "${agent}": a triggered run acts as the app, with no user, and drives no agent through ctx.harness`
      }
      if (actor.kind === 'mcp') {
        if (!actor.caller.proven) {
          return `${whose} agent "${agent}": the calling agent "${actor.caller.name}" was not proven by its own key, so its identity cannot be lent to a run`
        }
        if (agent !== actor.caller.name) {
          return `${whose} agent "${agent}": from an MCP tool, a harness may run only on the agent calling the tool ("${actor.caller.name}")`
        }
        return null
      }
      const usable = await actor.usableAgents().catch(() => null)
      if (usable === null) return `${whose} agent "${agent}": the agents this person may use could not be read — try again`
      if (!usable.includes(agent)) return `${whose} agent "${agent}": the signed-in person may not use that agent`
      return null
    }
    // A gateway model: the member allowlist, for the person the run acts for.
    // A run with no person (a trigger, an org agent's tool call) has no one to
    // gate, the same as an org-scoped platform harness.
    if (actingUserId === null) return null
    const [role, allow, catalog] = await Promise.all([edges.userRole(actingUserId), edges.allowlist(), edges.catalog()])
    if (!edges.allowed(role, model, allow, catalog)) {
      return `${whose} "${model}": that model is not open to the person this run acts for (Admin → Models → Member access)`
    }
    return null
  }

  /** The definition and context a run gets, or the sentence that refuses it. */
  const prepare = async <I, O>(
    edges: RunnerEdges,
    def: HarnessDefinition<I, O>,
    opts: HarnessRunOptions,
  ): Promise<{ ok: true; def: HarnessDefinition<I, O>; ctx: RunContext } | { ok: false; error: string; model: string | null }> => {
    const checked = edges.check(def)
    if (!checked.ok) {
      const id = checked.id ?? '(no id)'
      return { ok: false, model: null, error: `app "${app}" harness "${id}" does not meet the platform's harness rules: ${checked.problems.join('; ')}` }
    }
    const id = checked.def.id
    if (opts.model) {
      const refusal = await pinRefusal(edges, id, opts.model)
      if (refusal) return { ok: false, model: opts.model, error: refusal }
    }
    const ledger: RunLedger = { source: 'app' }
    for (const key of ['refId', 'taskId'] as const) {
      const v = opts.ledger?.[key]
      if (v == null) continue
      if (typeof v === 'string' && UUID.test(v)) ledger[key] = v
      else edges.warn(`[app-harness] app "${app}" harness "${id}": ledger.${key} ${JSON.stringify(v)} is not a UUID — booked without it`)
    }
    const signal = eitherSignal(binding.signal, opts.signal)
    // The JSON floor derived, the slot stamped by the host — whatever the app's
    // own spec said about either.
    const bound = { ...(checked.def as unknown as HarnessDefinition<I, O>), model: { ...checked.def.model, slot: edges.slot(app, id) } }
    // WHERE A LOOP TOOL'S CALL GOES. The app's own MCP tools dispatch in
    // process, called by the agent that called the app (from a tool) or by
    // this run itself, which the host vouches for; an org tool only as a
    // proven calling agent, through the gateway.
    const toolCaller =
      actor.kind === 'mcp' ? actor.caller : { name: `app:${app}:${id}`, id: null, proven: true }
    const loop: LoopEdges = {
      app,
      actingUserId,
      mcp: (tool, args, sig) => edges.ownTool(app, tool, args, { caller: toolCaller, actingUserId, signal: sig }),
      ...(binding.orgTools ? { org: binding.orgTools } : {}),
      orgUnavailable:
        actor.kind === 'mcp'
          ? 'org tools run as the agent that called this app, and that agent did not call on its own key — so there is no agent to run them as'
          : actor.kind === 'request'
            ? 'org tools run as an agent, under that agent\'s grants, and this run was started by a person, not an agent — run it from an MCP tool to use org tools'
            : 'org tools run as an agent, under that agent\'s grants, and a triggered run has none',
    }
    const ctx: RunContext = {
      caller: `app:${app}:${id}`,
      loop,
      ...(actingUserId ? { userId: actingUserId } : {}),
      ...(opts.model ? { model: opts.model } : {}),
      ...(opts.tier ? { tier: opts.tier } : {}),
      ...(opts.effort ? { effort: opts.effort } : {}),
      ledger,
      ...(signal ? { signal } : {}),
    }
    return { ok: true, def: bound, ctx }
  }

  const refuse = <I, O>(edges: RunnerEdges, def: HarnessDefinition<I, O>, model: string | null, error: string): HarnessResult<O> => {
    edges.warn(`[app-harness] ${error}`)
    // The declared failure policy, as the runner applies it to every pre-call
    // failure: 'throw' throws, the rest return the failed run.
    if (def?.onFailure === 'throw') throw new Error(error)
    return refusedRun<O>(model, error)
  }

  const edgesOf = async (): Promise<RunnerEdges> => ({ ...(await realEdges()), ...binding.edges })

  return {
    async run(def, input, opts = {}) {
      const edges = await edgesOf()
      const p = await prepare(edges, def, opts)
      if (!p.ok) return refuse(edges, def, p.model, p.error)
      return edges.run(p.def, input, p.ctx)
    },
    async stream(def, input, opts = {}) {
      const edges = await edgesOf()
      const p = await prepare(edges, def, opts)
      if (!p.ok) return refuse(edges, def, p.model, p.error)
      return edges.runStreamed(p.def, input, p.ctx, {
        stream: async (req, emit) => (await edges.streamTransport(req.model))(req, emit),
        ...(opts.onDelta ? { onDelta: opts.onDelta } : {}),
      })
    },
  }
}

// ── The free runner, as the SDK exports it ──────────────────────────────────

/** `RunContext` without `deps` or `loop` — what the SDK's free `runHarness`
 *  accepts. Both are the host's to supply, never the app's. */
export type AppRunContext = Omit<RunContext, 'deps' | 'loop'>

/** THE FREE `runHarness`, kept for apps written before `ctx.harness`. It runs
 *  what it is given — the caller string, the pin and the ledger are the app's
 *  word — so it is the road with no checks, and says so:
 *
 *   - `deps` is not part of its type and is dropped if passed anyway: nothing
 *     an app hands over replaces the guard, the transport or the recorders
 *     (`@talaria/sdk/testing` runs a harness against scripted replies);
 *   - pinning a fleet agent logs a warning, because nothing here checks the
 *     caller may use that agent — `ctx.harness.run` does. */
export async function runHarnessForApp<I, O>(
  def: HarnessDefinition<I, O>,
  input: I,
  ctx: AppRunContext,
  edges: Partial<Pick<RunnerEdges, 'fleetAgentOf' | 'run' | 'warn'>> = {},
): Promise<HarnessResult<O>> {
  const e = { ...(await realEdges()), ...edges }
  // `loop` goes too: where a loop tool's call goes is the host's to say. The
  // free runner has no host binding, so a loop run here runs its handlers and
  // answers `mcp`/`org` tools with the reason they are unavailable.
  const { deps, loop: _loop, ...rest } = ctx as RunContext
  if (deps !== undefined) {
    e.warn(`[sdk] runHarness("${def?.id}") was passed deps from app code; they are ignored. Test a harness with @talaria/sdk/testing's runHarnessOffline.`)
  }
  if (rest.model) {
    const agent = await e.fleetAgentOf(rest.model).catch(() => null)
    if (agent !== null) {
      e.warn(
        `[sdk] runHarness("${def?.id}") pins agent "${agent}" with no check that the caller may use it. Run it through ctx.harness.run, which checks.`,
      )
    }
  }
  return e.run(def, input, rest)
}
