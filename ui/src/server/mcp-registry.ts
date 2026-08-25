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
// IDENTITY: "the acting user" here is a HUMAN, and on a per-user server this
// module hands out that human's connected credentials (an OAuth bearer token,
// verbatim, in upstreamHeaders). That is owner-proxying, so it resolves
// through `assistantOwnerFor(subject)` — which refuses a legacy shared-key
// caller — and never through `personalAssistantOwners().get(model)`, whose
// bare-string key `subjectProven` reads as PROVEN. Pass the AgentCaller
// wherever one is in hand; a bare string is for the listing shapes that have
// no caller (fleet-render, the admin /api/mcp roster).
import { db } from './db/pg'
import { seal, open } from './secretbox'
import { assistantOwnerFor } from './users'
import { subjectModel, type AgentSubject } from './agent-auth'
import { hasOauthTokens, oauthTokenFor } from './mcp-oauth'
import { safeFetch } from './safe-fetch'

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

export interface McpAssignment {
  agentModel: string
  /** null = every tool the server offers. */
  tools: string[] | null
}

export interface McpUserAccess {
  userId: string
  allowed: boolean
  tools: string[] | null
}

const ROW = `id, name, label, description, url, headers, timeout_secs as "timeoutSecs", enabled,
  all_agents as "allAgents", auth_mode as "authMode", tools, tools_refreshed_at as "toolsRefreshedAt",
  required_headers as "requiredHeaders", (oauth is not null) as "oauthEnabled", builtin,
  app_slug as "appSlug", created_by as "createdBy", created_at as "createdAt"`

/** The Talaria toolkit as a governable system row: every agent carries it,
 *  and the same per-agent/per-person tool subsets apply — enforced by the
 *  gateway like any other server. Identity/lifecycle stay locked. */
