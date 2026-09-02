// @talaria/sdk/server — the server half of a Talaria app.
//
// apps/<slug>/server.ts default-exports defineAppServer(...): one fetch
// handler mounted at /api/apps/<slug>/*. The host authenticates the request
// (session cookie), checks the app is enabled and the user may reach it,
// then hands over with a context: the signed-in user, the sub-path, and a
// per-app document store (namespaced Postgres, no migrations needed).
import { json } from '@/server/http'
import type { SessionUser } from '@/server/auth/session'
import type { AppStore } from '@/server/app-store'
import type { HarnessDefinition } from '@/server/harness/define'

export { json }
// The same validation door the host's own routes use: a zod schema in, the
// validated data or the standard 400 (first issue message) out. Without it,
// every app hand-rolls its own body checks with their own shapes — the
// reference app had three spellings across four endpoints. `z` rides along
// because apps can't import zod themselves: `apps/` sits outside `ui/`, so
// the package's node_modules is not on their resolution path (the same
// reason the SDK exists at all).
export { parseBody } from '@/server/api-guard'
export { z } from 'zod'
export type { SessionUser, AppStore }
export type { AppDoc } from '@/server/app-store'

export interface AppRequestContext {
  /** The signed-in user this request runs as. Role/permission checks are yours to apply. */
  user: SessionUser
  /** This app's slug. */
  app: string
  /** Path after /api/apps/<slug>/ — e.g. "contacts/123". */
  path: string
  url: URL
  /** Namespaced document store: collections of JSON docs owned by this app. */
  store: AppStore
}

export interface AppServer {
  fetch: (request: Request, ctx: AppRequestContext) => Response | Promise<Response>
}

export const defineAppServer = (server: AppServer): AppServer => server

// ── MCP surface ────────────────────────────────────────────────────────────
// apps/<slug>/mcp.ts default-exports defineAppMcp(...): tools the org's
// AGENTS can call. The host registers them as an MCP server in the registry,
// so the whole granular governance applies unchanged — per-agent tool
// subsets, per-person allowances, gateway enforcement. Calls dispatch
// in-process (no network hop); the handler gets the calling agent's name and
// the same per-app store the HTTP server uses.

export interface AppMcpContext {
  /** This app's slug. */
  app: string
  /** The calling agent's name (fleet identity, gateway-authenticated). */
  agent: string
  store: AppStore
}

export interface AppMcpTool {
  name: string
  description: string
  /** JSON Schema for the arguments (defaults to an empty object schema). */
  inputSchema?: Record<string, unknown>
  /** Return value is serialized for the agent (string passes through as-is). */
  handler: (args: Record<string, unknown>, ctx: AppMcpContext) => unknown | Promise<unknown>
}

export interface AppMcp {
  tools: AppMcpTool[]
}

export const defineAppMcp = (mcp: AppMcp): AppMcp => mcp

// ── Workbench harnesses ────────────────────────────────────────────────────
// apps/<slug>/harness.ts default-exports defineWorkbenchHarness(...): a
// coding/work harness Hermes agents drive through their WORKBENCH. The host merges it
// into the harness registry (builtin < app-shipped < admin-custom, by slug):
// it becomes selectable per agent, its auth/env provision into the sandbox at
// render time, its MCP pass-through config is written in its own format, and
// — where it can serve MCP — it registers on the agent's Hermes config as
// stdio tools. Declarative only: no host code runs from the definition.

