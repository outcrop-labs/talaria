// Per-agent MCP servers, read from and written to the versioned Hermes config
// (raw.mcp_servers). Edits ride the same machinery as model edits: a NEW
// immutable version, optionally applied (re-render + restart). Untouched
// entries pass through byte-for-byte — headers, tool filters, timeouts all
// survive.
import { db } from './db/pg'
import { MCP_FLEET_URL } from './mcp-service'
import type { AgentConfig } from './agent-defs'

export interface McpServerEntry {
  name: string
  url: string
  timeout: number | null
  /** Extra config (headers, tools include/exclude) present in the raw entry. */
  extras: string[]
}

export interface AgentMcp {
  id: string
  slug: string
  displayName: string
  managed: boolean
  servers: McpServerEntry[]
}

type RawMcp = Record<string, Record<string, unknown>>

const rawServers = (config: AgentConfig | null): RawMcp =>
  ((config?.raw as Record<string, unknown> | undefined)?.mcp_servers ?? {}) as RawMcp

export async function listAgentMcp(): Promise<AgentMcp[]> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.slug, d.display_name as "displayName", d.managed, v.config
    from agent_defs d
    left join agent_versions v on v.agent_id = d.id and v.version = d.current_version
    where d.enabled order by d.slug
  `) as unknown as Array<{ id: string; slug: string; displayName: string; managed: boolean; config: AgentConfig | null }>
  return rows.map((r) => ({
    id: r.id,
    slug: r.slug,
    displayName: r.displayName,
    managed: r.managed,
    servers: [
      // The Talaria toolkit is injected at RENDER time (not stored in the
      // version config) — surface it here so admins can see and probe it.
      ...(r.managed
        ? [{ name: 'talaria', url: MCP_FLEET_URL(), timeout: null, extras: ['built-in'] }]
        : []),
      ...Object.entries(rawServers(r.config))
        .filter(([name]) => name !== 'talaria')
        .map(([name, entry]) => ({
          name,
          url: String(entry.url ?? ''),
          timeout: typeof entry.timeout === 'number' ? entry.timeout : null,
          extras: Object.keys(entry).filter((k) => !['url', 'timeout'].includes(k)),
        })),
    ],
  }))
}

/** Merge add/remove onto the previous config's raw.mcp_servers. Added entries
 *  get the agent's X-Agent-Name header (matching the fleet's convention). */
export function applyMcpEdits(
  prev: AgentConfig,
  slug: string,
  edits: { add: Array<{ name: string; url: string; timeout?: number }>; remove: string[] },
): AgentConfig {
  const prevRaw = (prev.raw ?? {}) as Record<string, unknown>
  const servers: RawMcp = { ...rawServers(prev) }
  for (const name of edits.remove) delete servers[name]
  for (const s of edits.add) {
    servers[s.name] = {
      url: s.url,
      headers: { 'X-Agent-Name': slug },
      ...(s.timeout ? { timeout: s.timeout } : {}),
    }
  }
  const raw = { ...prevRaw, mcp_servers: servers }
  return { ...prev, mcpServers: Object.keys(servers), raw }
}
