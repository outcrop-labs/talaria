import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSql } from '@/server/db/pg'
import { getRedis } from '@/server/db/redis'
import { rustApiUrl } from '@/server/rust-proxy'
import { bootMigrationCheck } from '@/server/boot-health'

// Liveness/readiness for whatever is watching the process — a load balancer, a
// container orchestrator, an uptime monitor, a human at 2am. PUBLIC BY DESIGN:
// no session guard, because a health check that needs a session tells you
// nothing when auth (which itself needs Postgres and Redis) is what's broken.
//
// It must also be SAFE to expose: the body carries booleans, latencies and
// short error CODES only. No connection strings, no hostnames, no env, no
// driver messages — those can carry a database name, a user, or a host:port.

const PING_TIMEOUT_MS = 2_500

interface Check {
  ok: boolean
  latencyMs: number | null
  error?: string
}

/** Never let a wedged socket hold the health check open. */
function withTimeout(work: Promise<unknown>, ms: number): Promise<unknown> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const bell = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error('TIMEOUT')), ms)
  })
  return Promise.race([work, bell]).finally(() => clearTimeout(timer))
}

/** Driver errors are not safe to echo. Keep the short machine code (ECONNREFUSED,
 *  28P01, TIMEOUT) and drop everything else on the floor — the full error is
 *  logged server-side. */
function safeCode(e: unknown): string {
  const code = (e as { code?: unknown })?.code
  if (typeof code === 'string' && /^[A-Z0-9_]{1,20}$/.test(code)) return code
  const msg = e instanceof Error ? e.message : ''
  if (msg === 'TIMEOUT') return 'TIMEOUT'
  return 'unreachable'
}

async function timed(name: string, ping: () => Promise<unknown>): Promise<Check> {
  const started = Date.now()
  try {
    await withTimeout(ping(), PING_TIMEOUT_MS)
    return { ok: true, latencyMs: Date.now() - started }
  } catch (e) {
    console.error(`[healthz] ${name}`, e)
    return { ok: false, latencyMs: null, error: safeCode(e) }
  }
}
// doc: Liveness/readiness — SQL and Redis round-trips, plus a `migrations`
// doc: check that appears (and fails the probe) when the boot migration pass
// doc: died. PUBLIC BY DESIGN: no session guard, because a health check that
// doc: needs a session tells you nothing exactly when you need it.


export const Route = defineApi('/api/healthz', {
  GET: async () => {
    // getSql(), not db(): a health check must never run schema migrations
    // or wait on the migration advisory lock. Just a round trip.
    const checks: Record<string, Check> = {
      postgres: await timed('postgres', async () => {
        await getSql()`select 1`
      }),
      redis: await timed('redis', () => getRedis().ping()),
    }

    // The Rust api this process fronts — the same effective URL the proxy
    // would hop to (default loopback included; only `off` skips the check,
    // because only `off` means this process fronts no api at all). Any HTTP
    // answer counts as ok, including the api's own 503: its postgres and
    // redis are the same two the checks above already speak for, so the only
    // fact this check adds is the one only this side can see — the hop works.
    // The body stays secret-free by the same rule as the rest: the URL never
    // rides, safeCode keeps the short code only.
    const rustUrl = rustApiUrl()
    if (rustUrl) {
      checks.rustApi = await timed('rustApi', () =>
        fetch(new URL('/api/healthz', rustUrl), {
          signal: AbortSignal.timeout(PING_TIMEOUT_MS),
        }).then((res) => {
          void res.body?.cancel()
        }),
      )
    }

    // A boot migration pass that FAILED (not one still running — slow is not
    // failed; that path warns and the app still reaches listen()). Without
    // this check the app serves green while every table query 500s; with it,
    // the probe (compose healthcheck, deploy gates) sees the truth.
    const migrations = bootMigrationCheck()
    if (migrations) checks.migrations = migrations

    const ok = Object.values(checks).every((c) => c.ok)
    return json(
      {
        status: ok ? 'ok' : 'degraded',
        uptimeSeconds: Math.round(process.uptime()),
        checks,
      },
      {
        // 503 so a probe fails on its own, without parsing the body.
        status: ok ? 200 : 503,
        headers: { 'Cache-Control': 'no-store' },
      },
    )
  },
})
