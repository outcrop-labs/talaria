// Cascading model/endpoint removal: when a model (or whole endpoint) is
// deleted from the registry while agents still target it, reconfigure those
// agents — a NEW version per agent with the targets stripped (auditable),
// re-rendered, and running managed containers restarted. An agent's MAIN
// model is never cascaded away: that requires an explicit reassignment.
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

const hits = (t: ModelTarget | undefined, endpoint: string, model: string | null) =>
  !!t && t.endpoint === endpoint && (model === null || t.model === model)

/** Every agent whose CURRENT version targets endpoint (+model). */
export async function modelUsage(endpoint: string, model: string | null): Promise<ModelUsage[]> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.slug, d.department, d.managed, d.enabled, v.config
    from agent_defs d
    join agent_versions v on v.agent_id = d.id and v.version = d.current_version
  `) as unknown as Array<{ id: string; slug: string; department: string; managed: boolean; enabled: boolean; config: AgentConfig }>
  const out: ModelUsage[] = []
  for (const r of rows) {
    const asMain = hits(r.config.main, endpoint, model)
    const aliases = (r.config.aliases ?? []).filter((a) => hits(a, endpoint, model)).map((a) => a.name)
    const fallbacks = (r.config.fallbacks ?? []).filter((f) => hits(f, endpoint, model)).length
    if (asMain || aliases.length || fallbacks) {
      out.push({ slug: r.slug, defId: r.id, department: r.department, managed: r.managed, enabled: r.enabled, asMain, aliases, fallbacks })
    }
  }
  return out
}

/** Strip alias/fallback targets of endpoint(+model) from every affected agent
 *  (new version each), re-render, restart running managed agents. Callers must
 *  have already verified no agent uses it as MAIN. */
export async function cascadeRemoval(endpoint: string, model: string | null, actor: string): Promise<string[]> {
  const affected = await modelUsage(endpoint, model)
  const changed: string[] = []
  for (const u of affected) {
    const latest = (await listVersions(u.defId))[0]
    if (!latest) continue
    const cfg = latest.config
    const keptAliases = (cfg.aliases ?? []).filter((a) => !hits(a, endpoint, model))
    const keptFallbacks = (cfg.fallbacks ?? []).filter((f) => !hits(f, endpoint, model))
    const raw = { ...((cfg.raw ?? {}) as Record<string, unknown>) }
    const rawAliases = { ...((raw.model_aliases ?? {}) as Record<string, unknown>) }
    for (const a of cfg.aliases ?? []) if (hits(a, endpoint, model)) delete rawAliases[a.name]
    raw.model_aliases = rawAliases
    if (Array.isArray(raw.fallback_providers)) {
      raw.fallback_providers = (raw.fallback_providers as Array<Record<string, unknown>>).filter((_, i) => {
        const f = (cfg.fallbacks ?? [])[i]
        return !f || !hits(f, endpoint, model)
      })
    }
    const { created } = await addVersionIfChanged(u.defId, {
      soul: latest.soul,
      config: { ...cfg, aliases: keptAliases, fallbacks: keptFallbacks, raw },
      note: `removed ${endpoint}${model ? `/${model}` : ''} (deleted in Models)`,
      createdBy: actor,
    })
    if (created) changed.push(u.slug)
  }
  if (changed.length) {
    await renderFleet()
    for (const u of affected) {
      if (u.managed && u.enabled && changed.includes(u.slug)) {
        await fleetRestart(u.department).catch(() => {}) // stopped agents pick it up on next start
      }
    }
  }
  return changed
}
