// Derived system alerts — computed from live state on every read, no tables.
// Sources: managed-container reality (docker), gateway reachability, the token
// ledger's blind spots (unpriced cloud usage, estimated counts), and stuck
// tickets on boards the requesting user can access.
import { db } from './db/pg'
import { listAgents } from './gateway'
import { containerStatus } from './fleet-docker'
import { costOverview } from './usage'
import { fleetBrainHealth } from './brain-health'
import { ragHealth } from './retrieval/backfill'
import { retrievalUpgradeStatus } from './retrieval/migrate'
import { MCP_PORT } from './mcp-service'

export type AlertSeverity = 'critical' | 'warning' | 'info'

export interface Alert {
  severity: AlertSeverity
  title: string
  detail: string
  href: string
}

const fmt = (n: number) => n.toLocaleString('en-US')

// Polled by /alerts (60s) and Home (30s): a short cache keeps repeat loads
// instant, and the probes below run in PARALLEL — serially they added up to
// seconds (docker exec + four network probes).
const alertsCache = new Map<string, { at: number; value: Alert[] }>()
const ALERTS_TTL_MS = 15_000

export async function computeAlerts(userId: string): Promise<Alert[]> {
  const hit = alertsCache.get(userId)
  if (hit && Date.now() - hit.at < ALERTS_TTL_MS) return hit.value
  const value = await computeAlertsFresh(userId)
  alertsCache.set(userId, { at: Date.now(), value })
  return value
}

