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
import { subjectModel, subjectProven, type AgentSubject } from './agent-auth'
import { oauthTokenFor } from './mcp-oauth'
import { safeFetch } from './safe-fetch'
import { MCP_PROTOCOL_VERSION } from './mcp-protocol'
import type { HeaderDecl } from '@/lib/mcp-headers'

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
  requiredHeaders: HeaderDecl[]
  /** The server negotiates OAuth (discovered from its 401 challenge). */
  oauthEnabled: boolean
  /** Talaria's own toolkit — governable here, but not removable/reconfigurable. */
  builtin: boolean
  /** Set when a Talaria APP publishes this server (mcp.ts) — lifecycle follows
   *  the app (enable/disable), calls dispatch in-process, access rules here. */
  appSlug: string | null
  /** App servers only: why the app is not serving (turned off, failed its
   *  build, lost its mcp.ts). The row and its grants stay; no agent carries
   *  it. Written by the api's app sync alone. Null = serving. */
  appOff: string | null
  createdBy: string | null
  createdAt: string
}

const ROW = `id, name, label, description, url, headers, timeout_secs as "timeoutSecs", enabled,
  all_agents as "allAgents", auth_mode as "authMode", tools, tools_refreshed_at as "toolsRefreshedAt",
  required_headers as "requiredHeaders", (oauth is not null) as "oauthEnabled", builtin,
  app_slug as "appSlug", app_off as "appOff", created_by as "createdBy", created_at as "createdAt"`

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
  const server = await getMcpServer(serverName)
  if (!server) throw new Error(`MCP server "${serverName}" is not registered`)
  if (caller) {
    const { resolveHandles } = await import('./workspace-secrets')
    // The destination is the server the value is about to reach — an app
    // dispatches in process under its own scheme, a remote server is its host
    // — so an allowlist that names github.com cannot be spent into Notion
    // because the arguments mention GitHub.
    const destination = server.appSlug ? `talaria-app://${server.appSlug}` : safeHost(server.url)
    const resolved = await resolveHandles(JSON.stringify(args), caller, destination)
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
  if (!server.enabled) throw new Error(`MCP server "${serverName}" is disabled`)
  if (server.appOff) throw new Error(`MCP server "${serverName}" is not serving: ${server.appOff}`)
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

/** The host a registered URL reaches, or the URL itself when it does not parse
 *  (an unparseable destination is never allowlisted, so the spend refuses). */
function safeHost(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
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

/** Union of tool allowlists. `null` = all tools, so it dominates — the twin
 *  of `union_tools` in api/crates/talaria-mcp/src/registry.rs. */
const union = (a: string[] | null, b: string[] | null): string[] | null =>
  a === null || b === null ? null : [...new Set([...a, ...b])]
/** `union_tool_lists`: no lists at all is "no narrowing", i.e. all tools. */
const unionAll = (lists: Array<string[] | null>): string[] | null => (lists.length === 0 ? null : lists.reduce(union))

/** The Workbench's registry name (`WORKBENCH_SERVER` in registry.rs). */
const WORKBENCH_SERVER = 'workbench'

/** What one AGENT may do on one server: assignment ∪ team-agent grants,
 *  intersected (for a personal assistant) with its owner's user/team access —
 *  with the owner's credentials on per-user servers. Null = no access at all.
 *
 *  THIS IS A TWIN, and the drift it had was the finding: the Rust resolver
 *  (`effective_mcp_for`, api/crates/talaria-mcp/src/registry.rs) reads
 *  `mcp_team_access` joined to `team_agents` for the agent and to
 *  `team_members` for the owner; this copy read neither, so a team granted
 *  an app-published server got 403 from /api/mcp/gw/app-* while the same
 *  grant on a remote server worked. The rules below are the Rust ones, in
 *  order, with the same union/intersect semantics (null = all, [] = nothing):
 *    · agent side — an assignment row ∪ every ALLOWED team grant for a team
 *      the agent is on; on a scoped server a team grant counts as assignment;
 *      all-agents servers carry everyone and rows only narrow.
 *    · owner side — a person's own deny wins outright; a team deny with no
 *      allow anywhere (person or team) denies; otherwise the person's allow
 *      ∪ the allowed team grants is the owner's allowance.
 *  Rust also answers "granted but not connected" as its own verdict; this
 *  resident serves only app servers (no upstream account to connect), so
 *  null stands for every refusal here.
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
  // The Workbench is granted by the agent's Developer Agent switch and by
  // nothing else; the Rust resolver reads that switch. This resident never
  // serves it (the proxy holds back only app-*), so it is a refusal here
  // rather than a second reading of agent_defs.developer.
  if (server.name === WORKBENCH_SERVER) return null

  let agentTools: string[] | null = null
  {
    const rows = (await sql`
      select tools from mcp_server_agents where server_id = ${server.id} and agent_model = ${agentModel}
    `) as unknown as Array<{ tools: string[] | null }>
    const teamRows = (await sql`
      select a.allowed, a.tools
      from mcp_team_access a
      join team_agents t on t.team_id = a.team_id
      where a.server_id = ${server.id} and t.agent_model = ${agentModel}
    `) as unknown as Array<{ allowed: boolean; tools: string[] | null }>
    const teamAllowLists = teamRows.filter((r) => r.allowed).map((r) => r.tools ?? null)
    // All-agents servers carry everyone; assignment rows become per-agent
    // tool OVERRIDES. Scoped servers require a row outright, or an allowed
    // team grant for a team the agent is on.
    if (rows.length === 0) {
      if (server.allAgents) agentTools = null
      else if (teamAllowLists.length === 0) return null
      else agentTools = unionAll(teamAllowLists)
    } else {
      agentTools = unionAll([rows[0]?.tools ?? null, ...teamAllowLists])
    }
  }

  const owner = await assistantOwnerFor(agent)
  let userTools: string[] | null = null
  let upstreamHeaders = server.headers
  if (owner) {
    const [access] = (await sql`
      select allowed, tools from mcp_user_access where server_id = ${server.id} and user_id = ${owner}
    `) as unknown as Array<{ allowed: boolean; tools: string[] | null }>
    if (access && !access.allowed) return null
    const teamRows = (await sql`
      select a.allowed, a.tools
      from mcp_team_access a
      join team_members m on m.team_id = a.team_id
      where a.server_id = ${server.id} and m.user_id = ${owner}
    `) as unknown as Array<{ allowed: boolean; tools: string[] | null }>
    const anyTeamDeny = teamRows.some((r) => !r.allowed)
    const anyTeamAllow = teamRows.some((r) => r.allowed)
    const userAllow = access?.allowed === true
    if (anyTeamDeny && !anyTeamAllow && !userAllow) return null
    const allowedLists: Array<string[] | null> = []
    if (userAllow) allowedLists.push(access?.tools ?? null)
    for (const r of teamRows) if (r.allowed) allowedLists.push(r.tools ?? null)
    userTools = unionAll(allowedLists)
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

// ── The call record ─────────────────────────────────────────────────────────
// One row in `mcp_tool_calls` per tools/call this resident answers — the same
// table, columns and rules as the Rust gateway's `CallLog`
// (api/crates/talaria-mcp/src/calls.rs), because the Observability panel
// reads that table and "which agent called which app tool" had no answer at
// all: app calls were the one gateway path that wrote nothing. What a row
// holds and never does is calls.rs's contract, restated here so the twin
// cannot quietly say less: who (agent, and the acting human on a personal
// assistant), where (server id + name), what (the tool NAME, capped), how it
// went (outcome, status, latency) and — on a failure — one sentence capped at
// 300 chars. NEVER the arguments, never the result: arguments carry the
// credential the spend boundary just put there, results carry whatever the
// tool read. The sentence is scrubbed of the argument strings the agent sent
// (`scrubArguments` in mcp-jsonrpc.ts) and, on a call that spent a
// credential, replaced outright (`SPENT_REDACTED`).
//
// Fire-and-forget: the hot path never waits on the insert, and a failed
// insert warns and is gone — losing a log row is the right trade against
// failing a tool call.

/** The closed set `mcp_tool_calls.outcome` takes (`Outcome::as_str` in calls.rs). */
export type McpCallOutcome = 'ok' | 'error' | 'denied' | 'not_connected' | 'upstream_error' | 'spend_error' | 'cancelled'

export interface McpCallRecord {
  /** Known when the server resolved; null on a refusal before resolution,
   *  and the insert then looks it up by name (and records nothing for a
   *  name that is not a server, like the Rust twin). */
  serverId: string | null
  serverName: string
  agent: AgentSubject
  tool: string | null
  outcome: McpCallOutcome
  /** The HTTP status this resident answered with. */
  status: number | null
  latencyMs: number | null
  /** Already scrubbed and capped by the caller (see mcp-jsonrpc.ts). */
  error: string | null
}

/** The longest error sentence / tool name a row keeps (calls.rs `ERROR_MAX_CHARS`, `TOOL_MAX_CHARS`). */
export const CALL_ERROR_MAX_CHARS = 300
export const CALL_TOOL_MAX_CHARS = 200

export function recordMcpToolCall(row: McpCallRecord): void {
  void (async () => {
    const sql = await db()
    // The Rust statement, verbatim in shape: the owner lookup rides inside as
    // a subquery, and `owner_eligible` (a proven caller, never a legacy
    // shared-key one) is what lets it run — a legacy caller's name proves
    // nothing, so no human is attributed to it.
    await sql.unsafe(
      `insert into mcp_tool_calls
         (server_id, server_name, agent, user_id, method, tool, outcome, status, latency_ms, error)
       select coalesce($1::text, (select s.id::text from mcp_servers s where s.name = $2::text)),
              $2::text, $3::text,
              case when $4::bool then (select d.owner_user_id::text from agent_defs d
                                        where d.model = $3::text and d.owner_user_id is not null
                                        limit 1) end,
              'tools/call', $5::text, $6::text, $7::smallint, $8::int4, $9::text
       where coalesce($1::text, (select s.id::text from mcp_servers s where s.name = $2::text)) is not null`,
      [
        row.serverId,
        row.serverName,
        subjectModel(row.agent),
        subjectProven(row.agent),
        row.tool === null ? null : [...row.tool].slice(0, CALL_TOOL_MAX_CHARS).join(''),
        row.outcome,
        row.status,
        row.latencyMs,
        row.error === null ? null : [...row.error].slice(0, CALL_ERROR_MAX_CHARS).join(''),
      ],
    )
  })().catch((e) => console.warn(`[mcp/calls] could not record ${row.serverName}.${row.tool ?? '?'}: ${(e as Error).message}`))
}
