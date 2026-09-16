// In-process MCP dispatcher for Talaria apps. An app's mcp.ts tools are
// registered in the MCP registry as a server row (app_slug set); the gateway
// resolves access with the SAME rules as any server, then dispatches here.
// The module is loaded from disk (dev) or the app's own build (prod) — never
// a host glob.
import { storeFor } from './app-store'
import { dispatchJsonRpc, rpcError, type Rpc } from './mcp-jsonrpc'
import type { AppMcp, AppMcpTool } from '@/sdk/server'
import { isolateApp } from './app-isolate'
import { appHasMcp as hasMcpFile, loadAppMcp } from './app-load'

export const appHasMcp = (slug: string): boolean => hasMcpFile(slug)

async function loadMcp(slug: string): Promise<AppMcp | null> {
  const loaded = await isolateApp(slug, 'mcp load', async () => {
    const mod = (await loadAppMcp(slug)) as { default?: AppMcp } | null
    return mod?.default?.tools ? mod.default : null
  })
  return loaded.ok ? loaded.value : null
}

export async function appMcpTools(slug: string): Promise<Array<{ name: string; description?: string }>> {
  const mcp = await loadMcp(slug)
  return (mcp?.tools ?? []).map((t) => ({ name: t.name, description: t.description.slice(0, 300) }))
}

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
