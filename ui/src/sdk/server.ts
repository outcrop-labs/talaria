// @talaria/sdk/server — the server half of a Talaria app.
//
// apps/<slug>/server.ts default-exports defineAppServer(...): one fetch
// handler mounted at /api/apps/<slug>/*. The host authenticates the request
// (session cookie), checks the app is enabled and the user holds at least one
// of its surfaces, then hands over with a context: the signed-in user, which
// surfaces and permissions they hold, the sub-path, a per-app document store
// (namespaced Postgres, no migrations needed), the platform API as the
// caller, the secrets the app was given, and the request's abort signal.
import { json } from '@/server/http'
import type { SessionUser } from '@/server/auth/session'
import type { AppStore } from '@/server/app-store'
import type { PlatformApi, PlatformApiInit } from '@/server/app-context'

export { json }
// What `ctx.api` throws on a non-2xx answer: `status` and the server's own
// sentence as `message`. The same class the host's client door throws.
export { HttpError } from '@/lib/fetch-json'
export type { PlatformApiInit }
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
  /** The signed-in user this request runs as. */
  user: SessionUser
  /** Which of this app's surfaces the caller holds. Each surface is its own
   *  grant (the settings tab follows the manage grant); the host admits a
   *  caller who holds at least one, so a route behind your Manage surface
   *  must check `access.manage` itself. An admin holds every surface the app
   *  ships; a surface the app doesn't ship is always false. */
  access: { admin: boolean; work: boolean; manage: boolean; settings: boolean }
  /** The caller's effective platform permissions (`boards.create`,
   *  `kb.edit`, …) — person, team and org defaults resolved. */
  perms: readonly string[]
  /** Whether the caller holds `perm`. Admins hold every permission. */
  can: (perm: string) => boolean
  /** This app's slug. */
  app: string
  /** Path after /api/apps/<slug>/, decoded once — e.g. "items/123". Every
   *  path under /api/apps/<slug>/ is the app's; the host's own app routes
   *  live under /api/app-host/. */
  path: string
  /** The same sub-path as decoded segments — `["items", "123"]`. A segment
   *  never holds a `/`: a path with an encoded slash (%2F) is refused with a
   *  400 before the handler runs. Route and authorize on these, not on a
   *  prefix of `path`. */
  segments: readonly string[]
  url: URL
  /** The origin people reach this instance at — `AUTH_PUBLIC_URL` when the
   *  operator set it, else what the proxy in front stated
   *  (`x-forwarded-proto`/`-host`), else the request's own. Build absolute
   *  links from this, not from `url`, which is plain http behind TLS. */
  origin: string
  /** Namespaced document store: collections of JSON docs owned by this app. */
  store: AppStore
  /** Call the platform API as the caller: `ctx.api('/api/boards')`. Their
   *  session cookie goes along, so every permission and ACL applies as in
   *  the core UI. Resolves to the parsed JSON (null for an empty body);
   *  throws an `HttpError` (status + the server's sentence) on non-2xx.
   *  A read follows `signal`; a write runs to completion after the caller
   *  leaves, unless you pass `signal` to it yourself. */
  api: PlatformApi
  /** A workspace secret's value — `ctx.secret('crm')` or
   *  `ctx.secret('crm.api_key')` — or null. Resolves only for a vault doc an
   *  admin gave this app by listing `talaria-app://<slug>` in its allowed
   *  hosts; every read is audited. Never send the value to the browser. */
  secret: (name: string) => Promise<string | null>
  /** Aborts when the caller disconnects before the answer is done. Pass it to
   *  long work (`runHarness`, `fetch`) so it stops spending for nobody. */
  signal: AbortSignal
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

// ── Activity harnesses ─────────────────────────────────────────────────────
// defineHarness(...) declares a MODEL CALL Talaria makes on the app's behalf —
// a prompt, an output contract, a model chain and a failure policy, executed
// by the one runner (`runHarness`), which brings model resolution, the
// capability floor, the guardrail pass, ledger attribution, the repair turn
// on malformed JSON and a `harness_runs` row.
//
// WHERE IT LIVES: one per file, default-exported from
// `apps/<slug>/harnesses/*.ts`. While the app is enabled the host checks each
// one against the platform's harness rules and registers it
// (server/app-harnesses.ts): it gets a column in Admin → Models → Fitness and
// the sweep replays its `evals` against each candidate model. A harness that
// fails a check is skipped and named on the app's row in Manage → Apps; the
// app keeps running. In production your server.ts runs it with `runHarness`
// (the bridge below).
//
// It carries CODE: `render` builds the messages, `output.clean`/`output.verify`
// decide whether the reply held the contract, `evals[].check` grades it. So it
// can only come from a file the deployment compiled, never from an
// admin-entered JSON row.
//
// (The coding harness an AGENT drives in its sandbox is not an extension point:
// the workbench runs Oh My Pi, and only Oh My Pi.)
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
 *  THE PLATFORM'S OWN defineHarness, not a copy. It checks `render`'s input
 *  type against `output`'s value type at the definition site, and it derives
 *  the JSON floor every platform harness gets: a harness with
 *  `output.kind === 'json'` gains `json` in `floor.capabilities` and
 *  `refuseBelow: true`, so the runner refuses a model that a probe measured
 *  (or an admin declared) unable to produce JSON instead of handing its prose
 *  to the parser. The SDK used to return the definition untouched, which ran
 *  app JSON harnesses on exactly those models. */
export { defineHarness } from '@/server/harness/define'

// ── The runner, from app code ───────────────────────────────────────────────
// How an app harness runs in production: the bridge pattern, where a route
// runs its harness live when a model chain is routable and falls back to its
// own deterministic answer when one isn't, journaling which answered. (The
// fitness sweep replays a registered harness's fixtures through this same
// runner, on the host.) That code sits in apps/<slug>/, where
// the tsconfig-paths plugin doesn't reach — `@/server/harness/run` cannot
// resolve there BY DESIGN (the vite config extends only the SDK ids to app
// files), so the SDK is the only road in and these two are app surface:
//
//   runHarness          — the one runner: resolve, floor, widen, render, call,
//                         parse, repair, guard, redact, meter, `harness_runs`
//                         row. Same function the platform's own TS harnesses
//                         use; an app calling it gets the same accounting.
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
