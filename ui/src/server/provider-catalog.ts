// Live provider catalogs: ask each backend what models it actually offers
// (GET /models on OpenAI-compatible APIs; provider-specific hosts for the
// native ones). Keys resolve from the server env or the stack .env — values
// never leave the server.
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import type { LlmEndpoint } from './agent-defs'
import { STACK_DIR } from './fleet-render'

async function resolveKey(envVar: string | null | undefined): Promise<string | null> {
  if (!envVar) return null
  if (process.env[envVar]) return process.env[envVar]!.trim()
  const env = await readFile(join(STACK_DIR(), '.env'), 'utf8').catch(() => '')
  const m = new RegExp(`^${envVar}=(.*)$`, 'm').exec(env)
  return m ? m[1]!.trim() : null
}

const NATIVE_BASE: Record<string, string> = {
  anthropic: 'https://api.anthropic.com/v1',
  openai: 'https://api.openai.com/v1',
  openrouter: 'https://openrouter.ai/api/v1',
  deepseek: 'https://api.deepseek.com/v1',
  'x-ai': 'https://api.x.ai/v1',
}

// Endpoints imported from configs often omit api_key (the provider reads its
// ambient env var) — assume the conventional name per provider.
const DEFAULT_KEY_ENV: Record<string, string> = {
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
  const key = await resolveKey(keyEnv)

  const headers: Record<string, string> = {}
  if (ep.provider === 'anthropic') {
    if (!key) throw new Error(`set ${keyEnv ?? 'ANTHROPIC_API_KEY'} to browse the catalog`)
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
