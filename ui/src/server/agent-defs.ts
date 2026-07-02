// Agent harness — Talaria-owned agent definitions and their versioned config.
// A definition is the identity (slug/department/model); a version is the whole
// configurable surface (soul, models/aliases, fallbacks, tools) as an immutable
// payload. Rendering + orchestration (Phase B) consume the current version.
import { db } from './db/pg'

export interface LlmEndpoint {
  id: string
  name: string
  provider: string
  baseUrl: string | null
  class: 'local' | 'cloud'
  apiKeyEnv: string | null
  contextLength: number | null
  priceInPerMtok: number | null
  priceOutPerMtok: number | null
}

export interface AgentDef {
  id: string
  slug: string
  department: string
  model: string
  displayName: string
  enabled: boolean
  managed: boolean
  /** 'imported' reuses the legacy stack's volumes/chassis; 'created' is fresh. */
  source: 'imported' | 'created'
  currentVersion: number
  createdAt: string
  updatedAt: string
}

/** A model target: which endpoint serves it and the upstream model id there. */
export interface ModelTarget {
  endpoint: string // llm_endpoints.name
  model: string
  contextLength?: number
}

/** The structured, Talaria-owned slice of an agent version's config. `raw`
 *  carries the full imported config.yaml so rendering loses nothing. */
export interface AgentConfig {
  main?: ModelTarget
  aliases?: Array<ModelTarget & { name: string }>
  fallbacks?: ModelTarget[]
  plugins?: string[]
  mcpServers?: string[]
  raw?: unknown
}

export interface AgentVersion {
  id: string
  agentId: string
  version: number
  soul: string
  config: AgentConfig
  note: string | null
  createdBy: string | null
  createdAt: string
}

// ── Endpoints ────────────────────────────────────────────────────────────────
export async function listEndpoints(): Promise<LlmEndpoint[]> {
  const sql = await db()
  return (await sql`
    select id, name, provider, base_url as "baseUrl", class, api_key_env as "apiKeyEnv",
           context_length as "contextLength", price_in_per_mtok as "priceInPerMtok",
           price_out_per_mtok as "priceOutPerMtok"
    from llm_endpoints order by (class = 'local') desc, name asc
  `) as unknown as LlmEndpoint[]
}

/** Idempotent registration keyed by name; existing class/pricing edits win. */
export async function ensureEndpoint(e: {
  name: string
  provider: string
  baseUrl?: string | null
  class: 'local' | 'cloud'
  apiKeyEnv?: string | null
  contextLength?: number | null
}): Promise<void> {
  const sql = await db()
  await sql`
    insert into llm_endpoints (name, provider, base_url, class, api_key_env, context_length)
    values (${e.name}, ${e.provider}, ${e.baseUrl ?? null}, ${e.class}, ${e.apiKeyEnv ?? null}, ${e.contextLength ?? null})
    on conflict (name) do update set
      provider = excluded.provider,
      base_url = excluded.base_url,
      updated_at = now()
  `
}

export async function updateEndpoint(
  id: string,
  patch: { class?: 'local' | 'cloud'; priceInPerMtok?: number | null; priceOutPerMtok?: number | null },
): Promise<void> {
  const sql = await db()
  if (patch.class) await sql`update llm_endpoints set class = ${patch.class}, updated_at = now() where id = ${id}`
  if (patch.priceInPerMtok !== undefined)
    await sql`update llm_endpoints set price_in_per_mtok = ${patch.priceInPerMtok}, updated_at = now() where id = ${id}`
  if (patch.priceOutPerMtok !== undefined)
    await sql`update llm_endpoints set price_out_per_mtok = ${patch.priceOutPerMtok}, updated_at = now() where id = ${id}`
}

// ── Definitions + versions ───────────────────────────────────────────────────
export async function listAgentDefs(): Promise<Array<AgentDef & { latest: AgentVersion | null }>> {
  const sql = await db()
  const defs = (await sql`
    select id, slug, department, model, display_name as "displayName", enabled, managed, source,
           current_version as "currentVersion", created_at as "createdAt", updated_at as "updatedAt"
    from agent_defs order by slug asc
  `) as unknown as AgentDef[]
  const versions = (await sql`
    select distinct on (agent_id)
           id, agent_id as "agentId", version, soul, config, note, created_by as "createdBy", created_at as "createdAt"
    from agent_versions order by agent_id, version desc
  `) as unknown as AgentVersion[]
  const byAgent = new Map(versions.map((v) => [v.agentId, v]))
  return defs.map((d) => ({ ...d, latest: byAgent.get(d.id) ?? null }))
}

export async function listVersions(agentId: string): Promise<AgentVersion[]> {
  const sql = await db()
  return (await sql`
    select id, agent_id as "agentId", version, soul, config, note, created_by as "createdBy", created_at as "createdAt"
    from agent_versions where agent_id = ${agentId} order by version desc
  `) as unknown as AgentVersion[]
}

