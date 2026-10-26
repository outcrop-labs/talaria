// AN ORG MCP TOOL FROM AN APP HARNESS'S LOOP — `loop.tools[].org:
// '<server>/<tool>'` — reached the way the agent itself would reach it.
//
// An org tool belongs to whoever an admin granted it to, so a harness may call
// one only AS AN AGENT, under that agent's grants: its tool subset, its owner's
// allowance, the spend boundary, the call record. That is the MCP gateway's
// whole job (api/crates/talaria-routes-fleet/src/mcp/mcp_gw_server.rs), so the
// call goes THROUGH it — never to the server directly, and never with a
// credential the host holds for itself.
//
// WHICH AGENT. Only one run has an agent to be: one an agent started, by
// calling one of the app's own MCP tools. The gateway route that served that
// call (mcp.gw.$server.ts) proved the agent by its own key; the nested call
// presents the same key, so the gateway sees the same agent and applies the
// same rules it applied a moment ago. A request or a trigger has no agent,
// and an unproven caller (the org-wide legacy key) lends no identity — for
// those, the model is told the tool is unavailable and why.
import { MCP_PROTOCOL_VERSION } from './mcp-protocol'
import { parseMcpResponse } from './mcp-registry'
import { rustApiUrl } from './rust-proxy'
import type { ToolOutcome } from './harness/run'

export type OrgToolCall = (server: string, tool: string, args: Record<string, unknown>, signal: AbortSignal) => Promise<ToolOutcome>

interface RpcAnswer {
  error?: { message?: string }
  result?: { content?: Array<{ type: string; text?: string }>; structuredContent?: unknown; isError?: boolean }
}

/** The org-tool caller for a run an agent started, presenting that agent's own
 *  credential to the MCP gateway. Null when there is no api to reach. */
export function orgToolsAsAgent(credential: string, agentName: string, apiBase: () => string | undefined = rustApiUrl): OrgToolCall | null {
  if (apiBase() === undefined) return null
  return async (server, tool, args, signal) => {
    const fail = (text: string): ToolOutcome => ({ text, isError: true })
    // An app server is served by this host, not the api, and an app reaches
    // another app's tools only through that app.
    if (server.startsWith('app-')) return fail(`"${server}" is an app's MCP server; a harness reaches its own app's tools with mcp, and no other app's`)
    const url = `${apiBase()}/api/mcp/gw/${encodeURIComponent(server)}`
    const post = (body: unknown, session?: string) =>
      fetch(url, {
        method: 'POST',
        headers: {
          authorization: `Bearer ${credential}`,
          'x-agent-name': agentName,
          'content-type': 'application/json',
          accept: 'application/json, text/event-stream',
          ...(session ? { 'mcp-session-id': session } : {}),
        },
        body: JSON.stringify(body),
        redirect: 'manual',
        signal,
      })
    try {
      const init = await post({
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: { protocolVersion: MCP_PROTOCOL_VERSION, capabilities: {}, clientInfo: { name: 'talaria-app-harness', version: '1' } },
      })
      const initText = await init.text()
      if (!init.ok) return fail(`the MCP gateway refused "${server}" (${init.status}): ${initText.slice(0, 300)}`)
      const session = init.headers.get('mcp-session-id') ?? undefined
      if (session) await post({ jsonrpc: '2.0', method: 'notifications/initialized' }, session).then((r) => r.text()).catch(() => '')
      const res = await post({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: tool, arguments: args } }, session)
      const text = await res.text()
      const answer = parseMcpResponse(text) as RpcAnswer | null
      if (!answer) return fail(`the MCP gateway answered "${server}.${tool}" with ${res.status} and no result${text ? `: ${text.slice(0, 300)}` : ''}`)
      if (answer.error) return fail(answer.error.message ?? `"${server}.${tool}" failed`)
      const result = answer.result ?? {}
      const out = (result.content ?? []).flatMap((b) => (b.type === 'text' && typeof b.text === 'string' ? [b.text] : [])).join('\n')
      return {
        text: out || (result.structuredContent !== undefined ? JSON.stringify(result.structuredContent) : ''),
        ...(result.structuredContent !== undefined ? { structured: result.structuredContent } : {}),
        isError: result.isError === true,
      }
    } catch (e) {
      return fail(`"${server}.${tool}" could not be called: ${e instanceof Error ? e.message : String(e)}`)
    }
  }
}
