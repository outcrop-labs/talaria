// In-process MCP dispatcher for Talaria apps. An app's mcp.ts tools are
// registered in the MCP registry as a server row (app_slug set); the gateway
// resolves access with the SAME rules as any server (per-agent subsets ∩
// per-person allowances), then — instead of proxying over HTTP — dispatches
// here. No hop, no credentials, and the tool list the registry caches comes
// straight from the module.
import { storeFor } from './app-store'
import { MCP_PROTOCOL_VERSION } from './mcp-protocol'
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

interface Rpc {
  jsonrpc?: string
  id?: unknown
  method?: string
  params?: { name?: string; arguments?: Record<string, unknown>; [k: string]: unknown }
}

const result = (id: unknown, res: unknown) => ({ jsonrpc: '2.0', id: id ?? null, result: res })
const rpcError = (id: unknown, code: number, message: string) => ({ jsonrpc: '2.0', id: id ?? null, error: { code, message } })

/** Handle one JSON-RPC message for an app's MCP surface. `allowed` is the
 *  gateway-resolved allowlist (null = all tools) — enforced here too so the
 *  dispatcher is safe even if called from elsewhere. */
export async function dispatchAppMcp(
  slug: string,
  rpc: Rpc,
  agent: string,
  allowed: string[] | null,
): Promise<{ status: number; body: unknown | null }> {
  const mcp = await loadMcp(slug)
  if (!mcp) return { status: 404, body: rpcError(rpc.id, -32601, `app "${slug}" has no MCP surface`) }
  const tools = mcp.tools.filter((t) => allowed === null || allowed.includes(t.name))

  switch (rpc.method) {
    case 'initialize':
      return {
        status: 200,
        body: result(rpc.id, {
          protocolVersion: (rpc.params?.protocolVersion as string) ?? MCP_PROTOCOL_VERSION,
          capabilities: { tools: {} },
          serverInfo: { name: `talaria-app-${slug}`, version: '1.0' },
        }),
      }
    case 'notifications/initialized':
      return { status: 202, body: null }
    case 'ping':
      return { status: 200, body: result(rpc.id, {}) }
    case 'tools/list':
      return {
        status: 200,
        body: result(rpc.id, {
          tools: tools.map((t: AppMcpTool) => ({
            name: t.name,
            description: t.description,
            inputSchema: t.inputSchema ?? { type: 'object', properties: {} },
          })),
        }),
      }
    case 'tools/call': {
      const tool = tools.find((t) => t.name === rpc.params?.name)
      if (!tool) return { status: 200, body: rpcError(rpc.id, -32602, `tool "${rpc.params?.name}" is not available here`) }
      try {
        const out = await tool.handler(rpc.params?.arguments ?? {}, { app: slug, agent, store: storeFor(slug) })
        const text = typeof out === 'string' ? out : JSON.stringify(out ?? null, null, 2)
        return { status: 200, body: result(rpc.id, { content: [{ type: 'text', text }] }) }
      } catch (e) {
        return {
          status: 200,
          body: result(rpc.id, { content: [{ type: 'text', text: `error: ${(e as Error).message}` }], isError: true }),
        }
      }
    }
    default:
      return { status: 200, body: rpcError(rpc.id, -32601, `method "${rpc.method}" not supported`) }
  }
}