export async function getAgentDef(id: string): Promise<AgentDef | null> {
  const sql = await db()
  const rows = await sql`
    select id, slug, department, model, display_name as "displayName", enabled, managed, source,
           current_version as "currentVersion", created_at as "createdAt", updated_at as "updatedAt"
    from agent_defs where id = ${id}
  `
  return (rows[0] as unknown as AgentDef) ?? null
}

export async function upsertAgentDef(input: {
  slug: string
  department: string
  displayName: string
  source?: 'imported' | 'created'
}): Promise<AgentDef> {
  const sql = await db()
  const model = `${input.slug}-${input.department}`
  const rows = await sql`
    insert into agent_defs (slug, department, model, display_name, source)
    values (${input.slug}, ${input.department}, ${model}, ${input.displayName}, ${input.source ?? 'imported'})
    on conflict (slug) do update set
      department = excluded.department, model = excluded.model,
      display_name = excluded.display_name, updated_at = now()
    returning id, slug, department, model, display_name as "displayName", enabled, managed, source,
              current_version as "currentVersion", created_at as "createdAt", updated_at as "updatedAt"
  `
  return rows[0] as unknown as AgentDef
}

/** Structured edits (from the UI) applied onto a previous version's config.
 *  The raw Hermes config is updated in the same stroke so rendering reflects
 *  the edit: model/base_url/api_key come from the endpoint registry; extra raw
 *  keys (e.g. context_length env refs) survive when the endpoint is unchanged. */
export async function applyConfigEdits(
  prev: AgentConfig,
  edits: { main: ModelTarget; aliases: Array<ModelTarget & { name: string }>; fallbacks: ModelTarget[] },
): Promise<AgentConfig> {
  const endpoints = new Map((await listEndpoints()).map((e) => [e.name, e]))
  const prevRaw = (prev.raw ?? {}) as Record<string, unknown>

  const block = (t: ModelTarget, prevBlock?: unknown): Record<string, unknown> => {
    const ep = endpoints.get(t.endpoint)
    if (!ep) throw new Error(`unknown endpoint "${t.endpoint}"`)
    const prevB = (prevBlock ?? {}) as Record<string, unknown>
    const samePlace = prevB.provider === ep.provider && (prevB.base_url ?? null) === (ep.baseUrl ?? null)
    return {
      ...(samePlace ? prevB : {}),
      provider: ep.provider,
      model: t.model,
      ...(ep.baseUrl ? { base_url: ep.baseUrl } : {}),
      ...(!samePlace && ep.apiKeyEnv ? { api_key: `\${${ep.apiKeyEnv}}` } : {}),
    }
  }

  const prevAliases = (prevRaw.model_aliases ?? {}) as Record<string, unknown>
  const raw: Record<string, unknown> = {
    ...prevRaw,
    model: block(edits.main, prevRaw.model),
    model_aliases: Object.fromEntries(edits.aliases.map((a) => [a.name, block(a, prevAliases[a.name])])),
    fallback_providers: edits.fallbacks.map((f) => block(f)),
  }
  return { ...prev, main: edits.main, aliases: edits.aliases, fallbacks: edits.fallbacks, raw }
}

/** Key-order-insensitive stringify — Postgres jsonb reorders object keys, so a
 *  plain JSON.stringify comparison would see every round-trip as a change. */
function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(',')}]`
  if (v && typeof v === 'object') {
    const entries = Object.entries(v as Record<string, unknown>)
      .filter(([, val]) => val !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    return `{${entries.map(([k, val]) => `${JSON.stringify(k)}:${canonical(val)}`).join(',')}}`
  }
  return JSON.stringify(v) ?? 'null'
}

/** Append a new version (and point current_version at it). Skips when the
 *  payload equals the latest version — safe to re-run imports. */
export async function addVersionIfChanged(
  agentId: string,
  payload: { soul: string; config: AgentConfig; note?: string; createdBy?: string },
): Promise<{ version: number; created: boolean }> {
  const sql = await db()
  const latest = (await sql`
    select version, soul, config from agent_versions
    where agent_id = ${agentId} order by version desc limit 1
  `) as unknown as Array<{ version: number; soul: string; config: AgentConfig }>
  const prev = latest[0]
  if (prev && prev.soul === payload.soul && canonical(prev.config) === canonical(payload.config)) {
    return { version: prev.version, created: false }
  }
  const next = (prev?.version ?? 0) + 1
  await sql.begin(async (tx) => {
    await tx`
      insert into agent_versions (agent_id, version, soul, config, note, created_by)
      values (${agentId}, ${next}, ${payload.soul},
              ${tx.json(payload.config as unknown as Parameters<typeof tx.json>[0])},
              ${payload.note ?? null}, ${payload.createdBy ?? null})
    `
    await tx`update agent_defs set current_version = ${next}, updated_at = now() where id = ${agentId}`
  })
  return { version: next, created: true }
}
