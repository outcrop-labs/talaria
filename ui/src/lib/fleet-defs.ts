// Harness registry client (admin): agent definitions + LLM endpoints.
import { useQuery } from '@tanstack/react-query'

export interface ModelTarget {
  endpoint: string
  model: string
  contextLength?: number
}

export interface AgentConfig {
  main?: ModelTarget
  aliases?: Array<ModelTarget & { name: string }>
  fallbacks?: ModelTarget[]
  plugins?: string[]
  mcpServers?: string[]
}

export interface AgentVersion {
  id: string
  version: number
  soul: string
  config: AgentConfig
  note: string | null
  createdBy: string | null
  createdAt: string
}

export interface AgentDef {
  id: string
  slug: string
  department: string
  model: string
  displayName: string
  enabled: boolean
  managed: boolean
  currentVersion: number
  latest: AgentVersion | null
}

export interface LlmEndpoint {
  id: string
  name: string
  provider: string
  baseUrl: string | null
  class: 'local' | 'cloud'
  contextLength: number | null
}

export function useFleetDefs(enabled: boolean) {
  return useQuery({
    queryKey: ['fleet-defs'],
    enabled,
    queryFn: async (): Promise<{ defs: AgentDef[]; endpoints: LlmEndpoint[] } | null> => {
      const r = await fetch('/api/fleet/defs', { credentials: 'same-origin' })
      if (!r.ok) return null
      return r.json()
    },
  })
}

export async function importFleet(): Promise<{
  agents: Array<{ slug: string; model: string; version: number; created: boolean }>
  errors: string[]
} | null> {
  const r = await fetch('/api/fleet/defs', { method: 'POST', credentials: 'same-origin' })
  if (!r.ok) return null
  return ((await r.json()) as { result: { agents: []; errors: [] } }).result
}
