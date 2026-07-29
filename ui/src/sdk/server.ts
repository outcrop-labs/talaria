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