async function computeAlertsFresh(userId: string): Promise<Alert[]> {
  const sql = await db()
  const alerts: Alert[] = []

  // ── Fleet: every enabled managed agent should have a running container ─────
  const managed = (await sql`
    select slug, department, display_name as "displayName" from agent_defs
    where managed and enabled order by slug
  `) as unknown as Array<{ slug: string; department: string; displayName: string }>
  // Everything independent, at once — the wall-clock is max(probe), not sum.
  const [states, gatewaySource, mcpUp, rag, upgrade, brains, cost, stuck] = await Promise.all([
    managed.length ? containerStatus(managed.map((m) => m.department)).catch(() => null) : Promise.resolve(null),
    listAgents().then((a) => a.source).catch(() => 'mock' as const),
    fetch(`http://127.0.0.1:${MCP_PORT()}/mcp`, { method: 'POST', signal: AbortSignal.timeout(2_500) })
      .then((r) => r.status === 401 || r.ok)
      .catch(() => false),
    ragHealth().catch(() => ({ qdrant: false, embeddings: false })),
    retrievalUpgradeStatus().catch(() => null),
    fleetBrainHealth().catch(() => [] as Awaited<ReturnType<typeof fleetBrainHealth>>),
    costOverview().catch(() => null),
    sql`
      select t.id, t.title, t.status, b.id as board_id, b.name as board,
             extract(day from now() - t.updated_at)::int as days
      from tasks t
      join boards b on b.id = t.board_id
      left join board_members m on m.board_id = b.id and m.user_id = ${userId}
      left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
      where (m.user_id is not null or tm.user_id is not null)
        and b.archived_at is null and t.archived_at is null
        and (t.status = 'failed' or (t.status = 'blocked' and t.updated_at < now() - interval '7 days'))
      order by t.updated_at asc limit 20
    ` as unknown as Promise<Array<{ id: string; title: string; status: string; board_id: string; board: string; days: number }>>,
  ])

  if (managed.length) {
    if (states) {
      for (const m of managed) {
        const st = states.find((s) => s.department === m.department)?.managed
        if (!st || st.state !== 'running') {
          alerts.push({
            severity: 'critical',
            title: `${m.displayName} is down`,
            detail: st ? `container ${st.name} is ${st.state} (${st.status})` : 'no managed container exists — render + up from /agents',
            href: '/agents',
          })
        } else if (/unhealthy/i.test(st.status)) {
          alerts.push({
            severity: 'warning',
            title: `${m.displayName} is unhealthy`,
            detail: st.status,
            href: '/agents',
          })
        }
      }
    }
  }

  // ── Gateway plane reachability ──────────────────────────────────────────────
  if (gatewaySource !== 'gateway') {
    alerts.push({
      severity: 'critical',
      title: 'Gateway plane unreachable',
      detail: 'The fleet multiplexer on :8642 is not answering — chat and channel replies will fail.',
      href: '/agents',
    })
  }

  // ── Agent toolkit endpoint: the fleet's MCP must answer or agents flail ────
  // A dead toolkit is silent from the app's side — agents just fail their
  // tool calls and improvise badly. Probe it like any other plane.
  if (!mcpUp) {
    alerts.push({
      severity: 'critical',
      title: 'Agent toolkit (MCP) unreachable',
      detail: `The fleet toolkit endpoint on :${MCP_PORT()} is not answering — every agent's Talaria tools (knowledge, tickets, documents, research) are failing. It respawns on the next comms read; if this persists, check the app logs.`,
      href: '/agents',
    })
  }

  // ── Retrieval plane: Qdrant + embeddings must be up or the brains starve ───
  // Indexing is fire-and-forget by design, so a dead service fails silently —
  // this alert is the guarantee it can't stay silent.
  if (!rag.qdrant || !rag.embeddings) {
    const down = [!rag.qdrant && 'Qdrant (vector store)', !rag.embeddings && 'embeddings (TEI)'].filter(Boolean).join(' and ')
    alerts.push({
      severity: 'critical',
      title: 'Retrieval plane down',
      detail: `${down} unreachable — nothing new is being indexed and agent knowledge search fails. Start the services (docker/dev-compose.yml), then run the backfill in Admin → Retrieval.`,
      href: '/admin',
    })
  } else {
    // Services up, but do the collections still match the live embedding
    // model? A TALARIA_EMBED_MODEL swap changes dimensions and every index/
    // search against the old collections fails just as silently as an outage.
    if (upgrade?.dimMismatch) {
      const bad = upgrade.collections.filter((c) => c.dimMismatch).map((c) => c.name).join(', ')
      alerts.push({
        severity: 'critical',
        title: 'Embedding model changed — brains need a rebuild',
        detail: `The embedding service now serves ${upgrade.embed?.modelId} (${upgrade.embed?.dim}d) but ${bad} ${upgrade.collections.filter((c) => c.dimMismatch).length === 1 ? 'is' : 'are'} built at a different dimension — indexing and search against ${upgrade.collections.filter((c) => c.dimMismatch).length === 1 ? 'it' : 'them'} are failing. Run the rebuild in Admin → Retrieval.`,
        href: '/admin',
      })
    }
  }

  // ── Brain routability: configured models still on the registry? ────────────
  // Provider pools churn; an agent whose main model lost its route freezes
  // silently mid-chat. Main = critical; tiers/fallbacks = one warning.
  for (const b of brains) {
    const badMain = b.targets.find((t) => t.kind === 'main' && !t.ok)
    if (!b.ok) {
      alerts.push({
        severity: 'critical',
        title: `${b.displayName}'s brain is unroutable`,
        detail: badMain
          ? `${badMain.endpoint}/${badMain.model}: ${badMain.reason} — chats will hang. Fix the model on /models or repoint the agent.`
          : 'no main model configured — the agent has nothing to think with',
        href: '/agents',
      })
    } else {
      const degraded = b.targets.filter((t) => !t.ok)
      if (degraded.length) {
        alerts.push({
          severity: 'warning',
          title: `${b.displayName}: ${degraded.length} ${degraded.length === 1 ? 'tier/fallback is' : 'tiers/fallbacks are'} unroutable`,
          detail: degraded.map((t) => `${t.kind}${t.name ? ` "${t.name}"` : ''} → ${t.endpoint}/${t.model} (${t.reason})`).join('; '),
          href: '/agents',
        })
      }
    }
  }

  // ── Ledger blind spots ──────────────────────────────────────────────────────
  if (cost) {
    if (cost.totals.unpricedCloudTokens > 0) {
      alerts.push({
        severity: 'warning',
        title: 'Unpriced cloud usage',
        detail: `${fmt(cost.totals.unpricedCloudTokens)} cloud tokens (30d) have no rate — spend is understated. Set prices on /models.`,
        href: '/models',
      })
    }
    if (cost.totals.month.generations >= 5 && cost.totals.estimatedShare > 0.5) {
      alerts.push({
        severity: 'warning',
        title: 'Token counts are mostly estimates',
        detail: `${Math.round(cost.totals.estimatedShare * 100)}% of the last 30 days' generations lack real usage from the gateway.`,
        href: '/observability?tab=cost',
      })
    }
  }

  // ── Stuck work (scoped to boards this user can see) ────────────────────────
  for (const t of stuck) {
    alerts.push(
      t.status === 'failed'
        ? {
            severity: 'warning',
            title: `Failed: ${t.title}`,
            detail: `on ${t.board} — needs a human decision`,
            href: `/boards/${t.board_id}/${t.id}`,
          }
        : {
            severity: 'info',
            title: `Blocked ${t.days}d: ${t.title}`,
            detail: `on ${t.board} — blocked for ${t.days} days`,
            href: `/boards/${t.board_id}/${t.id}`,
          },
    )
  }

  const rank: Record<AlertSeverity, number> = { critical: 0, warning: 1, info: 2 }
  return alerts.sort((a, b) => rank[a.severity] - rank[b.severity])
}
