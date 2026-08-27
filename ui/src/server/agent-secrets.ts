// Per-agent secrets — env vars an agent needs that don't belong in the shared
// fleet .env (a Figma token for one agent, a vendor key for another). Set in
// the UI, stored encrypted (secretbox), and materialized ONLY at render time
// into fleet/agents/<slug>/secrets.env (0600), which the rendered service
// loads via env_file. Values are write-only through the API: names and
// timestamps list; plaintext never leaves the server.
import { chmod, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { db } from './db/pg'
import { open, seal } from './secretbox'
import { FLEET_DIR } from './fleet-render'

/** Container-env name: UPPER_SNAKE, must not collide with the stamped vars. */
const SECRET_NAME_RE = /^[A-Z][A-Z0-9_]{1,63}$/
const RESERVED = new Set(['API_SERVER_KEY', 'API_SERVER_MODEL_NAME'])

export interface AgentSecretMeta {
  name: string
  updatedBy: string | null
  updatedAt: string
}

export async function listAgentSecrets(agentId: string): Promise<AgentSecretMeta[]> {
  const sql = await db()
  return (await sql`
    select name, updated_by as "updatedBy", updated_at as "updatedAt"
    from agent_secrets where agent_id = ${agentId} order by name
  `) as unknown as AgentSecretMeta[]
}

export async function setAgentSecret(agentId: string, name: string, value: string, actor: string | null): Promise<void> {
  if (!SECRET_NAME_RE.test(name)) throw new Error('secret names are UPPER_SNAKE (2–64 chars, starting with a letter)')
  if (RESERVED.has(name)) throw new Error(`${name} is managed by Talaria`)
  if (!value || value.length > 8192) throw new Error('value required (max 8 KB)')
  if (/[\n\r]/.test(value)) throw new Error('value cannot contain newlines')
  const sql = await db()
  await sql`
    insert into agent_secrets (agent_id, name, value_enc, updated_by)
    values (${agentId}, ${name}, ${seal(value)}, ${actor})
    on conflict (agent_id, name) do update set
      value_enc = excluded.value_enc, updated_by = excluded.updated_by, updated_at = now()
  `
}

export async function deleteAgentSecret(agentId: string, name: string): Promise<void> {
  const sql = await db()
  await sql`delete from agent_secrets where agent_id = ${agentId} and name = ${name}`
}

/** Write (or remove) the agent's secrets env file for the renderer. Returns
 *  whether the service should declare an env_file. */
export async function materializeAgentSecrets(agentId: string, slug: string): Promise<boolean> {
  const sql = await db()
  const rows = (await sql`
    select name, value_enc as "valueEnc" from agent_secrets where agent_id = ${agentId} order by name
  `) as unknown as Array<{ name: string; valueEnc: string }>
  const path = join(FLEET_DIR(), 'agents', slug, 'secrets.env')
  if (rows.length === 0) {
    await rm(path, { force: true })
    return false
  }
  const lines = rows.map((r) => `${r.name}=${open(r.valueEnc)}`)
  await writeFile(path, `# Rendered by Talaria — per-agent secrets. Do not hand-edit; edit in Talaria.\n` + lines.join('\n') + '\n', {
    mode: 0o600,
  })
  await chmod(path, 0o600).catch(() => {})
  return true
}
