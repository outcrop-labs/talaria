// Brain routability — is each agent's configured model still servable?
// Provider pools churn under no-train routing (a model can drop out of the US
// pool mid-day); when an agent's rendered model loses its route, chats freeze
// silently. This probes every enabled agent's config targets against the
// gateway registry (llm_endpoints) so the failure surfaces on /agents and in
// alerts instead of as a hung reply.
import { listAgentDefs, listEndpoints } from './agent-defs'

export interface BrainTarget {
  kind: 'main' | 'tier' | 'fallback'
  /** Tier alias name, when kind === 'tier'. */
  name?: string
  endpoint: string
  model: string
  ok: boolean
  reason?: 'endpoint missing' | 'model not on endpoint'
}

export interface AgentBrainHealth {
  /** The agent's fleet model id (agent_defs.model). */
  agent: string
  displayName: string
  /** Main brain routable? Tiers/fallbacks degrade; the main brain freezes chat. */
  ok: boolean
  targets: BrainTarget[]
}

let cache: { at: number; value: AgentBrainHealth[] } | null = null
const TTL_MS = 30_000

/** Routability of every enabled agent's brain targets (30s cache). */
export async function fleetBrainHealth(): Promise<AgentBrainHealth[]> {
  if (cache && Date.now() - cache.at < TTL_MS) return cache.value
  const [defs, endpoints] = await Promise.all([listAgentDefs(), listEndpoints()])
  const byName = new Map(endpoints.map((e) => [e.name, e]))

  const probe = (kind: BrainTarget['kind'], t: { endpoint: string; model: string }, name?: string): BrainTarget => {
    const ep = byName.get(t.endpoint)
    const ok = !!ep && ep.models.includes(t.model)
    return {
      kind,
      ...(name ? { name } : {}),
      endpoint: t.endpoint,
      model: t.model,
      ok,
      ...(ok ? {} : { reason: ep ? ('model not on endpoint' as const) : ('endpoint missing' as const) }),
    }
  }

  const value: AgentBrainHealth[] = []
  for (const d of defs) {
    if (!d.enabled) continue
    const cfg = d.latest?.config
    const targets: BrainTarget[] = []
    if (cfg?.main) targets.push(probe('main', cfg.main))
    for (const a of cfg?.aliases ?? []) targets.push(probe('tier', a, a.name))
    for (const f of cfg?.fallbacks ?? []) targets.push(probe('fallback', f))
    value.push({
      agent: d.model,
      displayName: d.displayName,
      // No main target at all = unroutable: nothing to serve chat with.
      ok: targets.some((t) => t.kind === 'main') ? targets.every((t) => t.kind !== 'main' || t.ok) : false,
      targets,
    })
  }
  cache = { at: Date.now(), value }
  return value
}
