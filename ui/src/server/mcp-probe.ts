// Probe an MCP server for reachability + auth state. MCP's streamable-HTTP
// transport speaks JSON-RPC over POST; we send an `initialize` and classify the
// response so the UI can show a real connection status instead of hoping.
//
// The URL is admin-supplied, so the probe goes through safeFetch: http(s)
// only, no private/loopback/link-local targets, and every redirect re-checked.
//
// This used to carry a dev convenience: a bare docker hostname that failed to
// resolve (agent-mail:9500) was retried against localhost. That is exactly the
// SSRF primitive we are removing — "type a name that doesn't resolve, get a
// probe of the app's own loopback" — so it's gone. Operators who genuinely
// need to reach an internal MCP server put the host or CIDR in
// TALARIA_FETCH_ALLOW_HOSTS and enter the real URL.
import { safeFetch, BlockedUrlError } from './safe-fetch'

export type McpProbeState = 'ok' | 'auth' | 'unreachable' | 'error'

export interface McpProbeResult {
  state: McpProbeState
  detail: string
  /** Tool names, when the server volunteered them. */
  tools?: string[]
}

const INITIALIZE = {
  jsonrpc: '2.0',
  id: 1,
  method: 'initialize',
  params: {
    protocolVersion: '2025-06-18',
    capabilities: {},
    clientInfo: { name: 'talaria-probe', version: '0.1.0' },
  },
}

const post = (url: string, headers: Record<string, string>, body: unknown): Promise<Response> =>
  safeFetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', ...headers },
    body: JSON.stringify(body),
    timeoutMs: 8_000,
  })

export async function probeMcp(url: string, headers: Record<string, string> = {}): Promise<McpProbeResult> {
  let res: Response
  try {
    res = await post(url, headers, INITIALIZE)
  } catch (e) {
    // A refused target is an answer, not a failure — say which it was.
    if (e instanceof BlockedUrlError) return { state: 'error', detail: e.message }
    return { state: 'unreachable', detail: (e as Error).message.includes('timeout') ? 'timed out (8s)' : 'could not connect' }
  }

  if (res.status === 401 || res.status === 403) {
    return { state: 'auth', detail: `server requires authentication (${res.status})` }
  }
  if (!res.ok) return { state: 'error', detail: `server answered ${res.status}` }

  // Body may be JSON or an SSE frame ("data: {}"). Pull the first JSON object.
  const text = await res.text()
  const jsonStart = text.indexOf('{')
  if (jsonStart === -1) return { state: 'ok', detail: 'reachable' }
  try {
    const j = JSON.parse(text.slice(jsonStart, text.lastIndexOf('}') + 1)) as {
      result?: { serverInfo?: { name?: string } }
      error?: { message?: string }
    }
    if (j.error) return { state: 'error', detail: j.error.message ?? 'server returned an error' }
    const name = j.result?.serverInfo?.name
    return { state: 'ok', detail: name ? `connected to ${name}` : 'connected' }
  } catch {
    return { state: 'ok', detail: 'reachable' }
  }
}
