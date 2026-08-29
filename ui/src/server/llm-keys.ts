// Per-user API keys for the Talaria LLM gateway. The secret is `tlk_<hex>`,
// shown exactly once at mint time; only its sha256 is stored. Admins may
// always mint; other users need the can_mint_keys grant (admin console).
import { createHash, randomBytes } from 'node:crypto'
import { hasPerm } from './permissions'
import { db } from './db/pg'

export interface LlmApiKey {
  id: string
  name: string
  prefix: string
  createdAt: string
  lastUsedAt: string | null
  revokedAt: string | null
  /** Self-imposed ceilings (#265); null = unlimited. See KeyPolicy. */
  spendCapTokens: number | null
  spendCapUsd: number | null
  rateLimitPerMinute: number | null
}

/** The per-key policy columns, on every read. bigint/numeric come back as
 *  strings from the driver, so each select casts ::float8 — and each select
 *  spells the list out (a shared string interpolated into a tagged sql
 *  template would bind as a parameter; see the ALL_TOKENS note in usage.ts). */


const hash = (secret: string) => createHash('sha256').update(secret).digest('hex')

export async function canMintKeys(userId: string, role: string): Promise<boolean> {
  // Delegated to the fine-grained permission system (the old can_mint_keys
  // column was backfilled into user_permissions as 'models.mint-keys').
  return hasPerm({ id: userId, role: role === 'admin' ? 'admin' : 'member' }, 'models.mint-keys')
}

export async function mintKey(userId: string, name: string): Promise<{ key: LlmApiKey; secret: string }> {
  const secret = `tlk_${randomBytes(24).toString('hex')}`
  const prefix = secret.slice(0, 12)
  const sql = await db()
  const rows = await sql`
    insert into llm_api_keys (user_id, name, key_hash, prefix)
    values (${userId}, ${name}, ${hash(secret)}, ${prefix})
    returning id, name, prefix, created_at as "createdAt", last_used_at as "lastUsedAt", revoked_at as "revokedAt",
      spend_cap_tokens::float8 as "spendCapTokens", spend_cap_usd::float8 as "spendCapUsd", rate_limit_per_minute as "rateLimitPerMinute"
  `
  return { key: rows[0] as unknown as LlmApiKey, secret }
}

export async function listKeys(userId: string): Promise<LlmApiKey[]> {
  const sql = await db()
  return (await sql`
    select id, name, prefix, created_at as "createdAt", last_used_at as "lastUsedAt", revoked_at as "revokedAt",
      spend_cap_tokens::float8 as "spendCapTokens", spend_cap_usd::float8 as "spendCapUsd", rate_limit_per_minute as "rateLimitPerMinute"
    from llm_api_keys where user_id = ${userId} order by created_at desc
  `) as unknown as LlmApiKey[]
}

export async function revokeKey(userId: string, keyId: string): Promise<void> {
  const sql = await db()
  await sql`update llm_api_keys set revoked_at = now() where id = ${keyId} and user_id = ${userId}`
}

/** The self-imposed policy a key owner can set (#265). Every field nullable:
 *  null = unlimited, and 0 means the same (normalized to null on write, so
 *  the row and the API never disagree about which spelling means "off"). */
export interface KeyPolicy {
  spendCapTokens: number | null
  spendCapUsd: number | null
  rateLimitPerMinute: number | null
}

const orNull = (v: number | null | undefined) => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : null)

/** Set a key's policy. Scoped to the owner; false = no such key (theirs). */
export async function setKeyPolicy(userId: string, keyId: string, policy: KeyPolicy): Promise<boolean> {
  const sql = await db()
  const rows = await sql`
    update llm_api_keys set
      spend_cap_tokens = ${orNull(policy.spendCapTokens)},
      spend_cap_usd = ${orNull(policy.spendCapUsd)},
      rate_limit_per_minute = ${orNull(policy.rateLimitPerMinute)}
    where id = ${keyId} and user_id = ${userId} and revoked_at is null
    returning id
  `
  return rows.length > 0
}

export interface KeyIdentity {
  keyId: string
  keyName: string
  userId: string
  email: string | null
  /** The key's own ceilings, read on the same hot-path query (#265). */
  caps: { tokens: number | null; usd: number | null; rpm: number | null }
}

/** Resolve a Bearer secret to its owner. Null = unknown/revoked. */
export async function authenticateKey(bearer: string | null): Promise<KeyIdentity | null> {
  if (!bearer?.startsWith('tlk_')) return null
  const sql = await db()
  const rows = (await sql`
    select k.id, k.name, k.user_id as "userId", u.email,
      k.spend_cap_tokens::float8 as "spendCapTokens",
      k.spend_cap_usd::float8 as "spendCapUsd",
      k.rate_limit_per_minute as "rateLimitPerMinute"
    from llm_api_keys k join users u on u.id = k.user_id
    where k.key_hash = ${hash(bearer)} and k.revoked_at is null
  `) as unknown as Array<{
    id: string
    name: string
    userId: string
    email: string | null
    spendCapTokens: number | null
    spendCapUsd: number | null
    rateLimitPerMinute: number | null
  }>
  const r = rows[0]
  if (!r) return null
  void sql`update llm_api_keys set last_used_at = now() where id = ${r.id}`.catch(() => {})
  return {
    keyId: r.id,
    keyName: r.name,
    userId: r.userId,
    email: r.email,
    caps: { tokens: r.spendCapTokens, usd: r.spendCapUsd, rpm: r.rateLimitPerMinute },
  }
}
