// Harness registry client (admin): agent definitions + LLM endpoints.
import { createQuery } from '@tanstack/svelte-query'
import { errorMessage, getJson, getList, patchJson, postJson } from '@/lib/fetch-json'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export interface ModelTarget {
  endpoint: string
  model: string
  contextLength?: number
  /** Default reasoning effort for this target's model, when it publishes
   *  levels — set in the agent editor beside the model pick. */
  effort?: string | null
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
  /** Send-address override for org agents: null = the derived org plus-address
   *  (org+slug@domain) — see server/google/aliasing.ts. */
  emailAlias: string | null
  /** The human a PERSONAL assistant belongs to (null = an org agent). */
  ownerUserId: string | null
  enabled: boolean
  managed: boolean
  source: 'imported' | 'created'
  /** THE workbench setting: off | auto (fit rules) | on (forced). */
  workbench: 'off' | 'auto' | 'on'
  workbenchProfile: string | null
  workbenchHarness: string | null
  workbenchModels: Partial<Record<'light' | 'standard' | 'heavy', string>>
  /** Template overrides — this agent always formats tickets/plans on these. */
  ticketTemplateId: string | null
  planTemplateId: string | null
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
  /** Admin-declared reasoning-effort ladders, for models whose catalog
   *  publishes none — the endpoint modal's effort editor writes here. */
  modelEfforts?: Record<string, string[]>
  /** Auto-fetched $/MTok (public OpenRouter catalog); overrides win. */
  autoPrices?: Record<string, { in: number; out: number }>
  /** Extra request-body defaults merged into gateway/agent calls (e.g. the
   *  OpenRouter no-train provider allowlist). */
  requestDefaults?: Record<string, unknown>
}

export interface BrainTarget {
  kind: 'main' | 'tier' | 'fallback'
  name?: string
  endpoint: string
  model: string
  ok: boolean
  reason?: string
}

export interface AgentBrainHealth {
  agent: string
  displayName: string
  ok: boolean
  targets: BrainTarget[]
}

export function useFleetDefs(enabled: MaybeGetter<boolean>) {
  return createQuery(() => ({
    queryKey: ['fleet-defs'],
    enabled: resolve(enabled),
    queryFn: (): Promise<{ defs: AgentDef[]; endpoints: LlmEndpoint[]; brains?: AgentBrainHealth[] }> =>
      getJson<{ defs: AgentDef[]; endpoints: LlmEndpoint[]; brains?: AgentBrainHealth[] }>('/api/fleet/defs'),
  }))
}

export interface ReconcileResult {
  rendered?: number
  started?: string[]
  alreadyRunning?: string[]
  warnings?: string[]
  error?: string
}

/** Render configs + start every enabled managed agent that isn't running.
 *  Callers render the answer in-band (`r.error ?? 'Started …'`), so failures
 *  resolve as `{ error }` with the server's sentence rather than rejecting. */
export async function reconcileFleet(): Promise<ReconcileResult> {
  try {
    return await postJson<ReconcileResult>('/api/fleet/reconcile')
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

export interface ContainerState {
  name: string
  state: string
  status: string
  /** Healthcheck phase — 'starting' is the warm-up window after an up/roll. */
  health: 'starting' | 'healthy' | 'unhealthy' | null
}

export interface AgentContainers {
  department: string
  managed: ContainerState | null
}

export function useFleetContainers(enabled: MaybeGetter<boolean>) {
  return createQuery(() => ({
    queryKey: ['fleet-containers'],
    enabled: resolve(enabled),
    refetchInterval: 10_000,
    // "No containers" and "could not ask Docker" look identical on the roster
    // otherwise — every agent silently reads as stopped.
    queryFn: (): Promise<AgentContainers[]> => getList<AgentContainers>('/api/fleet/containers', 'containers'),
  }))
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
  try {
    return await postJson<{ ok?: boolean; version?: number; created?: boolean; applied?: boolean }>(`/api/fleet/defs/${id}/edit`, edit)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

export type FleetAction = 'up' | 'stop' | 'restart' | 'roll' | 'retire' | 'unretire' | 'delete'

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
  try {
    return await postJson<{ ok?: boolean; healthy?: boolean }>('/api/fleet/create', input)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

/** Update an agent's editable identity (role, display name, template bindings). */
export async function patchAgentMeta(
  id: string,
  patch: { role?: string | null; displayName?: string; emailAlias?: string | null; ticketTemplateId?: string | null; planTemplateId?: string | null; workbench?: 'off' | 'auto' | 'on'; workbenchProfile?: string | null; workbenchHarness?: string | null; workbenchModels?: Partial<Record<'light' | 'standard' | 'heavy', string | null>> },
): Promise<{ ok?: boolean; error?: string }> {
  try {
    return await patchJson<{ ok: boolean }>(`/api/fleet/defs/${id}`, patch)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

export async function controlAgent(id: string, action: FleetAction): Promise<{ ok?: boolean; healthy?: boolean; error?: string }> {
  try {
    return await postJson<{ ok?: boolean; healthy?: boolean; warming?: boolean }>(`/api/fleet/agents/${id}/control`, { action })
  } catch (e) {
    return { error: errorMessage(e) }
  }
}
