// Rate limiting — Redis-backed, so it survives a restart and holds across
// instances. A fixed window (INCR + EXPIRE on first hit) rather than a sliding
// one: it can let through up to 2x the limit across a window boundary, which is
// the right trade for a brute-force brake where the goal is "thousands of
// guesses become dozens", not exact accounting.
import { getRedis } from './db/redis'

export interface RateLimitResult {
  /** False when the caller should be refused. */
  ok: boolean
  /** Seconds until the window resets — send as Retry-After. */
  retryAfter: number
}

/** Count one hit against `key`. Fails OPEN if Redis is unreachable: a limiter
 *  outage must not lock everyone out of their own deployment. */
export async function rateLimit(key: string, limit: number, windowSeconds: number): Promise<RateLimitResult> {
  try {
    const redis = getRedis()
    const k = `talaria:rl:${key}`
    const count = await redis.incr(k)
    if (count === 1) await redis.expire(k, windowSeconds)
    if (count <= limit) return { ok: true, retryAfter: 0 }
    const ttl = await redis.ttl(k)
    return { ok: false, retryAfter: ttl > 0 ? ttl : windowSeconds }
  } catch {
    return { ok: true, retryAfter: 0 }
  }
}

/** Drop the counter for `key` (a successful login shouldn't leave the failure
 *  budget spent). */
export async function rateLimitReset(key: string): Promise<void> {
  await getRedis()
    .del(`talaria:rl:${key}`)
    .catch(() => {})
}

/**
 * The client's address, for limiting purposes only.
 *
 * X-Forwarded-For is caller-supplied and trivially rotated, so trusting it
 * blindly turns a per-IP limit into no limit at all — which is exactly what the
 * previous implementation did. It is only honored when the operator states that
 * a proxy really is in front (TALARIA_TRUST_PROXY), because only then is the
 * left-most entry something the client can't freely invent.
 *
 * With no trusted proxy this returns a constant, and callers should lean on the
 * per-identity limit instead. That is deliberate: a wrong IP is worse than no
 * IP, because it silently partitions the counter per attacker-chosen value.
 */
export function clientIp(request: Request): string {
  const trust = (process.env.TALARIA_TRUST_PROXY ?? '').trim().toLowerCase()
  if (trust === '' || trust === '0' || trust === 'false' || trust === 'no') return 'direct'
  const xff = request.headers.get('x-forwarded-for')
  const first = xff?.split(',')[0]?.trim()
  return first || request.headers.get('x-real-ip')?.trim() || 'direct'
}
