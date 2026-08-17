// Derived system alerts — computed from live state on every read, no tables.
// Sources: managed-container reality (docker), gateway reachability, the token
// ledger's blind spots (unpriced cloud usage, estimated counts), the background
// scheduler (failing jobs, and whether it is running at all), the notification
// outbox (depth, age, breaker, what has been lost), and stuck tickets on boards
// the requesting user can access.
//
// THE RULE THIS FILE IS FOR: anything whose failure mode is SILENCE has to be
// asked here, because nothing else asks it. A dead container is obvious the
// moment you use the product; a paused mail queue, a job that stopped running
// and a scheduler that never started are all indistinguishable from a quiet
// week. Every one of those has a live number somewhere in the process — the
// work is bringing it to a person, not computing it.
import { db } from './db/pg'
import { listAgents } from './gateway'
import { containerStatus } from './fleet-docker'
import { costOverview } from './usage'
import { fleetBrainHealth } from './brain-health'
import { ragHealth } from './retrieval/backfill'
import { retrievalUpgradeStatus } from './retrieval/migrate'
import { MCP_PORT } from './mcp-service'
import { notificationMailStats } from './notifications'
import { schedulerStatus, unhealthyJobs } from './scheduler'

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

/** How long a mail may sit in the outbox before the queue is "stuck" rather
 *  than "busy". The drain runs every 30 seconds and gives itself a 25-second
 *  budget, so ten drains' worth of waiting is not a backlog moving slowly — it
 *  is a backlog not moving. */
