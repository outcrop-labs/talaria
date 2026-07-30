// @talaria/sdk/server — the server half of a Talaria app.
//
// apps/<slug>/server.ts default-exports defineAppServer(...): one fetch
// handler mounted at /api/apps/<slug>/*. The host authenticates the request
// (session cookie), checks the app is enabled and the user may reach it,
// then hands over with a context: the signed-in user, the sub-path, and a
// per-app document store (namespaced Postgres, no migrations needed).
import { json } from '@tanstack/react-start'
import type { SessionUser } from '@/server/auth/session'
import type { AppStore } from '@/server/app-store'

export { json }
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

// apps/<slug>/harness.ts default-exports defineHarness(...): a coding/work
// harness Hermes agents can drive through their WORKBENCH. The host merges it
// into the harness registry (builtin < app-shipped < admin-custom, by slug):
// it becomes selectable per agent, its auth/env provision into the sandbox at
// render time, its MCP pass-through config is written in its own format, and
// — where it can serve MCP — it registers on the agent's Hermes config as
// stdio tools. Declarative only: no host code runs from the definition.

export interface HarnessDefinition {
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
  /** Image-build layer hints (consumed by the workbench image pipeline). */
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

export const defineHarness = (h: HarnessDefinition): HarnessDefinition => h
