// Live provider catalogs: ask each backend what models it actually offers
// (GET /models on OpenAI-compatible APIs; provider-specific hosts for the
// native ones). Keys resolve from the server env or the fleet .env — values
// never leave the server.
import { readFile } from 'node:fs/promises'
import type { LlmEndpoint } from './agent-defs'
import { db } from './db/pg'
import { open, seal } from './secretbox'
import { FLEET_ENV } from './fleet-render'

/** Env names the catalog fetch may read. Only provider-key-shaped vars — an
 *  endpoint pairs an admin-chosen env name with an admin-chosen base URL, so
 *  without this gate an admin could exfiltrate ANY env value (GH_TOKEN,
 *  HERMES_KEY_*, LITELLM_MASTER_KEY) to an arbitrary host. Cross-provider
 *  key-to-wrong-host remains possible for *_API_KEY names — that's the same
 *  residual class as agent-config rendering and is part of the admin trust
 *  model; everything else is refused here. */
export const KEY_ENV_RE = /^(LLM_API_KEY|[A-Z][A-Z0-9_]*_API_KEY)$/

export async function resolveKey(envVar: string | null | undefined): Promise<string | null> {
  if (!envVar || !KEY_ENV_RE.test(envVar)) return null
  if (process.env[envVar]) return process.env[envVar]!.trim()
  const env = await readFile(FLEET_ENV(), 'utf8').catch(() => '')
  const m = new RegExp(`^${envVar}=(.*)$`, 'm').exec(env)
  return m ? m[1]!.trim() : null
}

/** The provider's API key: the sealed key stored in the DB is the source of
 *  truth (encrypted at rest, entered via /models — never in a config file). An
 *  env var / fleet .env of the endpoint's api_key_env is a fallback for ops
 *  overrides and pre-migration deployments. */
export async function resolveEndpointKey(ep: Pick<LlmEndpoint, 'id' | 'apiKeyEnv' | 'provider'>): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select api_key_cipher from llm_endpoints where id = ${ep.id}`) as unknown as Array<{
    api_key_cipher: string | null
  }>
  const cipher = rows[0]?.api_key_cipher
  if (cipher) {
    try {
      return open(cipher)
    } catch {
      /* tampered/old key material — fall back to env */
    }
  }
  return resolveKey(ep.apiKeyEnv ?? DEFAULT_KEY_ENV[ep.provider] ?? null)
}

/** One-time migration: seal any provider key that still lives only in the env /
 *  fleet .env into the DB, so keys stop depending on config files. Idempotent —
 *  only touches endpoints with no sealed key yet. Safe to call on every boot. */
export async function migrateEnvKeysToCipher(): Promise<number> {
  const sql = await db()
  const eps = (await sql`
    select id, provider, api_key_env as "apiKeyEnv" from llm_endpoints where api_key_cipher is null
  `) as unknown as Array<{ id: string; provider: string; apiKeyEnv: string | null }>
  let sealed = 0
  for (const ep of eps) {
    const key = await resolveKey(ep.apiKeyEnv ?? DEFAULT_KEY_ENV[ep.provider] ?? null)
    if (!key) continue
    await sql`update llm_endpoints set api_key_cipher = ${seal(key)}, updated_at = now() where id = ${ep.id}`
    sealed++
  }
  return sealed
}

export const NATIVE_BASE: Record<string, string> = {
  anthropic: 'https://api.anthropic.com/v1',
  openai: 'https://api.openai.com/v1',
  openrouter: 'https://openrouter.ai/api/v1',
  deepseek: 'https://api.deepseek.com/v1',
  'x-ai': 'https://api.x.ai/v1',
}

// Endpoints imported from configs often omit api_key (the provider reads its
// ambient env var) — assume the conventional name per provider.
export const DEFAULT_KEY_ENV: Record<string, string> = {
  anthropic: 'ANTHROPIC_API_KEY',
  openai: 'OPENAI_API_KEY',
  openrouter: 'OPENROUTER_API_KEY',
  deepseek: 'DEEPSEEK_API_KEY',
  'x-ai': 'XAI_API_KEY',
}

/** Fetch with a dev-mode fallback: docker-internal hostnames (bare names like
 *  inference-router) don't resolve from the host — retry on localhost with the
 *  same port, where the compose stacks publish their ports. */
async function fetchModels(base: string, headers: Record<string, string>): Promise<Response> {
  try {
    return await fetch(`${base}/models`, { headers, signal: AbortSignal.timeout(10_000) })
  } catch (err) {
    const m = /^(https?):\/\/([^/:]+)(:\d+)?(\/.*)?$/.exec(base)
    const host = m?.[2] ?? ''
    if (m && host && !host.includes('.') && host !== 'localhost') {
      const local = `${m[1]}://localhost${m[3] ?? ''}${m[4] ?? ''}`
      return await fetch(`${local}/models`, { headers, signal: AbortSignal.timeout(10_000) })
    }
    throw err
  }
}

/** The models a provider reports right now. Throws with a human message. */
export async function availableModels(ep: LlmEndpoint): Promise<string[]> {
  const base = (ep.baseUrl ?? NATIVE_BASE[ep.provider])?.replace(/\/$/, '')
  if (!base) throw new Error('no API base known for this provider')
  const keyEnv = ep.apiKeyEnv ?? DEFAULT_KEY_ENV[ep.provider] ?? null
  const key = await resolveEndpointKey(ep)

  const headers: Record<string, string> = {}
  if (ep.provider === 'anthropic') {
    if (!key) throw new Error(`add the API key for this provider (or set ${keyEnv ?? 'ANTHROPIC_API_KEY'})`)
    headers['x-api-key'] = key
    headers['anthropic-version'] = '2023-06-01'
  } else if (key) {
    headers['Authorization'] = `Bearer ${key}`
  }

  const r = await fetchModels(base, headers)
  if (!r.ok) throw new Error(`provider answered ${r.status}${r.status === 401 ? ' — check the API key env' : ''}`)
  const j = (await r.json()) as { data?: Array<{ id?: string }>; models?: Array<{ id?: string; name?: string }> }
  const ids = (j.data ?? j.models ?? [])
    .map((m) => m.id ?? (m as { name?: string }).name ?? '')
    .filter(Boolean)
  return [...new Set(ids)].sort()
}
