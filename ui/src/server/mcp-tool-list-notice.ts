// THE TOOL-LIST NOTICE, for the servers an app publishes — the twin of the
// Rust gateway's (api/crates/talaria-routes-fleet/src/mcp/mcp_gw_server.rs,
// "The tool-list notice"), which every other MCP server goes through.
//
// A running Hermes lists a server's tools when it connects, and again only
// when the server sends `notifications/tools/list_changed`. An app's catalog
// can change under the server name its carriers keep (an app update), and so
// can a carrier's tool subset; each server counts its own such changes, in
// the `mcp.tool_list_count:<server>` setting (talaria-mcp-apply
// `bump_tool_lists`), so a change to one app re-lists that app's server
// alone. Per agent and server the gateway remembers the count it last saw; a
// handshake
// starts that count when none is remembered, and a keepalive `ping` from an
// agent behind it is answered here with the notice ahead of the ping's result —
// an SSE answer, which the Hermes client reads notifications from — and the
// agent lists the tools again. Same keys and rules as the Rust side, less one:
// the Rust relay also puts the notice on a `tools/list` from an agent behind
// the count, because a third-party upstream that does not answer ping makes
// Hermes keep alive with `list_tools` instead. This gateway answers ping
// itself (mcp-jsonrpc.ts), so an app server never puts Hermes there.

import { getSetting } from '@/server/audit'
import { getRedis } from '@/server/db/redis'
import type { Rpc } from '@/server/mcp-jsonrpc'

/** One server's count of tool-list changes — talaria-mcp-apply's `tool_list_count_key`. */
export function toolListCountKey(server: string): string {
  return `mcp.tool_list_count:${server}`
}
/** Every keepalive refreshes it, so it lapses only for an agent that stopped asking. */
const TOOLS_SEEN_TTL_SECONDS = 24 * 60 * 60

/** v2: the remembered count is the server's own (v1 held one count for every server). */
export function toolsSeenKey(server: string, caller: string): string {
  return `talaria:mcp-tools-seen:v2:${server}:${caller}`
}

/** The staleness rule: nothing remembered is adopted as current, never announced. */
export function toolListIsStale(seen: number | null, epoch: number): boolean {
  return seen !== null && seen < epoch
}

export function isPing(rpc: Rpc): boolean {
  return rpc.method === 'ping' && rpc.id !== undefined
}

export function acceptsEventStream(request: Request): boolean {
  return (request.headers.get('accept') ?? '').includes('text/event-stream')
}

/** The keepalive answer that carries the notice: the notification first, then
 *  the ping's own result, which ends the client's read of the stream. */
export function listChangedAnswer(id: unknown): Response {
  const notice = { jsonrpc: '2.0', method: 'notifications/tools/list_changed' }
  const result = { jsonrpc: '2.0', id: id ?? null, result: {} }
  return new Response(`event: message\ndata: ${JSON.stringify(notice)}\n\nevent: message\ndata: ${JSON.stringify(result)}\n\n`, {
    status: 200,
    headers: { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' },
  })
}

async function swapToolsSeen(server: string, caller: string, epoch: number): Promise<number | null | undefined> {
  try {
    const redis = getRedis()
    const key = toolsSeenKey(server, caller)
    const raw = await redis.get(key)
    await redis.set(key, String(epoch), 'EX', TOOLS_SEEN_TTL_SECONDS)
    const seen = raw === null ? null : Number(raw)
    return seen === null || Number.isFinite(seen) ? seen : null
  } catch {
    return undefined // Redis out of reach: no notice; the request goes on as always.
  }
}

/** Start the count for an agent none is remembered for — only then: Hermes
 *  re-lists on a reconnect but re-publishes only when it has nothing
 *  registered, so a reconnect between a change and the next keepalive must
 *  still be told. */
async function startToolsSeen(server: string, caller: string, epoch: number): Promise<void> {
  try {
    await getRedis().set(toolsSeenKey(server, caller), String(epoch), 'EX', TOOLS_SEEN_TTL_SECONDS, 'NX')
  } catch {
    // Redis out of reach: the first keepalive adopts the count instead.
  }
}

/** For a request the gateway already let in: a handshake starts the agent's
 *  count when it has none; a keepalive behind it gets `listChangedAnswer`. */
export async function toolListNotice(server: string, caller: string, rpc: Rpc, request: Request): Promise<Response | null> {
  const initialize = rpc.method === 'initialize'
  if (!initialize && !(isPing(rpc) && acceptsEventStream(request))) return null
  let epoch: number
  try {
    epoch = Number(await getSetting<number>(toolListCountKey(server), 0)) || 0
  } catch {
    return null
  }
  if (initialize) {
    await startToolsSeen(server, caller, epoch)
    return null
  }
  const seen = await swapToolsSeen(server, caller, epoch)
  if (seen === undefined || !toolListIsStale(seen, epoch)) return null
  console.info(`[mcp/gw:${server}] ${caller}: its tool list may have changed — the keepalive answer carries tools/list_changed`)
  return listChangedAnswer(rpc.id)
}
