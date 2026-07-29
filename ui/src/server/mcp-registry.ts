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
import { db } from './db/pg'
import { seal, open } from './secretbox'
import { personalAssistantOwners } from './users'

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
  created_by as "createdBy", created_at as "createdAt"`

export async function listMcpServers(): Promise<McpServer[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from mcp_servers order by name`)) as unknown as McpServer[]
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
  createdBy: string
}): Promise<McpServer> {
  const sql = await db()
  const rows = (await sql`
    insert into mcp_servers (name, label, description, url, headers, timeout_secs, auth_mode, created_by)
    values (${input.name}, ${input.label ?? input.name}, ${input.description ?? null}, ${input.url},
            ${sql.json(input.headers ?? {})}, ${input.timeoutSecs ?? null}, ${input.authMode ?? 'org'}, ${input.createdBy})
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
  await sql`delete from mcp_servers where id = ${id}` // assignments/access/credentials cascade
}

// ── Discovery ───────────────────────────────────────────────────────────────

/** Ask the upstream for its tool catalog (initialize + tools/list) and cache
 *  it. MCP streamable-HTTP: some servers answer JSON, some SSE-frame it. */
export async function refreshMcpTools(id: string): Promise<{ tools: Array<{ name: string; description?: string }> } | { error: string }> {
  const server = await getMcpServer(id)
  if (!server) return { error: 'not found' }
  try {
    const call = async (body: unknown, sessionId?: string | null) => {
      const r = await fetch(server.url, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          accept: 'application/json, text/event-stream',
          ...server.headers,
          ...(sessionId ? { 'mcp-session-id': sessionId } : {}),
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout((server.timeoutSecs ?? 30) * 1000),
      })
      const session = r.headers.get('mcp-session-id')
      const text = await r.text()
      return { json: parseMcpResponse(text), session, status: r.status }
    }
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
 *  per-user servers. Null = no access at all. */
export async function effectiveMcpFor(agentModel: string, serverName: string): Promise<EffectiveMcp | null> {
  const server = await getMcpServer(serverName)
  if (!server || !server.enabled) return null
  const sql = await db()

  let agentTools: string[] | null = null
  if (!server.allAgents) {
    const rows = (await sql`
      select tools from mcp_server_agents where server_id = ${server.id} and agent_model = ${agentModel}
    `) as unknown as Array<{ tools: string[] | null }>
    if (rows.length === 0) return null
    agentTools = rows[0]!.tools
  }

  const owner = (await personalAssistantOwners()).get(agentModel) ?? null
  let userTools: string[] | null = null
  let upstreamHeaders = server.headers
  if (owner) {
    const [access] = (await sql`
      select allowed, tools from mcp_user_access where server_id = ${server.id} and user_id = ${owner}
    `) as unknown as Array<{ allowed: boolean; tools: string[] | null }>
    if (access && !access.allowed) return null
    userTools = access?.tools ?? null
    if (server.authMode === 'per-user') {
      const creds = await getUserCredentials(server.id, owner)
      if (!creds) return null // not connected — the server doesn't exist for this assistant yet
      upstreamHeaders = { ...server.headers, ...creds }
    }
  } else if (server.authMode === 'per-user') {
    // Org agents have no single acting user to be — per-user servers are
    // personal-assistant territory by definition.
    return null
  }

  return { server, tools: intersect(agentTools, userTools), upstreamHeaders }
}

/** Every server an agent should carry in its rendered config (gateway URLs). */
export async function serversForAgent(agentModel: string): Promise<Array<{ name: string; timeoutSecs: number | null }>> {
  const sql = await db()
  const rows = (await sql`
    select s.name, s.timeout_secs as "timeoutSecs", s.auth_mode as "authMode", s.id
    from mcp_servers s
    where s.enabled and (s.all_agents or exists (
      select 1 from mcp_server_agents a where a.server_id = s.id and a.agent_model = ${agentModel}
    ))
    order by s.name
  `) as unknown as Array<{ name: string; timeoutSecs: number | null; authMode: string; id: string }>
  const owner = (await personalAssistantOwners()).get(agentModel) ?? null
  const out: Array<{ name: string; timeoutSecs: number | null }> = []
  for (const r of rows) {
    if (r.authMode === 'per-user') {
      // Only rendered once the acting user actually connected an account.
      if (!owner || !(await hasUserCredentials(r.id, owner))) continue
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
