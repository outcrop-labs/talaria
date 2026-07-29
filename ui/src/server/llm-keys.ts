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
}

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
    returning id, name, prefix, created_at as "createdAt", last_used_at as "lastUsedAt", revoked_at as "revokedAt"
  `
  return { key: rows[0] as unknown as LlmApiKey, secret }
}

export async function listKeys(userId: string): Promise<LlmApiKey[]> {
  const sql = await db()
  return (await sql`
    select id, name, prefix, created_at as "createdAt", last_used_at as "lastUsedAt", revoked_at as "revokedAt"
    from llm_api_keys where user_id = ${userId} order by created_at desc
  `) as unknown as LlmApiKey[]
}

export async function revokeKey(userId: string, keyId: string): Promise<void> {
  const sql = await db()
  await sql`update llm_api_keys set revoked_at = now() where id = ${keyId} and user_id = ${userId}`
}

export interface KeyIdentity {
  keyId: string
  keyName: string
  userId: string
  email: string | null
}

/** Resolve a Bearer secret to its owner. Null = unknown/revoked. */
export async function authenticateKey(bearer: string | null): Promise<KeyIdentity | null> {
  if (!bearer?.startsWith('tlk_')) return null
  const sql = await db()
  const rows = (await sql`
    select k.id, k.name, k.user_id as "userId", u.email
    from llm_api_keys k join users u on u.id = k.user_id
    where k.key_hash = ${hash(bearer)} and k.revoked_at is null
  `) as unknown as Array<{ id: string; name: string; userId: string; email: string | null }>
  const r = rows[0]
  if (!r) return null
  void sql`update llm_api_keys set last_used_at = now() where id = ${r.id}`.catch(() => {})
  return { keyId: r.id, keyName: r.name, userId: r.userId, email: r.email }
}
