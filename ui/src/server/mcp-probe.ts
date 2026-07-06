// Probe an MCP server for reachability + auth state. MCP's streamable-HTTP
// transport speaks JSON-RPC over POST; we send an `initialize` and classify the
// response so the UI can show a real connection status instead of hoping.
//
// Dev note: fleet MCP servers live on the docker network (agent-mail:9500…),
// which the host can't resolve — the docker-hostname→localhost fallback lets
// the probe work in dev when the port is published; otherwise it honestly
// reports unreachable.
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

async function post(url: string, headers: Record<string, string>, body: unknown): Promise<Response> {
  const init = {
    method: 'POST' as const,
    headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', ...headers },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(8_000),
  }
  try {
    return await fetch(url, init)
  } catch (err) {
    const m = /^(https?):\/\/([^/:]+)(:\d+)?(\/.*)?$/.exec(url)
    const host = m?.[2] ?? ''
    if (m && host && !host.includes('.') && host !== 'localhost') {
      return await fetch(`${m[1]}://localhost${m[3] ?? ''}${m[4] ?? ''}`, init)
    }
    throw err
  }
}

export async function probeMcp(url: string, headers: Record<string, string> = {}): Promise<McpProbeResult> {
  let res: Response
  try {
    res = await post(url, headers, INITIALIZE)
  } catch (e) {
    return { state: 'unreachable', detail: (e as Error).message.includes('timeout') ? 'timed out (8s)' : 'could not connect' }
  }

  if (res.status === 401 || res.status === 403) {
    return { state: 'auth', detail: `server requires authentication (${res.status})` }
  }
  if (!res.ok) return { state: 'error', detail: `server answered ${res.status}` }

  // Body may be JSON or an SSE frame ("data: {…}"). Pull the first JSON object.
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
