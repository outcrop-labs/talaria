// The org-wide MCP registry — Composio's shape, self-hosted and tailored:
//   servers      registered once for the whole org (url + org headers)
//   assignments  which agents carry a server, optionally a TOOL SUBSET
//   user access  which users may exercise a server through agents acting for
//                them, optionally their own tool subset
//   credentials  per-user connected accounts: on auth_mode 'per-user' the
//                gateway injects the ACTING user's sealed headers, so the same
//                server acts as each user, never as a shared identity
// Enforcement is the /api/mcp/gw gateway, not config: agents only ever see
// gateway URLs, and the gateway filters tools/list + rejects tools/call
// outside the allowed set. Config can't be jailbroken past the registry.
//
// This is the slice the TS tier still runs: the reads the harness needs
// (capability reach, web search) and the effective-access resolution the
// app-MCP resident enforces. The admin write half, the app-published-server
// sync, and the builtin-row seeding are the Rust twin's
// (api/src/mcp/registry.rs), which guarantees those rows from boot.
//
// IDENTITY: "the acting user" here is a HUMAN, and on a per-user server this
// module hands out that human's connected credentials (an OAuth bearer token,
// verbatim, in upstreamHeaders). That is owner-proxying, so it resolves
// through `assistantOwnerFor(subject)` — which refuses a legacy shared-key
// caller — and never through `personalAssistantOwners().get(model)`, whose
// bare-string key `subjectProven` reads as PROVEN. Pass the AgentCaller
// wherever one is in hand.
import { db } from './db/pg'
import { open } from './secretbox'
import { assistantOwnerFor } from './users'
import { subjectModel, type AgentSubject } from './agent-auth'
import { oauthTokenFor } from './mcp-oauth'
import { safeFetch } from './safe-fetch'
import { MCP_PROTOCOL_VERSION } from './mcp-protocol'

export interface McpServer {
  id: string
  name: string
  label: string
  description: string | null
  url: string
  headers: Record<string, string>
  timeoutSecs: number | null
  enabled: boolean
  allAgents: boolean
  /** 'org' = shared org headers; 'per-user' = each user connects their own. */
  authMode: 'org' | 'per-user'
  tools: Array<{ name: string; description?: string }>
  toolsRefreshedAt: string | null
  /** Header declarations captured at install (drive per-user connect forms). */
  requiredHeaders: Array<{ name: string; description: string | null; isSecret: boolean; placeholder: string | null }>
  /** The server negotiates OAuth (discovered from its 401 challenge). */
  oauthEnabled: boolean
  /** Talaria's own toolkit — governable here, but not removable/reconfigurable. */
  builtin: boolean
  /** Set when a Talaria APP publishes this server (mcp.ts) — lifecycle follows
   *  the app (enable/disable), calls dispatch in-process, access rules here. */
  appSlug: string | null
  createdBy: string | null
  createdAt: string
}

const ROW = `id, name, label, description, url, headers, timeout_secs as "timeoutSecs", enabled,
  all_agents as "allAgents", auth_mode as "authMode", tools, tools_refreshed_at as "toolsRefreshedAt",
  required_headers as "requiredHeaders", (oauth is not null) as "oauthEnabled", builtin,
  app_slug as "appSlug", created_by as "createdBy", created_at as "createdAt"`

