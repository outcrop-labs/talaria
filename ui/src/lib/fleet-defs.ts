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
  /** Human-readable job title (e.g. "Support Lead"); editable, shown on the roster. */
  role: string | null
  enabled: boolean
  managed: boolean
  source: 'imported' | 'created'
  currentVersion: number
  latest: AgentVersion | null
}

export interface LlmEndpoint {
  id: string
  name: string
  provider: string
  baseUrl: string | null
  class: 'local' | 'cloud'
  apiKeyEnv?: string | null
  /** Whether an encrypted API key is stored (the value never reaches the client). */
  hasKey?: boolean
  contextLength: number | null
  priceInPerMtok?: number | null
  priceOutPerMtok?: number | null
  models: string[]
  modelPrices?: Record<string, { in?: number; out?: number }>
  /** Auto-fetched $/MTok (public OpenRouter catalog); overrides win. */
  autoPrices?: Record<string, { in: number; out: number }>
  /** Extra request-body defaults merged into gateway/agent calls (e.g. the
   *  OpenRouter no-train provider allowlist). */
  requestDefaults?: Record<string, unknown>
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

export interface ReconcileResult {
  rendered?: number
  started?: string[]
  alreadyRunning?: string[]
  warnings?: string[]
  error?: string
}

/** Render configs + start every enabled managed agent that isn't running. */
export async function reconcileFleet(): Promise<ReconcileResult> {
  const r = await fetch('/api/fleet/reconcile', { method: 'POST', credentials: 'same-origin' })
  return (await r.json().catch(() => ({ error: `reconcile failed (${r.status})` }))) as ReconcileResult
}

export interface ContainerState {
  name: string
  state: string
  status: string
}

export interface AgentContainers {
  department: string
  managed: ContainerState | null
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

export interface AgentEdit {
  soul: string
  main: ModelTarget
  aliases: Array<ModelTarget & { name: string }>
  fallbacks: ModelTarget[]
  note?: string
  apply?: boolean
}

export async function saveAgentEdit(
  id: string,
  edit: AgentEdit,
): Promise<{ ok?: boolean; version?: number; created?: boolean; applied?: boolean; error?: string }> {
  const r = await fetch(`/api/fleet/defs/${id}/edit`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(edit),
  })
  return (await r.json().catch(() => ({ error: `save failed (${r.status})` }))) as { error?: string }
}

export type FleetAction = 'up' | 'stop' | 'retire' | 'unretire'

export async function createFleetAgent(input: {
  slug: string
  department: string
  displayName: string
  role?: string | null
  /** Clone this agent's config; omit for the platform defaults. */
  templateId?: string
  soul?: string
  skills?: Array<{ name: string; content: string }>
  start?: boolean
}): Promise<{ ok?: boolean; healthy?: boolean; error?: string }> {
  const r = await fetch('/api/fleet/create', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(input),
  })
  return (await r.json().catch(() => ({ error: `create failed (${r.status})` }))) as { error?: string }
}

/** Update an agent's editable identity (role, display name). */
export async function patchAgentMeta(id: string, patch: { role?: string | null; displayName?: string }): Promise<{ ok?: boolean; error?: string }> {
  const r = await fetch(`/api/fleet/defs/${id}`, {
    method: 'PATCH',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  }).catch(() => null)
  return (await r?.json().catch(() => ({ error: 'network error' }))) ?? { error: 'network error' }
}

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
