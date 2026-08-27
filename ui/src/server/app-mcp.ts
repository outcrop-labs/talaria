// In-process MCP dispatcher for Talaria apps. An app's mcp.ts tools are
// registered in the MCP registry as a server row (app_slug set); the gateway
// resolves access with the SAME rules as any server (per-agent subsets ∩
// per-person allowances), then — instead of proxying over HTTP — dispatches
// here. No hop, no credentials, and the tool list the registry caches comes
// straight from the module.
import { storeFor } from './app-store'
import { dispatchJsonRpc, rpcError, type Rpc } from './mcp-jsonrpc'
import type { AppMcp, AppMcpTool } from '@/sdk/server'

const MCP_MODS = import.meta.glob('../../../apps/*/mcp.ts') as Record<string, () => Promise<unknown>>

const slugOf = (path: string): string => /apps\/([^/]+)\//.exec(path)?.[1] ?? path

export const appHasMcp = (slug: string): boolean => Object.keys(MCP_MODS).some((p) => slugOf(p) === slug)

export const appsWithMcp = (): string[] => Object.keys(MCP_MODS).map(slugOf)

async function loadMcp(slug: string): Promise<AppMcp | null> {
  const entry = Object.entries(MCP_MODS).find(([p]) => slugOf(p) === slug)
  if (!entry) return null
  const mod = (await entry[1]()) as { default?: AppMcp }
  return mod.default?.tools ? mod.default : null
}

/** The registry's tool-catalog source for app servers (no HTTP round trip). */
export async function appMcpTools(slug: string): Promise<Array<{ name: string; description?: string }>> {
  const mcp = await loadMcp(slug)
  return (mcp?.tools ?? []).map((t) => ({ name: t.name, description: t.description.slice(0, 300) }))
}

/** Handle one JSON-RPC message for an app's MCP surface. `allowed` is the
 *  gateway-resolved allowlist (null = all tools) — enforced here too so the
 *  dispatcher is safe even if called from elsewhere. The method envelope is
 *  the shared dispatcher; what this surface adds is the tool source (the
 *  app's mcp.ts module), the catalog shape, and the handler context. */
export async function dispatchAppMcp(
  slug: string,
  rpc: Rpc,
  agent: string,
  allowed: string[] | null,
): Promise<{ status: number; body: unknown | null }> {
  const mcp = await loadMcp(slug)
  if (!mcp) return { status: 404, body: rpcError(rpc.id, -32601, `app "${slug}" has no MCP surface`) }
  const tools = mcp.tools.filter((t) => allowed === null || allowed.includes(t.name))

  return dispatchJsonRpc(rpc, tools, {
    serverName: `talaria-app-${slug}`,
    listEntry: (t: AppMcpTool) => ({
      name: t.name,
      description: t.description,
      inputSchema: t.inputSchema ?? { type: 'object', properties: {} },
    }),
    call: async (tool, args) => {
      const out = await tool.handler(args, { app: slug, agent, store: storeFor(slug) })
      return { text: typeof out === 'string' ? out : JSON.stringify(out ?? null, null, 2) }
    },
  })
}
