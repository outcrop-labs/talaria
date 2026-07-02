// Cascading model/endpoint removal: when models (or a whole endpoint) are
// deleted from the registry while agents still target them, reconfigure those
// agents — ONE new version per agent with every stripped tier (auditable),
// one re-render, one restart wave for running managed containers. An agent's
// MAIN model is never cascaded away: that requires an explicit reassignment.
import { db } from './db/pg'
import { addVersionIfChanged, listVersions, type AgentConfig, type ModelTarget } from './agent-defs'
import { fleetRestart } from './fleet-docker'
import { renderFleet } from './fleet-render'

export interface ModelUsage {
  slug: string
  defId: string
  department: string
  managed: boolean
  enabled: boolean
  asMain: boolean
  aliases: string[] // alias names targeting it
  fallbacks: number // fallback entries targeting it
}

/** models = null → any model on the endpoint; else only those model ids. */
const hits = (t: ModelTarget | undefined, endpoint: string, models: string[] | null) =>
  !!t && t.endpoint === endpoint && (models === null || models.includes(t.model))

/** Every agent whose CURRENT version targets endpoint (+models). */
export async function modelUsage(endpoint: string, models: string[] | null): Promise<ModelUsage[]> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.slug, d.department, d.managed, d.enabled, v.config
    from agent_defs d
    join agent_versions v on v.agent_id = d.id and v.version = d.current_version
  `) as unknown as Array<{ id: string; slug: string; department: string; managed: boolean; enabled: boolean; config: AgentConfig }>
  const out: ModelUsage[] = []
  for (const r of rows) {
    const asMain = hits(r.config.main, endpoint, models)
    const aliases = (r.config.aliases ?? []).filter((a) => hits(a, endpoint, models)).map((a) => a.name)
    const fallbacks = (r.config.fallbacks ?? []).filter((f) => hits(f, endpoint, models)).length
    if (asMain || aliases.length || fallbacks) {
      out.push({ slug: r.slug, defId: r.id, department: r.department, managed: r.managed, enabled: r.enabled, asMain, aliases, fallbacks })
    }
  }
  return out
}

export interface CascadeResult {
  changed: string[]
  /** Set when the post-cascade render/restart failed — agent VERSIONS are
   *  already written; re-render from /agents once the cause is fixed. */
  renderError?: string
}

/** Strip alias/fallback targets of endpoint(+models) from every affected agent
 *  (one new version each), then re-render once and restart running managed
 *  agents. Callers must have verified no agent uses the targets as MAIN. */
export async function cascadeRemoval(endpoint: string, models: string[] | null, actor: string): Promise<CascadeResult> {
  const affected = await modelUsage(endpoint, models)
  const changed: string[] = []
  const what = `${endpoint}${models ? `/${models.join(',')}` : ''}`
  for (const u of affected) {
    const latest = (await listVersions(u.defId))[0]
    if (!latest) continue
    const cfg = latest.config
    const keptAliases = (cfg.aliases ?? []).filter((a) => !hits(a, endpoint, models))
    const keptFallbacks = (cfg.fallbacks ?? []).filter((f) => !hits(f, endpoint, models))
    const raw = { ...((cfg.raw ?? {}) as Record<string, unknown>) }
    const rawAliases = { ...((raw.model_aliases ?? {}) as Record<string, unknown>) }
    for (const a of cfg.aliases ?? []) if (hits(a, endpoint, models)) delete rawAliases[a.name]
    raw.model_aliases = rawAliases
    if (Array.isArray(raw.fallback_providers)) {
      raw.fallback_providers = (raw.fallback_providers as Array<Record<string, unknown>>).filter((_, i) => {
        const f = (cfg.fallbacks ?? [])[i]
        return !f || !hits(f, endpoint, models)
      })
    }
    const { created } = await addVersionIfChanged(u.defId, {
      soul: latest.soul,
      config: { ...cfg, aliases: keptAliases, fallbacks: keptFallbacks, raw },
      note: `removed ${what} (deleted in Models)`,
      createdBy: actor,
    })
    if (created) changed.push(u.slug)
  }
  if (!changed.length) return { changed }
  try {
    await renderFleet()
    for (const u of affected) {
      if (u.managed && u.enabled && changed.includes(u.slug)) {
        await fleetRestart(u.department).catch(() => {}) // stopped agents pick it up on next start
      }
    }
    return { changed }
  } catch (e) {
    return { changed, renderError: (e as Error).message }
  }
}