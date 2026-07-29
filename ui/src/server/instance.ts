// The instance's HOSTING domain — where this Talaria deployment lives
// (talaria.example.com), as opposed to the email sign-up domains people
// authenticate with. Verification is a SELF-FETCH: the server requests its
// own identity beacon through the candidate domain and checks
// the instance id that comes back — proof that DNS, routing, and TLS all
// actually land on THIS deployment, not just that someone owns the name.
// Once verified it is the canonical base URL (stable OAuth callbacks, links).
import { randomUUID } from 'node:crypto'
import { getSetting, setSetting } from './audit'

const ID_KEY = 'instance_id'
const DOMAIN_KEY = 'instance_domain'

export interface InstanceDomain {
  domain: string
  verified: boolean
  verifiedAt: string | null
}

/** Stable per-deployment identity, minted on first ask. */
export async function getInstanceId(): Promise<string> {
  const existing = await getSetting<string | null>(ID_KEY, null)
  if (existing) return existing
  const id = randomUUID()
  await setSetting(ID_KEY, id)
  return id
}

export const getInstanceDomain = () => getSetting<InstanceDomain | null>(DOMAIN_KEY, null)

export async function setInstanceDomain(domain: string | null): Promise<InstanceDomain | null> {
  if (domain === null) {
    await setSetting(DOMAIN_KEY, null)
    return null
  }
  const d = domain.trim().toLowerCase().replace(/^https?:\/\//, '').replace(/\/.*$/, '')
  if (!/^(?!-)[a-z0-9-]{1,63}(?<!-)(\.(?!-)[a-z0-9-]{1,63}(?<!-))+(:\d+)?$/.test(d)) {
    throw new Error('that does not look like a domain')
  }
  const next: InstanceDomain = { domain: d, verified: false, verifiedAt: null }
  await setSetting(DOMAIN_KEY, next)
  return next
}

/** Round-trip proof: fetch our own well-known through the domain. */
export async function verifyInstanceDomain(): Promise<{ verified: boolean; error?: string }> {
  const cfg = await getInstanceDomain()
  if (!cfg) return { verified: false, error: 'no domain configured' }
  const id = await getInstanceId()
  for (const scheme of ['https', 'http'] as const) {
    try {
      const r = await fetch(`${scheme}://${cfg.domain}/api/well-known/talaria-instance`, {
        signal: AbortSignal.timeout(8_000),
        redirect: 'follow',
      })
      if (!r.ok) continue
      const j = (await r.json()) as { instance?: string }
      if (j.instance === id) {
        await setSetting(DOMAIN_KEY, { ...cfg, verified: true, verifiedAt: new Date().toISOString() } satisfies InstanceDomain)
        return { verified: true }
      }
      return { verified: false, error: `${cfg.domain} answers, but as a DIFFERENT Talaria instance — check your DNS/proxy target` }
    } catch {
      /* try the next scheme */
    }
  }
  return { verified: false, error: `${cfg.domain} is not reachable from this server (or does not serve Talaria yet)` }
}

/** Canonical base URL when a verified hosting domain exists; else null (the
 *  caller falls back to the request origin). */
export async function instanceBaseUrl(): Promise<string | null> {
  const cfg = await getInstanceDomain()
  if (!cfg?.verified) return null
  return `https://${cfg.domain}`
}
