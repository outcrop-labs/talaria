import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSql } from '@/server/db/pg'
import { getRedis } from '@/server/db/redis'

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

export const Route = defineApi('/api/healthz', {
  GET: async () => {
    // getSql(), not db(): a health check must never run schema migrations
    // or wait on the migration advisory lock. Just a round trip.
    const [postgres, redis] = await Promise.all([
      timed('postgres', async () => {
        await getSql()`select 1`
      }),
      timed('redis', () => getRedis().ping()),
    ])

    const ok = postgres.ok && redis.ok
    return json(
      {
        status: ok ? 'ok' : 'degraded',
        uptimeSeconds: Math.round(process.uptime()),
        checks: { postgres, redis },
      },
      {
        // 503 so a probe fails on its own, without parsing the body.
        status: ok ? 200 : 503,
        headers: { 'Cache-Control': 'no-store' },
      },
    )
  },
})