// Pure read: the builtin rows (the toolkit, the Workbench) are the api's to
// guarantee — it seeds them at boot and re-ensures on its own list.
export async function listMcpServers(): Promise<McpServer[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from mcp_servers order by builtin desc, name`)) as unknown as McpServer[]
}

export async function getMcpServer(idOrName: string): Promise<McpServer | null> {
  const sql = await db()
  const rows = (await sql.unsafe(
    `select ${ROW} from mcp_servers where id::text = $1 or name = $1`,
    [idOrName],
  )) as unknown as McpServer[]
  return rows[0] ?? null
}

// ── Discovery ───────────────────────────────────────────────────────────────

/** ONE JSON-RPC CONVERSATION WITH A SERVER, AS THE ORG.
 *
 *  The caller is PLATFORM-INITIATED — Talaria asking a server something on
 *  the org's behalf, not an agent acting for a person — which is why the org
 *  bearer is right here and why nothing in this helper takes a user.
 *
 *  A per-user server has no org identity to act as, so a platform caller must
 *  not silently fall back to shared credentials: `callMcpTool` refuses those
 *  outright rather than acting as nobody in particular. */
async function orgSession(server: McpServer): Promise<{ call: (body: unknown, sessionId?: string | null) => Promise<{ json: unknown; session: string | null; status: number }> }> {
  // OAuth servers authenticate with the org connection when one exists; the
  // builtin toolkit authenticates with the fleet key as Talaria itself.
  const bearer = server.oauthEnabled ? await oauthTokenFor(server.id, 'org') : null
  const builtinHeaders: Record<string, string> = server.builtin ? { 'X-Agent-Name': 'talaria', 'X-Api-Key': process.env.TALARIA_AGENT_KEY ?? '' } : {}
  const call = async (body: unknown, sessionId?: string | null) => {
    const init = {
      method: 'POST' as const,
      headers: {
        'content-type': 'application/json',
        accept: 'application/json, text/event-stream',
        ...server.headers,
        ...builtinHeaders,
        ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
        ...(sessionId ? { 'mcp-session-id': sessionId } : {}),
      },
      body: JSON.stringify(body),
    }
    const timeoutMs = (server.timeoutSecs ?? 30) * 1000
    // Everything but the built-in toolkit is a URL (and a header set) someone
    // with agents.manage typed — straight through the SSRF guard. The builtin
    // row is Talaria's own MCP service on loopback, written by the api and
    // un-editable by design (its update path refuses url/headers patches on
    // builtin), so it is infrastructure, not input.
    const r = server.builtin
      ? await fetch(server.url, { ...init, signal: AbortSignal.timeout(timeoutMs) })
      : await safeFetch(server.url, { ...init, timeoutMs })
    const session = r.headers.get('mcp-session-id')
    const text = await r.text()
    return { json: parseMcpResponse(text), session, status: r.status }
  }
  return { call }
}

/** One tool result, flattened to the two things a caller needs: the text the
 *  model should see, and whatever structured payload the server also returned. */
export interface McpToolResult {
  /** Every text content block, joined. Empty when the server returned none. */
  text: string
  /** `structuredContent`, verbatim, when the server sent it. */
  structured: unknown
  isError: boolean
}

/** CALL ONE TOOL ON A REGISTERED SERVER, AS THE ORG.
 *
 *  This is the platform's own door to MCP, and it is deliberately NOT the
 *  agent-facing one. Agents reach tools through `/api/mcp/gw/<server>`, which
 *  resolves the acting agent, intersects its assignment with the owner's
 *  allowance, and injects per-user credentials — none of which applies to a
 *  platform stage like the research search step, where there is no agent and no
 *  acting human, only Talaria doing a job the org configured it to do.
 *
 *  REFUSES PER-USER SERVERS, and that refusal is the security-relevant line in
 *  this function. On `auth_mode: 'per-user'` the credentials belong to a HUMAN;
 *  a platform caller has no such human, and quietly using the org headers
 *  instead would act as a shared identity on a server explicitly configured to
 *  never be one. Better to fail and say so. */
export async function callMcpTool(serverName: string, tool: string, args: Record<string, unknown>, caller?: string): Promise<McpToolResult> {
  // ── THE BOUNDARY THAT SPENDS A CREDENTIAL ──────────────────────────────────
  //
  // A model can hold `«secret:deploy.github_pat»` and pass it wherever the value
  // would go; this is where the value actually appears, on its way OUT to a tool
  // that needs it. Nothing that comes back is resolved — a tool RESULT re-enters
  // the model's context, and putting a credential there would undo the whole
  // arrangement in one line.
  //
  // A caller with no grant resolves nothing. The reason is reported to the
  // OPERATOR rather than to the model: a caller that learns which names exist
  // has been handed a map of the workspace's credentials.
  if (caller) {
    const { resolveHandles } = await import('./workspace-secrets')
    const resolved = await resolveHandles(JSON.stringify(args), caller)
    if (resolved.used.length > 0 || resolved.unresolved.length > 0) {
      try {
        args = JSON.parse(resolved.text) as Record<string, unknown>
      } catch {
        /* a credential containing a quote broke the round trip — send the args
           unresolved rather than a malformed body, and let the tool refuse. */
      }
      for (const u of resolved.used) console.warn(`[secrets] ${caller} spent ${u.name}.${u.key} (${u.label}) on ${serverName}.${tool}`)
      for (const u of resolved.unresolved) console.warn(`[secrets] ${caller} could not resolve ${u.handle} on ${serverName}.${tool}: ${u.reason}`)
    }
  }
  const server = await getMcpServer(serverName)
  if (!server) throw new Error(`MCP server "${serverName}" is not registered`)
  if (!server.enabled) throw new Error(`MCP server "${serverName}" is disabled`)
  if (server.authMode === 'per-user') {
    throw new Error(`MCP server "${serverName}" authenticates per user, so a platform stage cannot call it. Register an org-authenticated server for this.`)
  }
  if (server.appSlug) {
    // App servers dispatch IN PROCESS — no socket, no headers. `allowed: null`
    // is the org's own call rather than an agent's, and the same "as Talaria"
    // identity the builtin toolkit gets.
    const { dispatchAppMcp } = await import('./app-mcp')
    const out = await dispatchAppMcp(server.appSlug, { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: tool, arguments: args } }, 'talaria', null)
    return readToolResult(out.body, serverName, tool)
  }

  const { call } = await orgSession(server)
  const init = await call({
    jsonrpc: '2.0',
    id: 1,
    method: 'initialize',
    params: { protocolVersion: MCP_PROTOCOL_VERSION, capabilities: {}, clientInfo: { name: 'talaria', version: '1.0' } },
  })
  if (init.status >= 400) throw new Error(`MCP server "${serverName}" answered ${init.status}`)
  const res = await call({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: tool, arguments: args } }, init.session)
  if (res.status >= 400) throw new Error(`MCP tool "${serverName}.${tool}" answered ${res.status}`)

  return readToolResult(res.json, serverName, tool)
}

/** The MCP `tools/call` result shape, flattened. One reader for both the socket
 *  path and the in-process app path, so the two cannot disagree about what a
 *  tool returned. */
function readToolResult(raw: unknown, serverName: string, tool: string): McpToolResult {
  const body = raw as { result?: { content?: Array<{ type?: string; text?: string }>; structuredContent?: unknown; isError?: boolean }; error?: { message?: string } }
  if (body?.error) throw new Error(`MCP tool "${serverName}.${tool}" failed: ${body.error.message ?? 'unknown error'}`)
  const content = body?.result?.content ?? []
  return {
    text: content
      .filter((c) => c.type === 'text' && typeof c.text === 'string')
      .map((c) => c.text!)
      .join('\n')
      .trim(),
    structured: body?.result?.structuredContent ?? null,
    isError: body?.result?.isError === true,
  }
}

/** JSON body, or the LAST data frame of an SSE-encoded response. */
function parseMcpResponse(text: string): unknown {
  const t = text.trim()
  if (!t) return null
  if (t.startsWith('{') || t.startsWith('[')) {
    try {
      return JSON.parse(t)
    } catch {
      return null
    }
  }
  const frames = t
    .split('\n')
    .filter((l) => l.startsWith('data:'))
    .map((l) => l.slice(5).trim())
  for (const f of frames.reverse()) {
    try {
      return JSON.parse(f)
    } catch {
      /* keep looking */
    }
  }
  return null
}

// ── Per-user connected accounts ─────────────────────────────────────────────

export async function getUserCredentials(serverId: string, userId: string): Promise<Record<string, string> | null> {
  const sql = await db()
  const rows = (await sql`
    select headers_enc as enc from mcp_user_credentials where server_id = ${serverId} and user_id = ${userId}
  `) as unknown as Array<{ enc: string }>
  if (!rows[0]) return null
  try {
    return JSON.parse(open(rows[0].enc)) as Record<string, string>
  } catch {
    return null
  }
}

// ── Effective resolution (the gateway's brain) ──────────────────────────────

export interface EffectiveMcp {
  server: McpServer
  /** null = all tools; otherwise the enforced allowlist. */
  tools: string[] | null
  /** Headers to speak upstream with (org's, or the acting user's sealed set). */
  upstreamHeaders: Record<string, string>
}

const intersect = (a: string[] | null, b: string[] | null): string[] | null => {
  if (a === null) return b
  if (b === null) return a
  const bs = new Set(b)
  return a.filter((t) => bs.has(t))
}

/** What one AGENT may do on one server: assignment ∩ (for a personal
 *  assistant) its owner's user access — with the owner's credentials on
 *  per-user servers. Null = no access at all.
 *
 *  Takes the CALLER wherever the gateway has one. Resolving the acting owner
 *  hands out that human's connected account — on an OAuth server, literally
 *  their bearer token in `upstreamHeaders` — so it goes through
 *  `assistantOwnerFor`, which consults `legacy`, never
 *  `personalAssistantOwners().get(model)`, whose bare-string key reads as
 *  proven and would let an ASSERTED identity act as the human. */
export async function effectiveMcpFor(agent: AgentSubject, serverName: string): Promise<EffectiveMcp | null> {
  const agentModel = subjectModel(agent)
  const server = await getMcpServer(serverName)
  if (!server || !server.enabled) return null
  const sql = await db()

  let agentTools: string[] | null = null
  {
    const rows = (await sql`
      select tools from mcp_server_agents where server_id = ${server.id} and agent_model = ${agentModel}
    `) as unknown as Array<{ tools: string[] | null }>
    // All-agents servers carry everyone; assignment rows become per-agent
    // tool OVERRIDES. Scoped servers require a row outright.
    if (rows.length === 0 && !server.allAgents) return null
    agentTools = rows[0]?.tools ?? null
  }

  const owner = await assistantOwnerFor(agent)
  let userTools: string[] | null = null
  let upstreamHeaders = server.headers
  if (owner) {
    const [access] = (await sql`
      select allowed, tools from mcp_user_access where server_id = ${server.id} and user_id = ${owner}
    `) as unknown as Array<{ allowed: boolean; tools: string[] | null }>
    if (access && !access.allowed) return null
    userTools = access?.tools ?? null
    if (server.authMode === 'per-user') {
      if (server.oauthEnabled) {
        const bearer = await oauthTokenFor(server.id, owner)
        if (!bearer) return null // not connected — the server doesn't exist for this assistant yet
        upstreamHeaders = { ...server.headers, authorization: `Bearer ${bearer}` }
      } else {
        const creds = await getUserCredentials(server.id, owner)
        if (!creds) return null
        upstreamHeaders = { ...server.headers, ...creds }
      }
    }
  } else if (server.authMode === 'per-user') {
    // Org agents have no single acting user to be — per-user servers are
    // personal-assistant territory by definition.
    return null
  }
  // Org-auth OAuth servers speak with the shared org connection.
  if (server.authMode === 'org' && server.oauthEnabled) {
    const bearer = await oauthTokenFor(server.id, 'org')
    if (!bearer) return null // nobody connected the org account yet
    upstreamHeaders = { ...upstreamHeaders, authorization: `Bearer ${bearer}` }
  }

  return { server, tools: intersect(agentTools, userTools), upstreamHeaders }
}