export interface WorkbenchHarnessDefinition {
  /** Stable id — what profiles and per-agent picks reference. */
  slug: string
  label: string
  description?: string
  /** 'gateway' = OpenAI-compatible; the host points it at Talaria's gateway
   *  (metered, attributed). Otherwise name the provider whose key the org's
   *  endpoint registry provisions, and the env var the harness reads. */
  auth: 'gateway' | { provider: string; envVar: string }
  /** Extra container env (compose-interpolated; merged over the auth env). */
  env?: Record<string, string>
  /** Prefix model ids need for this harness's CLI (e.g. "openai/"). */
  modelPrefix?: string
  /** Invocation template — <model> and <task> placeholders. */
  invoke: string
  /** Structured-output form — REQUIRED for good drivers; agents are taught
   *  to read structured results, never scrape logs. */
  jsonInvoke?: string
  /** How to run the harness AS an MCP server (stdio) — the preferred
   *  integration: agents drive it with tools. */
  mcpServe?: { command: string; args: string[] }
  /** MCP pass-through config the harness reads: written per agent in this
   *  format at render time ('claude-json' = .mcp.json-style, 'opencode-json'
   *  = opencode config, 'custom' = your renderMcpConfig below — app-shipped
   *  harnesses only). Omit if the harness has no MCP client. */
  mcpConfig?: { format: 'claude-json' | 'opencode-json' | 'custom'; filename: string }
  /** Custom pass-through renderer (format: 'custom'; app-shipped only —
   *  admin-JSON definitions can't carry code). Return the JSON-serializable
   *  config your harness reads; you own env-substitution syntax. */
  renderMcpConfig?: (ctx: HarnessMcpRenderContext) => unknown
  /** A cheap command that proves the harness runs in a sandbox (version
   *  check) — surfaced by the workbench doctor for agents to self-verify. */
  probe?: string
  /** What a driving agent should understand: sessions, resume, results. */
  guide: string
  /** RESERVED: image-build layer hints — declared and validated now, consumed
   *  when the workbench image pipeline lands. */
  install?: { npm?: string[]; commands?: string[]; notes?: string }
}

export interface HarnessMcpRenderContext {
  /** The agent this config is rendered for (its fleet model id). */
  agentModel: string
  /** The agent's granted MCP servers, as per-agent gateway endpoints. */
  servers: Array<{ name: string; url: string }>
  /** Env var (set in the container) holding the fleet API key — use your
   *  harness's own env-substitution syntax to reference it in headers,
   *  together with an X-Agent-Name: <agentModel> header. */
  apiKeyEnvVar: string
}

export const defineWorkbenchHarness = (h: WorkbenchHarnessDefinition): WorkbenchHarnessDefinition => h

// ── Activity harnesses ─────────────────────────────────────────────────────
// apps/<slug>/harnesses/*.ts default-export defineHarness(...): a MODEL CALL
// Talaria makes on the app's behalf — a prompt, an output contract, a model
// chain and a failure policy, executed by the one runner (`runHarness`). The
// host merges it into the activity registry (builtin < app-shipped <
// admin-custom, by id), and from that one array the app gets the whole
// platform for free: model resolution, the capability floor, the guardrail
// pass, ledger attribution, the repair turn on malformed JSON, a
// `harness_runs` row — and, if it declares `evals`, ITS OWN COLUMN IN THE
// ORG'S MODEL-FITNESS MATRIX. That last one is the point of shipping fixtures
// in the registry rather than in a test directory: a third-party app can tell
// an admin which of their models it actually works on, for the cost of an
// array.
//
// UNLIKE the workbench definition above, this one carries CODE — `render`
// builds the messages, `output.clean`/`output.verify` decide whether the reply
// held the contract, `evals[].check` grades it. So it can only come from a
// file the deployment compiled, never from an admin-entered JSON row.

// `HarnessDefinition` NOW MEANS THE ACTIVITY ONE, and it takes two type
// arguments, so a file that meant the workbench shape gets "requires 2 type
// arguments" at its own import — a compile error naming both types, rather than
// a silently wrong one. Say `WorkbenchHarnessDefinition` there. The VALUE
// `defineHarness` still accepts either (see the overloads below), because a call
// in an app's `harness.ts` has to keep building.
export type { HarnessDefinition, EvalCase, EvalBand, EvalContext, RoleFloor, RenderContext, Message, Grounding, Verify } from '@/server/harness/define'
export type { CheckResult } from '@/server/harness/define'
export type { Capability } from '@/server/harness/capability'
export type { ModelSpec } from '@/server/harness/model'
// The chain vocabulary and its resolution — referenced by `ModelSpec`/`HarnessResult`;
// a bridge author labeling WHICH step answered has no word for it otherwise.
export type { ModelChainStep, ResolvedHarnessModel } from '@/server/harness/model'
export type { HarnessResult, RunLedger } from '@/server/harness/run'
// `toolDefs` is a field on the definition above, so its element type has to be
// nameable out here or the field is only usable as an inline literal — an author
// who factors four tools into a `const TOOLS = [...]` has no type to annotate it
// with, and infers `parameters: { type: string }` instead. `ToolCall` travels
// with it because it is the other half of the same channel (`TransportReply`),
// and a harness reading a tool record back wants the same word for it.
//
// NOT the MCP `AppMcpTool` above, which is a different system's tool with a
// different envelope (`inputSchema`, a handler Talaria dispatches in-process).
// This one is the OpenAI wire shape, offered on ONE turn, executed by nobody.
export type { ToolDefinition, ToolCall, ToolPolicy } from '@/server/harness/transport'

