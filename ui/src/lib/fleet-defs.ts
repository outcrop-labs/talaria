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

export interface ContainerState {
  name: string
  state: string
  status: string
}

export interface AgentContainers {
  department: string
  managed: ContainerState | null
  legacy: ContainerState | null
}

export function useFleetContainers(enabled: boolean) {
  return useQuery({
    queryKey: ['fleet-containers'],
    enabled,
    refetchInterval: 10_000,
    queryFn: async (): Promise<AgentContainers[]> => {
      const r = await fetch('/api/fleet/containers', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { containers: AgentContainers[] }).containers
    },
  })
}

export type FleetAction = 'migrate' | 'up' | 'stop' | 'legacy-start' | 'legacy-stop'

export async function controlAgent(id: string, action: FleetAction): Promise<{ ok?: boolean; healthy?: boolean; error?: string }> {
  const r = await fetch(`/api/fleet/agents/${id}/control`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ action }),
  })
  return (await r.json().catch(() => ({ error: `control failed (${r.status})` }))) as {
    ok?: boolean
    healthy?: boolean
    error?: string
  }
}
