// JSON-RPC 2.0 over MCP — the wire shape and the method envelope every
// in-process MCP surface in this repo speaks. One shared dispatch: the
// server's name and what a tool call actually does arrive as callbacks.
//
// A leaf beside mcp-protocol.ts: the registry, the app dispatcher, and the
// workbench dispatcher all consume it, and none may import the others.

import { MCP_PROTOCOL_VERSION } from './mcp-protocol'

/** One JSON-RPC request message. `id` is `unknown` on purpose — JSON-RPC ids
 *  may be numbers, strings, or null, and the envelope echoes whatever the
 *  caller sent (or null when absent). */
export interface Rpc {
  jsonrpc?: string
  id?: unknown
  method?: string
  params?: { name?: string; arguments?: Record<string, unknown>; [k: string]: unknown }
}

export const result = (id: unknown, res: unknown) => ({ jsonrpc: '2.0', id: id ?? null, result: res })
export const rpcError = (id: unknown, code: number, message: string) => ({ jsonrpc: '2.0', id: id ?? null, error: { code, message } })

/** The method envelope for an MCP tools surface. `tools` arrives ALREADY
 *  filtered to what the caller may use (the gateway's resolution, enforced
 *  again by the in-process dispatcher); `listEntry` shapes each tool for
 *  tools/list; `call` runs one tool and returns the text the model reads,
 *  with `isError: true` for a tool that answered with a failure. A thrown
 *  error becomes isError content — the exception's message is tool output
 *  the model should see and adapt to, not a transport failure. */
export async function dispatchJsonRpc<T extends { name: string }>(
  rpc: Rpc,
  tools: T[],
  opts: {
    serverName: string
    listEntry: (tool: T) => unknown
    call: (tool: T, args: Record<string, unknown>, rpc: Rpc) => Promise<{ text: string; isError?: boolean }> | { text: string; isError?: boolean }
  },
): Promise<{ status: number; body: unknown | null }> {
  switch (rpc.method) {
    case 'initialize':
      return {
        status: 200,
        body: result(rpc.id, {
          protocolVersion: (rpc.params?.protocolVersion as string) ?? MCP_PROTOCOL_VERSION,
          capabilities: { tools: {} },
          serverInfo: { name: opts.serverName, version: '1.0' },
        }),
      }
    case 'notifications/initialized':
      // A notification carries no id and expects no reply — acknowledged,
      // not answered.
      return { status: 202, body: null }
    case 'ping':
      return { status: 200, body: result(rpc.id, {}) }
    case 'tools/list':
      return { status: 200, body: result(rpc.id, { tools: tools.map(opts.listEntry) }) }
    case 'tools/call': {
      const tool = tools.find((t) => t.name === rpc.params?.name)
      if (!tool) return { status: 200, body: rpcError(rpc.id, -32602, `tool "${rpc.params?.name}" is not available here`) }
      try {
        const out = await opts.call(tool, rpc.params?.arguments ?? {}, rpc)
        return {
          status: 200,
          body: out.isError
            ? result(rpc.id, { content: [{ type: 'text', text: out.text }], isError: true })
            : result(rpc.id, { content: [{ type: 'text', text: out.text }] }),
        }
      } catch (e) {
        return { status: 200, body: result(rpc.id, { content: [{ type: 'text', text: `error: ${(e as Error).message}` }], isError: true }) }
      }
    }
    default:
      return { status: 200, body: rpcError(rpc.id, -32601, `method "${rpc.method}" not supported`) }
  }
}
