// Org sign-up EMAIL domains — self-joins for federated identities. These are
// the domains after the @ in people's email addresses, wholly independent of
// wherever this Talaria instance is HOSTED (talaria.example.com hosting ≠
// example.com emails). An admin adds an email domain, proves ownership with a
// DNS TXT record, and from then on
// anyone who authenticates through a provider that VERIFIES email (Google)
// with an address on that domain joins automatically as a member. DNS proof
// is required before a domain admits anyone — an admin can't (typo or
// otherwise) claim gmail.com.
import { randomBytes } from 'node:crypto'
import { resolveTxt } from 'node:dns/promises'
import { db } from './db/pg'

export interface OrgDomain {
  id: string
  domain: string
  verified: boolean
  verificationToken: string
  addedBy: string | null
  createdAt: string
  verifiedAt: string | null
}

const ROW = `id, domain, verified, verification_token as "verificationToken", added_by as "addedBy",
  created_at as "createdAt", verified_at as "verifiedAt"`

const normalize = (d: string) => d.trim().toLowerCase().replace(/^https?:\/\//, '').replace(/\/.*$/, '').replace(/^@/, '')
const DOMAIN_RE = /^(?!-)[a-z0-9-]{1,63}(?<!-)(\.(?!-)[a-z0-9-]{1,63}(?<!-))+$/

export async function listOrgDomains(): Promise<OrgDomain[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from org_domains order by domain`)) as unknown as OrgDomain[]
}

export async function addOrgDomain(domain: string, addedBy: string): Promise<OrgDomain> {
  const d = normalize(domain)
  if (!DOMAIN_RE.test(d)) throw new Error('that does not look like a domain')
  const sql = await db()
  const token = `talaria-verify=${randomBytes(18).toString('hex')}`
  const rows = (await sql`
    insert into org_domains (domain, verification_token, added_by) values (${d}, ${token}, ${addedBy})
    on conflict (domain) do update set added_by = excluded.added_by
    returning ${sql.unsafe(ROW)}
  `) as unknown as OrgDomain[]
  return rows[0]!
}

export async function removeOrgDomain(id: string): Promise<void> {
  const sql = await db()
  await sql`delete from org_domains where id = ${id}`
}

/** Check DNS for the verification TXT record. Canonical placement is
 *  `_talaria-verify.<domain>` — deliberately NOT `talaria.<domain>`-shaped,
 *  since many orgs HOST Talaria on a subdomain like talaria.example.com and
 *  the email domain being verified here is a separate concern entirely.
 *  Legacy/root placements still pass. */
export async function verifyOrgDomain(id: string): Promise<{ verified: boolean; error?: string }> {
  const sql = await db()
  const [row] = (await sql.unsafe(`select ${ROW} from org_domains where id = $1`, [id])) as unknown as OrgDomain[]
  if (!row) return { verified: false, error: 'not found' }
  const hosts = [`_talaria-verify.${row.domain}`, `_talaria.${row.domain}`, row.domain]
  for (const host of hosts) {
    const records = await resolveTxt(host).catch(() => [] as string[][])
    if (records.some((chunks) => chunks.join('').trim() === row.verificationToken)) {
      await sql`update org_domains set verified = true, verified_at = now() where id = ${id}`
      return { verified: true }
    }
  }
  return {
    verified: false,
    error: `TXT record not found. Add "${row.verificationToken}" as a TXT record on _talaria-verify.${row.domain} (or on ${row.domain} itself) and try again`,
  }
}

/** Self-join gate: a provider-verified email on a VERIFIED org domain may
 *  create an account. Exact-domain match only — subdomains are added
 *  individually, on purpose. */
export async function selfJoinAllowed(email: string | null | undefined): Promise<boolean> {
  if (!email) return false
  const domain = email.toLowerCase().split('@')[1]
  if (!domain) return false
  const sql = await db()
  const rows = await sql`select 1 as ok from org_domains where domain = ${domain} and verified`
  return rows.length > 0
}