const OUTBOX_STALE_MS = 5 * 60_000

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
  const [states, manifestCount, mcpUp, rag, upgrade, brains, cost, stuck] = await Promise.all([
    managed.length ? containerStatus(managed.map((m) => m.department)).catch(() => null) : Promise.resolve(null),
    listAgents().then((a) => a.length).catch(() => 0),
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
  // Only a failure when agents EXIST but nothing is rendered. An empty manifest
  // on an instance with no managed agents is a fresh install, not an outage —
  // this used to fire a critical alert at everybody who had not created an
  // agent yet, because "no manifest" and "unreachable" were the same value.
  if (managed.length > 0 && manifestCount === 0) {
    alerts.push({
      severity: 'critical',
      title: 'Gateway plane unreachable',
      detail: 'No rendered fleet manifest — Talaria has no agent url or key to reach, so chat and channel replies will fail. Render the fleet from /agents.',
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

  // ── Background jobs ─────────────────────────────────────────────────────────
  //
  // The digest, the approval SLA, comms decay, the notification mail drain. A
  // scheduled job is the one part of the product you cannot discover is broken
  // by using it: its failure mode is silence, and silence is what a quiet week
  // looks like too. The scheduler has always kept this in `status()`; this is
  // what reads it. Synchronous and in-process, so it costs nothing and is not
  // in the Promise.all above.
  for (const job of unhealthyJobs()) {
    alerts.push({
      severity: job.severity,
      title: `Background job "${job.name}" is not running cleanly`,
      detail: `${job.detail} Reported by the instance that served this request.`,
      href: '/observability',
    })
  }

  // ── Is the scheduler running AT ALL? ────────────────────────────────────────
  //
  // `unhealthyJobs()` is structurally blind to this and cannot be fixed there:
  // it iterates the SAME registry, so an empty one produces an empty list, and
  // its lateness checks are gated on the scheduler having started. Both of its
  // blind spots look identical from /observability — a clean board — and both
  // mean every job in this file's list above (the digest, the approval SLA,
  // comms decay, the mail drain) is not running. That is the failure this whole
  // file exists to make impossible to miss, so it is asked directly, of
  // `schedulerStatus()`, which until now nothing outside the scheduler read.
  const jobs = schedulerStatus()
  const unarmed = jobs.filter((j) => !j.firstRunDueAt)
  const inProduction = process.env.NODE_ENV === 'production'
  if (!jobs.length) {
    alerts.push({
      severity: 'critical',
      title: 'No background jobs are registered on this instance',
      detail:
        'The scheduler has an empty registry, so the daily digest, the approval SLA, comms decay and the' +
        ' notification mail drain are all not running — and none of them will report a failure, because none of' +
        ' them exists. Their modules were never imported by this build.',
      href: '/observability',
    })
  } else if (unarmed.length && unarmed.length < jobs.length) {
    // SOME armed and some not. That is not a configuration choice — the
    // scheduler arms everything in the registry in one pass — it is a job whose
    // module was imported AFTER `startScheduler()` ran, typically because a
    // route lazily imported it on first request. Nothing will ever time it, and
    // it is the one shape `unhealthyJobs()` cannot report either: its lateness
    // checks all need a `firstRunDueAt` to be late against.
    alerts.push({
      severity: 'critical',
      title: `Background job${unarmed.length === 1 ? '' : 's'} registered too late to be armed`,
      detail:
        `${unarmed.map((j) => `"${j.name}"`).join(', ')} registered after the scheduler started, so ${
          unarmed.length === 1 ? 'it is' : 'they are'
        } timed by nothing and will never run. A job's module has to be in the runtime graph before startScheduler() — imported from the server entry, not lazily from a route.`,
      href: '/observability',
    })
  } else if (unarmed.length === jobs.length) {
    // Registered but never armed: `startScheduler()` was not called, or it
    // returned early. Expected on a developer's machine (`vite dev` does not
    // run server-entry.js, deliberately), so it is only an emergency where a
    // build is actually serving people.
    alerts.push({
      severity: inProduction ? 'critical' : 'info',
      title: inProduction ? 'Background jobs are not running on this instance' : 'Background jobs are not armed (dev)',
      detail:
        `${jobs.length} job(s) are registered and not one of them has been armed, so nothing is timing them:` +
        ' no digest, no approval escalation, no comms decay, no notification mail.' +
        (inProduction
          ? ' Either TALARIA_SCHEDULER=off, or server-entry.js could not find the scheduler handle at boot — the' +
            ' startup log says which.'
          : ' Normal under `vite dev`, which does not run server-entry.js on purpose.'),
      href: '/observability',
    })
  }

  // ── The notification outbox ────────────────────────────────────────────────
  //
  // The mail queue is in memory, it is bounded, and it has a breaker — and a
  // breaker is exactly what makes a stuck queue INVISIBLE: the drain
  // short-circuits, so the job neither sends nor fails, and both the log and
  // every counter that reads "what happened this pass" go quiet while N mails sit
  // there going nowhere. The queue's own numbers are the only thing that can say
  // so, and `notificationMailStats()` had no readers at all. Now it has this one.
  const mail = notificationMailStats()
  const minutes = (ms: number) => Math.max(1, Math.round(ms / 60_000))
  if (mail.pausedForMs > 0) {
    alerts.push({
      severity: 'critical',
      title: 'Notification email is paused after repeated failures',
      // The failure count that opens the breaker is notifications.ts's to
      // state, not this file's — a second copy of the number here is how the
      // two drift and the alert starts describing a rule that changed.
      detail:
        `Enough sends failed in a row that the outbox breaker opened, so nothing is being attempted for another` +
        ` ${minutes(mail.pausedForMs)} minute(s). ` +
        (mail.queued === 0
          ? 'Nothing is queued this second, but nothing queued before it closes will be attempted either.'
          : `${mail.queued} mail(s) are queued${
              mail.oldestQueuedMs === null ? '' : `, the oldest waiting ${minutes(mail.oldestQueuedMs)} minute(s)`
            }. Every one of them is still in its recipient’s in-app inbox — only the email is late.`) +
        ' Check the mail provider (Admin → Org → Email).',
      href: '/observability',
    })
  } else if (mail.queued > 0 && (mail.oldestQueuedMs ?? 0) > OUTBOX_STALE_MS) {
    // Not paused and still not moving. The drain runs every 30 seconds, so mail
    // this old is a transport that is slow rather than dead — which produces no
    // error anywhere and is the other half of the same silence.
    alerts.push({
      severity: 'warning',
      title: 'Notification email is backing up',
      detail:
        `${mail.queued} of a possible ${mail.capacity} mail(s) are queued and the oldest has been waiting` +
        ` ${minutes(mail.oldestQueuedMs ?? 0)} minute(s), on a drain that runs every 30 seconds. The transport is not` +
        ' keeping up. Every one of them is still in its recipient’s in-app inbox.',
      href: '/observability',
    })
  } else if (mail.queued >= mail.capacity / 2) {
    alerts.push({
      severity: 'warning',
      title: 'Notification email queue is filling up',
      detail:
        `${mail.queued} of a possible ${mail.capacity} mail(s) are queued. Past ${mail.capacity} new mail is refused` +
        ' at the door — the notification still lands in the app, the email does not.',
      href: '/observability',
    })
  }
  if (mail.dropped > 0 || mail.abandoned > 0) {
    // Since boot, and cumulative on purpose: these are mails that will never be
    // sent, and "it recovered" does not un-lose them.
    const lost = [
      mail.dropped > 0 && `${fmt(mail.dropped)} refused at the door because the queue was full`,
      mail.abandoned > 0 && `${fmt(mail.abandoned)} given up on after repeated send failures`,
    ]
      .filter(Boolean)
      .join(', ')
    alerts.push({
      severity: 'warning',
      title: 'Notification emails have been lost since boot',
      detail: `${lost}. Each one is still unread in its recipient’s in-app inbox — the email was lost, the notification was not.`,
      href: '/observability',
    })
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
