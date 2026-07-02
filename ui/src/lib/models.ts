// Models tab client: provider presets + endpoint CRUD.
import { useQuery } from '@tanstack/react-query'
import type { LlmEndpoint } from '@/lib/fleet-defs'

/** Common providers, one click to add. Model lists are starting points — the
 *  catalog is editable per endpoint. */
export const PROVIDER_PRESETS: Array<{
  key: string
  label: string
  provider: string
  class: 'local' | 'cloud'
  baseUrl?: string
  apiKeyEnv?: string
  models: string[]
}> = [
  {
    key: 'anthropic',
    label: 'Anthropic',
    provider: 'anthropic',
    class: 'cloud',
    apiKeyEnv: 'ANTHROPIC_API_KEY',
    models: ['claude-opus-4-8', 'claude-sonnet-4-6', 'claude-haiku-4-5'],
  },
  {
    key: 'openai',
    label: 'OpenAI',
    provider: 'openai',
    class: 'cloud',
    apiKeyEnv: 'OPENAI_API_KEY',
    models: ['gpt-5.4', 'gpt-5.4-mini'],
  },
  {
    key: 'openrouter',
    label: 'OpenRouter',
    provider: 'openrouter',
    class: 'cloud',
    apiKeyEnv: 'OPENROUTER_API_KEY',
    models: ['anthropic/claude-sonnet-4-6', 'deepseek/deepseek-v4-pro', 'meta-llama/llama-4-70b'],
  },
  {
    key: 'deepseek',
    label: 'DeepSeek',
    provider: 'deepseek',
    class: 'cloud',
    apiKeyEnv: 'DEEPSEEK_API_KEY',
    models: ['deepseek-v4-pro', 'deepseek-v4-flash'],
  },
  {
    key: 'ollama',
    label: 'Ollama (local)',
    provider: 'custom',
    class: 'local',
    baseUrl: 'http://localhost:11434/v1',
    models: [],
  },
  {
    key: 'vllm',
    label: 'vLLM (local)',
    provider: 'custom',
    class: 'local',
    baseUrl: 'http://localhost:8000/v1',
    apiKeyEnv: 'LLM_API_KEY',
    models: [],
  },
  { key: 'custom', label: 'Custom (OpenAI-compatible)', provider: 'custom', class: 'cloud', models: [] },
]

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
  error?: string
  needsForce?: boolean
  affected?: AffectedAgent[]
  cascaded?: string[]
}

const j = async (r: Response): Promise<EndpointOpResult> =>
  (await r.json().catch(() => ({ error: `request failed (${r.status})` }))) as EndpointOpResult

export const addEndpoint = (e: {
  name: string
  provider: string
  baseUrl?: string | null
  class: 'local' | 'cloud'
  apiKeyEnv?: string | null
  models?: string[]
}) =>
  fetch('/api/fleet/endpoints', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(e),
  }).then(j)

export const patchEndpoint = (
  id: string,
  patch: {
    class?: 'local' | 'cloud'
    priceInPerMtok?: number | null
    priceOutPerMtok?: number | null
    models?: string[]
    force?: boolean
  },
) =>
  fetch(`/api/fleet/endpoints/${id}`, {
    method: 'PUT',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  }).then(j)

export const removeEndpoint = (id: string, force = false) =>
  fetch(`/api/fleet/endpoints/${id}${force ? '?force=1' : ''}`, { method: 'DELETE', credentials: 'same-origin' }).then(j)