export async function ensureBuiltinMcp(): Promise<void> {
  const sql = await db()
  const { MCP_PORT } = await import('./mcp-service')
  await sql`
    insert into mcp_servers (name, label, description, url, all_agents, builtin, created_by)
    values ('talaria', 'Talaria toolkit', 'Talaria''s own tools: tickets, documents, knowledge, channels, research, media.',
            ${`http://127.0.0.1:${MCP_PORT()}/mcp`}, true, true, 'talaria')
    on conflict (name) do update set builtin = true, all_agents = true, enabled = true
  `
  // The Workbench surface — in-process like app servers, but NOT all_agents:
  // access is an explicit per-agent grant like any other governed capability.
  const { WORKBENCH_TOOLS } = await import('./workbench-mcp')
  const tools = WORKBENCH_TOOLS.map((t) => ({ name: t.name, description: t.description.slice(0, 300) }))
  await sql`
    insert into mcp_servers (name, label, description, url, all_agents, created_by, tools, tools_refreshed_at)
    values ('workbench', 'Workbench', 'Sandboxed execution for granted agents: jobs, branches, and PRs under the platform-owned git flow.',
            'talaria-workbench://core', false, 'talaria', ${sql.json(tools)}, now())
    on conflict (name) do update set tools = ${sql.json(tools)}, tools_refreshed_at = now(), enabled = true
  `
}

export async function listMcpServers(): Promise<McpServer[]> {
  const sql = await db()
  await ensureBuiltinMcp().catch(() => {})
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

export async function createMcpServer(input: {
  name: string
  label?: string
  description?: string | null
  url: string
  headers?: Record<string, string>
  timeoutSecs?: number | null
  authMode?: 'org' | 'per-user'
  requiredHeaders?: Array<{ name: string; description?: string | null; isSecret?: boolean; placeholder?: string | null }>
  createdBy: string
}): Promise<McpServer> {
  const sql = await db()
  const declared = (input.requiredHeaders ?? []).map((h) => ({
    name: h.name,
    description: h.description ?? null,
    isSecret: h.isSecret ?? false,
    placeholder: h.placeholder ?? null,
  }))
  const rows = (await sql`
    insert into mcp_servers (name, label, description, url, headers, timeout_secs, auth_mode, required_headers, created_by)
    values (${input.name}, ${input.label ?? input.name}, ${input.description ?? null}, ${input.url},
            ${sql.json(input.headers ?? {})}, ${input.timeoutSecs ?? null}, ${input.authMode ?? 'org'},
            ${sql.json(declared)}, ${input.createdBy})
    returning ${sql.unsafe(ROW)}
  `) as unknown as McpServer[]
  return rows[0]!
}

export async function updateMcpServer(
  id: string,
  patch: Partial<{
    label: string
    description: string | null
    url: string
    headers: Record<string, string>
    timeoutSecs: number | null
    enabled: boolean
    allAgents: boolean
    authMode: 'org' | 'per-user'
  }>,
): Promise<void> {
  const sql = await db()
  const [row] = (await sql`select builtin, app_slug as "appSlug" from mcp_servers where id = ${id}`) as unknown as Array<{
    builtin: boolean
    appSlug: string | null
  }>
  if (row?.builtin) {
    // The toolkit's identity and lifecycle are Talaria's; only access rules
    // (assignments/user access, handled elsewhere) are governable.
    for (const k of ['url', 'headers', 'enabled', 'allAgents', 'authMode'] as const) {
      if (patch[k] !== undefined) throw new Error('the built-in Talaria toolkit cannot be reconfigured')
    }
  }
  if (row?.appSlug) {
    // App servers have no upstream to point elsewhere and follow the app's
    // lifecycle; allAgents/access stay governable like any server.
    for (const k of ['url', 'headers', 'enabled', 'authMode'] as const) {
      if (patch[k] !== undefined) throw new Error('this server is published by an app; disable the app instead')
    }
  }
  if (patch.label !== undefined) await sql`update mcp_servers set label = ${patch.label}, updated_at = now() where id = ${id}`
  if (patch.description !== undefined) await sql`update mcp_servers set description = ${patch.description}, updated_at = now() where id = ${id}`
  if (patch.url !== undefined) await sql`update mcp_servers set url = ${patch.url}, tools = '[]', tools_refreshed_at = null, updated_at = now() where id = ${id}`
  if (patch.headers !== undefined) await sql`update mcp_servers set headers = ${sql.json(patch.headers)}, updated_at = now() where id = ${id}`
  if (patch.timeoutSecs !== undefined) await sql`update mcp_servers set timeout_secs = ${patch.timeoutSecs}, updated_at = now() where id = ${id}`
  if (patch.enabled !== undefined) await sql`update mcp_servers set enabled = ${patch.enabled}, updated_at = now() where id = ${id}`
  if (patch.allAgents !== undefined) await sql`update mcp_servers set all_agents = ${patch.allAgents}, updated_at = now() where id = ${id}`
  if (patch.authMode !== undefined) await sql`update mcp_servers set auth_mode = ${patch.authMode}, updated_at = now() where id = ${id}`
}

export async function deleteMcpServer(id: string): Promise<void> {
  const sql = await db()
  const [row] = (await sql`select builtin, app_slug as "appSlug" from mcp_servers where id = ${id}`) as unknown as Array<{
    builtin: boolean
    appSlug: string | null
  }>
  if (row?.builtin) throw new Error('the built-in Talaria toolkit cannot be removed')
  if (row?.appSlug) throw new Error('this server is published by an app; disable or uninstall the app instead')
  await sql`delete from mcp_servers where id = ${id}` // assignments/access/credentials cascade
}

// ── App-published servers ───────────────────────────────────────────────────

/** Reconcile registry rows with the ENABLED apps that publish MCP tools:
 *  upsert one row per app (tools cached straight from the module, no probe),
 *  drop rows whose app went away — rolling the agents that carried them. */
export async function syncAppMcpServers(): Promise<void> {
  const sql = await db()
  const [{ enabledApps }, { appHasMcp, appMcpTools }, { rollAgentsForServer }] = await Promise.all([
    import('./apps'),
    import('./app-mcp'),
    import('./mcp-apply'),
  ])
  const want = (await enabledApps()).filter((a) => appHasMcp(a.slug))
  const have = (await sql`select id, app_slug as "appSlug" from mcp_servers where app_slug is not null`) as unknown as Array<{
    id: string
    appSlug: string
  }>
  for (const row of have) {
    if (!want.some((a) => a.slug === row.appSlug)) {
      await rollAgentsForServer(row.id).catch(() => {})
      await sql`delete from mcp_servers where id = ${row.id}`
    }
  }
  for (const a of want) {
    const tools = await appMcpTools(a.slug)
    await sql`
      insert into mcp_servers (name, label, description, url, all_agents, app_slug, tools, tools_refreshed_at, created_by)
      values (${`app-${a.slug}`}, ${a.name}, ${a.description || null}, ${`talaria-app://${a.slug}`}, false, ${a.slug},
              ${sql.json(tools)}, now(), 'talaria')
      on conflict (name) do update set
        label = excluded.label, description = excluded.description, app_slug = excluded.app_slug,
        tools = excluded.tools, tools_refreshed_at = now(), enabled = true, updated_at = now()
    `
  }
}

// ── Discovery ───────────────────────────────────────────────────────────────

/** Ask the upstream for its tool catalog (initialize + tools/list) and cache
 *  it. MCP streamable-HTTP: some servers answer JSON, some SSE-frame it. */
/** ONE JSON-RPC CONVERSATION WITH A SERVER, AS THE ORG.
 *
 *  Factored out of `refreshMcpTools` when `callMcpTool` needed the identical
 *  five lines of header assembly and session handling. Both callers are
 *  PLATFORM-INITIATED — Talaria asking a server something on the org's behalf,
 *  not an agent acting for a person — which is why the org bearer is right here
 *  and why nothing in this helper takes a user.
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
    // row is Talaria's own MCP service on loopback, written by ensureBuiltinMcp()
    // and un-editable by design (updateMcpServer refuses url/headers patches on
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
    // identity `refreshMcpTools` uses for the builtin toolkit.
    const { dispatchAppMcp } = await import('./app-mcp')
    const out = await dispatchAppMcp(server.appSlug, { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: tool, arguments: args } }, 'talaria', null)
    return readToolResult(out.body, serverName, tool)
  }

  const { call } = await orgSession(server)
  const init = await call({
    jsonrpc: '2.0',
    id: 1,
    method: 'initialize',
    params: { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'talaria', version: '1.0' } },
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

export async function refreshMcpTools(id: string): Promise<{ tools: Array<{ name: string; description?: string }> } | { error: string }> {
  const server = await getMcpServer(id)
  if (!server) return { error: 'not found' }
  if (server.appSlug) {
    // App servers: the catalog comes straight from the compiled module.
    const { appMcpTools } = await import('./app-mcp')
    const tools = await appMcpTools(server.appSlug)
    const sql = await db()
    await sql`update mcp_servers set tools = ${sql.json(tools)}, tools_refreshed_at = now(), updated_at = now() where id = ${id}`
    return { tools }
  }
  // The Workbench is IN-PROCESS, and `talaria-workbench://core` is a routing
  // token rather than an endpoint — there is nothing to connect to, so this
  // never had a working discovery path. It only looked fine because
  // ensureBuiltinMcp seeds the row from the same module export; pressing
  // refresh then tried to fetch a URL with a scheme no client resolves. Read
  // the catalog the way the app-server branch above does.
  if (!/^https?:\/\//i.test(server.url)) {
    const { WORKBENCH_TOOLS } = await import('./workbench-mcp')
    const tools = WORKBENCH_TOOLS.map((t) => ({ name: t.name, description: t.description.slice(0, 300) }))
    const sql = await db()
    await sql`update mcp_servers set tools = ${sql.json(tools)}, tools_refreshed_at = now(), updated_at = now() where id = ${id}`
    return { tools }
  }
  // The built-in toolkit runs as a child of this app, spawned OPPORTUNISTICALLY
  // (renders, comms reads). On a freshly booted instance none of those has
  // happened, so the endpoint is simply not up yet and refresh reported the
  // platform's own tools unreachable. Start it and wait, rather than probing a
  // port nothing is listening on.
  if (server.builtin) {
    const { awaitMcpService } = await import('./mcp-service')
    if (!(await awaitMcpService())) {
      return { error: 'the Talaria toolkit service did not start; check the app logs' }
    }
  }
  try {
    const { call } = await orgSession(server)
    const init = await call({
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'talaria', version: '1.0' } },
    })
    if (init.status >= 400) return { error: `upstream ${init.status}` }
    const list = await call({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }, init.session)
    const tools = ((list.json as { result?: { tools?: Array<{ name: string; description?: string }> } })?.result?.tools ?? []).map(
      (t) => ({ name: t.name, description: t.description?.slice(0, 300) }),
    )
    const sql = await db()
    await sql`update mcp_servers set tools = ${sql.json(tools)}, tools_refreshed_at = now(), updated_at = now() where id = ${id}`
    return { tools }
  } catch (e) {
    return { error: (e as Error).message }
  }
}