/** THE FLOOR EVERY ONE-SIDED TEXT FIXTURE NEEDS, exported because an app
 *  author writing `evals` for a text harness walks into the exact trap it was
 *  written for: a `check` that only asserts what the answer must NOT be (too
 *  long, not markdown, not a question) is passed by a model that says almost
 *  nothing. Give it a minimum length and a set of words the answer had to
 *  engage with, and a non-answer scores as one. */
export { belowAnswerFloor } from '@/server/harness/define'
/** The `EvalContext` a fixture receives when no tools ran — every single-shot
 *  harness, which is most of them. Exported so an app author calling a `check`
 *  by hand has the same value the suite would pass. */
export { NO_TOOLS } from '@/server/harness/define'

// `defineEvals` is deliberately absent. Fixtures written inside the
// `defineHarness(...)` call are already contextually typed against the same I
// and O as `render` and `output`, so a wrapper would add a name without adding
// inference; fixtures written apart from their harness need explicit type
// arguments either way, and `EvalCase<I, O>` is exported above to annotate
// them with.

/** Declare an ACTIVITY harness — a model call Talaria runs for your app.
 *
 *  TWO CONTRACTS WORE THIS ONE NAME, and the one-sentence version is: this is
 *  the harness Talaria runs (a prompt and an output contract), and
 *  `defineWorkbenchHarness` is the harness an AGENT runs (a coding CLI in a
 *  sandbox, declared as shell templates and env). Neither is a specialization
 *  of the other.
 *
 *  Identity at runtime; it exists so `render`'s input type and `output`'s value
 *  type are checked against each other at the definition site, which is the one
 *  place an author can get that pair wrong and the last place anyone looks. */
export function defineHarness<I, O>(h: HarnessDefinition<I, O>): HarnessDefinition<I, O>
/** @deprecated Use `defineWorkbenchHarness`.
 *
 *  The overload that keeps the old spelling working: `apps/<slug>/harness.ts`
 *  files and stored `workbench_harness_defs` rows were written against
 *  `defineHarness`, and renaming an extension point out from under third-party
 *  apps is not a rename, it is a break. The two shapes are disjoint — a
 *  workbench definition has `slug`/`invoke`/`guide` and no `render` — so which
 *  contract a call means is never ambiguous, and new code says which it means
 *  by name. */
export function defineHarness(h: WorkbenchHarnessDefinition): WorkbenchHarnessDefinition
export function defineHarness<T>(h: T): T {
  return h
}

// ── The runner, from app code ───────────────────────────────────────────────
// The registry runs app harnesses on the app's behalf (fitness matrix, admin
// panel) — but an app's OWN server code has a legitimate reason to invoke one
// directly: the bridge pattern, where a route runs its harness live when a
// model chain is routable and falls back to its own deterministic answer when
// one isn't, journaling which answered. That code sits in apps/<slug>/, where
// the tsconfig-paths plugin doesn't reach — `@/server/harness/run` cannot
// resolve there BY DESIGN (the vite config extends only the SDK ids to app
// files), so the SDK is the only road in and these two are app surface:
//
//   runHarness          — the one runner: resolve, floor, widen, render, call,
//                         parse, repair, guard, redact, meter, `harness_runs`
//                         row. Same function the registry uses; an app calling
//                         it gets the same accounting.
//   resolveHarnessModel — the free probe (no model call): what chain would
//                         carry a harness with this spec, or null. A bridge
//                         labels itself honestly without spending a turn.
//
// `RunContext` rides along by the same argument as `ToolDefinition` above: a
// bridge that builds its context apart from the call (caller now, signal from
// the request) has no type to annotate it with otherwise.
export { runHarness } from '@/server/harness/run'
export type { RunContext } from '@/server/harness/run'
export { resolveHarnessModel } from '@/server/harness/model'
