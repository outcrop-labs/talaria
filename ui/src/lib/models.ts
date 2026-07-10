// Models tab client: provider presets + endpoint CRUD.
import { useQuery } from '@tanstack/react-query'
import type { LlmEndpoint } from '@/lib/fleet-defs'

/** Every common US model provider, preconfigured: base URLs and provider
 *  wiring are baked in so adding one is pick → name the key env → done. NO
 *  model lists live here — a new endpoint starts empty and models are always
 *  picked from the provider's LIVE catalog, so nothing here can go stale. */
export const PROVIDER_PRESETS: Array<{
  key: string
  label: string
  provider: string
  class: 'local' | 'cloud'
  baseUrl?: string
  apiKeyEnv?: string
  /** Show the base-URL field (local/custom endpoints only). */
  configurableUrl?: boolean
}> = [
  {
    key: 'anthropic',
    label: 'Anthropic',
    provider: 'anthropic',
    class: 'cloud',
    apiKeyEnv: 'ANTHROPIC_API_KEY',
  },
  {
    key: 'openai',
    label: 'OpenAI',
    provider: 'openai',
    class: 'cloud',
    apiKeyEnv: 'OPENAI_API_KEY',
  },
  {
    key: 'google',
    label: 'Google (Gemini)',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai',
    apiKeyEnv: 'GEMINI_API_KEY',
  },
  {
    key: 'x-ai',
    label: 'xAI (Grok)',
    provider: 'x-ai',
    class: 'cloud',
    apiKeyEnv: 'XAI_API_KEY',
  },
  {
    key: 'meta',
    label: 'Meta (Llama API)',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://api.llama.com/compat/v1',
    apiKeyEnv: 'LLAMA_API_KEY',
  },
  {
    key: 'openrouter',
    label: 'OpenRouter',
    provider: 'openrouter',
    class: 'cloud',
    apiKeyEnv: 'OPENROUTER_API_KEY',
  },
  {
    key: 'groq',
    label: 'Groq',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://api.groq.com/openai/v1',
    apiKeyEnv: 'GROQ_API_KEY',
  },
  {
    key: 'together',
    label: 'Together AI',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://api.together.xyz/v1',
    apiKeyEnv: 'TOGETHER_API_KEY',
  },
  {
    key: 'fireworks',
    label: 'Fireworks AI',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://api.fireworks.ai/inference/v1',
    apiKeyEnv: 'FIREWORKS_API_KEY',
  },
  {
    key: 'cerebras',
    label: 'Cerebras',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://api.cerebras.ai/v1',
    apiKeyEnv: 'CEREBRAS_API_KEY',
  },
  {
    key: 'perplexity',
    label: 'Perplexity',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://api.perplexity.ai',
    apiKeyEnv: 'PERPLEXITY_API_KEY',
  },
  {
    key: 'deepinfra',
    label: 'DeepInfra',
    provider: 'custom',
    class: 'cloud',
    baseUrl: 'https://api.deepinfra.com/v1/openai',
    apiKeyEnv: 'DEEPINFRA_API_KEY',
  },
  {
    key: 'deepseek',
    label: 'DeepSeek',
    provider: 'deepseek',
    class: 'cloud',
    apiKeyEnv: 'DEEPSEEK_API_KEY',
  },
  {
    key: 'ollama',
    label: 'Ollama (self-hosted)',
    provider: 'custom',
    class: 'local',
    baseUrl: 'http://localhost:11434/v1',
    configurableUrl: true,
  },
  {
    key: 'vllm',
    label: 'vLLM (self-hosted)',
    provider: 'custom',
    class: 'local',
    baseUrl: 'http://localhost:8000/v1',
    apiKeyEnv: 'LLM_API_KEY',
    configurableUrl: true,
  },
  { key: 'custom', label: 'Custom (OpenAI-compatible)', provider: 'custom', class: 'cloud', configurableUrl: true },
]

/** Class inference: users never pick local/cloud — LAN/loopback hosts are
 *  local, everything else (incl. keyless native providers) is cloud. */
export function inferClass(baseUrl?: string | null): 'local' | 'cloud' {
  if (!baseUrl) return 'cloud'
  const host = baseUrl.replace(/^https?:\/\//, '').split(/[/:]/)[0] ?? ''
  return /^(localhost|127\.|10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host) || !host.includes('.')
    ? 'local'
    : 'cloud'
}

/** The provider's live catalog — what it offers right now (server-side fetch
 *  on every open; `note` explains an empty result, e.g. missing key). */
export function useAvailableModels(endpointId: string) {
  return useQuery({
    queryKey: ['available-models', endpointId],
    staleTime: 60_000,
    queryFn: async (): Promise<{ models: string[]; note?: string }> => {
      const r = await fetch(`/api/fleet/endpoints/${endpointId}/available`, { credentials: 'same-origin' })
      if (!r.ok) return { models: [] }
      return r.json()
    },
  })
}

export function useEndpoints(enabled = true) {
  return useQuery({
    queryKey: ['fleet-endpoints'],
    enabled,
    queryFn: async (): Promise<LlmEndpoint[]> => {
      const r = await fetch('/api/fleet/endpoints', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { endpoints: LlmEndpoint[] }).endpoints
    },
  })
}

export interface AffectedAgent {
  slug: string
  asMain: boolean
  aliases: string[]
  fallbacks: number
}

export interface EndpointOpResult {
  ok?: boolean
  /** The created endpoint's id (POST /api/fleet/endpoints). */
  id?: string
  error?: string
  needsForce?: boolean
  affected?: AffectedAgent[]
  cascaded?: string[]
}

const j = async (r: Response): Promise<EndpointOpResult> =>
  (await r.json().catch(() => ({ error: `request failed (${r.status})` }))) as EndpointOpResult

// fetch() itself rejects on network failure (server restarting, offline) —
// surface that as a normal error instead of an unhandled rejection.
const netErr = (): EndpointOpResult => ({ error: 'network error. Is the server up?' })

export const addEndpoint = (e: {
  name: string
  provider: string
  baseUrl?: string | null
  class: 'local' | 'cloud'
  apiKeyEnv?: string | null
  /** Raw provider key — sealed (encrypted) server-side. */
  apiKey?: string | null
}) =>
  fetch('/api/fleet/endpoints', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(e),
  })
    .then(j)
    .catch(netErr)

export const patchEndpoint = (
  id: string,
  patch: {
    class?: 'local' | 'cloud'
    priceInPerMtok?: number | null
    priceOutPerMtok?: number | null
    models?: string[]
    modelPrices?: Record<string, { in?: number; out?: number }>
    requestDefaults?: Record<string, unknown>
    /** Raw provider key — sealed server-side. '' clears it; omitted leaves it. */
    apiKey?: string | null
    force?: boolean
  },
) =>
  fetch(`/api/fleet/endpoints/${id}`, {
    method: 'PUT',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  })
    .then(j)
    .catch(netErr)

export const removeEndpoint = (id: string, force = false) =>
  fetch(`/api/fleet/endpoints/${id}${force ? '?force=1' : ''}`, { method: 'DELETE', credentials: 'same-origin' })
    .then(j)
    .catch(netErr)