/** JSON body, or the LAST data frame of an SSE-encoded response. */
export function parseMcpResponse(text: string): unknown {
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

// ── Assignments + user access ───────────────────────────────────────────────

export async function listAssignments(serverId: string): Promise<McpAssignment[]> {
  const sql = await db()
  return (await sql`
    select agent_model as "agentModel", tools from mcp_server_agents where server_id = ${serverId} order by agent_model
  `) as unknown as McpAssignment[]
}

export async function setAssignment(serverId: string, agentModel: string, tools: string[] | null): Promise<void> {
  const sql = await db()
  await sql`
    insert into mcp_server_agents (server_id, agent_model, tools) values (${serverId}, ${agentModel}, ${tools})
    on conflict (server_id, agent_model) do update set tools = ${tools}
  `
}

export async function removeAssignment(serverId: string, agentModel: string): Promise<void> {
  const sql = await db()
  await sql`delete from mcp_server_agents where server_id = ${serverId} and agent_model = ${agentModel}`
}

export async function listUserAccess(serverId: string): Promise<McpUserAccess[]> {
  const sql = await db()
  return (await sql`
    select user_id as "userId", allowed, tools from mcp_user_access where server_id = ${serverId}
  `) as unknown as McpUserAccess[]
}

export async function setUserAccess(serverId: string, userId: string, allowed: boolean | null, tools: string[] | null): Promise<void> {
  const sql = await db()
  if (allowed === null) {
    await sql`delete from mcp_user_access where server_id = ${serverId} and user_id = ${userId}`
  } else {
    await sql`
      insert into mcp_user_access (server_id, user_id, allowed, tools) values (${serverId}, ${userId}, ${allowed}, ${tools})
      on conflict (server_id, user_id) do update set allowed = ${allowed}, tools = ${tools}
    `
  }
}

// ── Per-user connected accounts ─────────────────────────────────────────────

export async function setUserCredentials(serverId: string, userId: string, headers: Record<string, string> | null): Promise<void> {
  const sql = await db()
  if (!headers || Object.keys(headers).length === 0) {
    await sql`delete from mcp_user_credentials where server_id = ${serverId} and user_id = ${userId}`
    return
  }
  await sql`
    insert into mcp_user_credentials (server_id, user_id, headers_enc) values (${serverId}, ${userId}, ${seal(JSON.stringify(headers))})
    on conflict (server_id, user_id) do update set headers_enc = ${seal(JSON.stringify(headers))}, updated_at = now()
  `
}

export async function hasUserCredentials(serverId: string, userId: string): Promise<boolean> {
  const sql = await db()
  return (await sql`select 1 as ok from mcp_user_credentials where server_id = ${serverId} and user_id = ${userId}`).length > 0
}

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

/** Every server an agent should carry in its rendered config (gateway URLs).
 *
 *  This one legitimately answers about a THIRD PARTY — fleet-render and the
 *  /api/mcp admin listing ask "what should <model> carry?" with no caller in
 *  hand — so a bare model string stays accepted. It grants nothing on its own:
 *  the credential is never rendered, only a gateway URL, and the gateway
 *  re-derives access per request through `effectiveMcpFor`. Pass the caller
 *  anyway wherever one exists. */
export async function serversForAgent(agent: AgentSubject): Promise<Array<{ name: string; timeoutSecs: number | null }>> {
  const agentModel = subjectModel(agent)
  const sql = await db()
  const rows = (await sql`
    select s.name, s.timeout_secs as "timeoutSecs", s.auth_mode as "authMode", (s.oauth is not null) as "oauthEnabled", s.id
    from mcp_servers s
    where s.enabled and not s.builtin and (s.all_agents or exists (
      select 1 from mcp_server_agents a where a.server_id = s.id and a.agent_model = ${agentModel}
    ))
    order by s.name
  `) as unknown as Array<{ name: string; timeoutSecs: number | null; authMode: string; oauthEnabled: boolean; id: string }>
  const owner = await assistantOwnerFor(agent)
  const out: Array<{ name: string; timeoutSecs: number | null }> = []
  for (const r of rows) {
    if (r.authMode === 'per-user') {
      // Only rendered once the acting user actually connected an account.
      if (!owner) continue
      const connected = r.oauthEnabled ? await hasOauthTokens(r.id, owner) : await hasUserCredentials(r.id, owner)
      if (!connected) continue
    } else if (r.oauthEnabled && !(await hasOauthTokens(r.id, 'org'))) {
      continue // org account not connected yet — keep it out of configs
    } else if (owner) {
      // A PA skips servers its owner is explicitly denied.
      const [access] = (await sql`
        select allowed from mcp_user_access where server_id = ${r.id} and user_id = ${owner}
      `) as unknown as Array<{ allowed: boolean }>
      if (access && !access.allowed) continue
    }
    out.push({ name: r.name, timeoutSecs: r.timeoutSecs })
  }
  return out
}
