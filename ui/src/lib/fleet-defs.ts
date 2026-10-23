// Harness registry client (admin): agent definitions + LLM endpoints.
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'
import { createQuery } from '@tanstack/svelte-query'
import { errorMessage, getJson, getList, patchJson, postJson, putJson } from '@/lib/fetch-json'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */

export interface ModelTarget {
  endpoint: string
  model: string
  contextLength?: number
  /** Default reasoning effort for this target's model, when it publishes
   *  levels — set in the agent editor beside the model pick. */
  effort?: string | null
}

/** When Hermes compacts a pinned session. Both values are clearable, and a
 *  cleared one is `null` — "use Hermes' own default" rather than zero, which
 *  would mean "compact constantly".
 *
 *  `threshold` is a RATIO of the routed model's context window; Hermes floors
 *  anything under 0.75 at 0.75 for models with less than 512K of context, so a
 *  small number here is not honored everywhere. `thresholdTokens` is an
 *  absolute ceiling, and whichever of the two is reached first fires. */
/** The compaction knobs' bounds, the client's one spelling of the route's
 *  (`fleet_defs_id_edit.rs`): the guards, the inputs and the copy read these,
 *  so a change on the api side moves the whole form at once. */
export const COMPACTION_BOUNDS = {
  threshold: { min: 0.1, max: 1, step: 0.05 },
  tokens: { min: 1_000, max: 10_000_000, step: 1_000 },
} as const

export interface AgentCompression {
  threshold: number | null
  thresholdTokens: number | null
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
   *  (org+slug@domain) — see the aliasing derivation (api/src/google/org.rs). */
  emailAlias: string | null
  /** The human a PERSONAL assistant belongs to (null = an org agent). */
  ownerUserId: string | null
  /** Who OWNS this agent: the only people (admins aside) who may change it. */
  managers: AgentManager[]
  /** Whether THIS viewer is one of them — every per-agent affordance reads
   *  this, and the matching 403 is what the server answers if it doesn't. */
  canManage: boolean
  enabled: boolean
  managed: boolean
  source: 'imported' | 'created'
  /** The Developer Agent switch: dev sandbox + Oh My Pi + the Workbench tools. */
  developer: boolean
  /** How many subagents this agent may run at once (0 = it delegates to
   *  nobody) — the cap the fleet render hands Hermes. */
  maxChildren: number
  /** Pinned sessions: one Hermes session per conversation, Hermes holding the
   *  history, so its compaction and session search cover the whole thread. */
  pinnedSessions: boolean
  /** Hermes toolsets this agent does NOT get. Empty for most agents; `browser`
   *  here keeps an agent that should not browse off the web. `todo` and
   *  `clarify` are off for every agent whatever this says — Talaria's own
   *  versions are the ones a person sees. */
  disabledToolsets: string[]
  /** Template overrides — this agent always formats tickets/plans on these. */
  ticketTemplateId: string | null
  planTemplateId: string | null
  currentVersion: number
  /** The latest version's `raw.compression`, projected into camelCase by the
   *  roster read — a def-level field rather than a member of `latest.config`,
   *  because the wire's `config` is the stored jsonb verbatim and this is a
   *  derived view of one block inside it. Null (or absent, on an older api)
   *  means the def says nothing and Hermes' own defaults are in force. */
  compression?: AgentCompression | null
  latest: AgentVersion | null
}

/** One of an agent's managers. `role` is their org role, so the roster can
 *  mark the admins (who hold reach over every agent regardless). */
export interface AgentManager {
  userId: string
  email: string | null
  name: string | null
  role: string
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
  /** Published provider rates. Read-only. Not a spend source when the provider reports a charge. */
  providerPrices?: Array<{
    model: string
    variant: string
    in: number | null
    out: number | null
    source: string
    fetchedAt: string
  }>
  /** Fetched catalog rates. Not a spend source. */
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

/** The roster read. `defs` is what this viewer may see — the whole fleet for
 *  `agents.manage` and admins, otherwise the agents they manage — and
 *  `canHire` says which it was, so an empty list can tell "no agents yet"
 *  apart from "not yours to see". */
export interface FleetDefs {
  defs: AgentDef[]
  endpoints: LlmEndpoint[]
  brains?: AgentBrainHealth[]
  canHire: boolean
  /** The toolset vocabulary, published by the server rather than restated
   *  here: `governed` is every Hermes toolset Talaria will accept in a def's
   *  `disabledToolsets` (alphabetical), and `mandatoryDisabled` is the WHOLE
   *  floor the renderer merges on top of any agent's list — off for every
   *  agent whatever a def says: Talaria's own pair (its versions of those
   *  tools are the ones a person sees) plus whatever an admin closed
   *  org-wide. `orgDisabled` is that org part alone, so the grid can say an
   *  admin locked a row rather than Talaria; absent on an older api, which
   *  publishes the pair and nothing else. Optional so a UI built against an
   *  older api simply renders no toolset grid instead of an empty one. */
  toolsets?: { governed: string[]; mandatoryDisabled: string[]; orgDisabled?: string[] }
}

export function useFleetDefs(enabled: MaybeGetter<boolean>) {
  return createQuery(() => ({
    queryKey: ['fleet-defs'],
    enabled: resolve(enabled),
    queryFn: (): Promise<FleetDefs> => getJson<FleetDefs>('/api/fleet/defs'),
  }))
}

/** One agent's manager roster. Read through the def on the roster; this is
 *  the modal's own read, so a change shows without a full roster refetch. */
export function useAgentManagers(id: MaybeGetter<string>) {
  return createQuery(() => ({
    queryKey: ['agent-managers', resolve(id)],
    queryFn: (): Promise<AgentManager[]> =>
      getList<AgentManager>(`/api/fleet/defs/${resolve(id)}/managers`, 'managers'),
  }))
}

/** Replace an agent's manager roster. The set is never empty — the server
 *  refuses that, because a manager who cleared it would lock themselves out
 *  of their own agent. */
export async function setAgentManagers(id: string, userIds: string[]): Promise<AgentManager[]> {
  const body = await putJson<{ managers: AgentManager[] }>(`/api/fleet/defs/${id}/managers`, { userIds })
  return body.managers
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

// ── Rolling the whole fleet ──────────────────────────────────────────────────

/** One agent's place in the roll picture (`GET /api/fleet/roll`). `idle` is
 *  the word for "no roll on record": the registry keeps outcomes for an hour,
 *  so `done` and `failed` are recent history, not the steady state. */
export type FleetRollState = 'rolling' | 'idle' | 'failed' | 'done'

export interface FleetRollRow {
  slug: string
  department: string
  displayName: string
  state: FleetRollState
  /** Epoch ms the roll started (or was queued); null when idle. */
  since: number | null
  /** The failure sentence, or a progress note ("queued") while rolling. */
  detail: string | null
  batch: string | null
}

export interface FleetRollPicture {
  ok: boolean
  /** Whether any department is mid-roll right now. */
  rolling: boolean
  /** The most recent batch on record, if any. */
  batch: string | null
  rolls: FleetRollRow[]
  /** False when the other instances' half of the picture (Redis) could not
   *  be read — this instance's rolls are still exact. */
  crossInstance: boolean
}

/** The live roll picture, admin only. Polls every five seconds while anything
 *  rolls; slower otherwise, so a roll somebody else started (one agent's
 *  manager, another admin) still shows up without a reload. */
export function useFleetRoll(enabled: MaybeGetter<boolean>) {
  return createQuery(() => ({
    queryKey: ['fleet-roll'],
    enabled: resolve(enabled),
    refetchInterval: (query: { state: { data?: FleetRollPicture } }) => (query.state.data?.rolling ? 5_000 : 30_000),
    queryFn: (): Promise<FleetRollPicture> => getJson<FleetRollPicture>('/api/fleet/roll'),
  }))
}

export interface FleetRollStart {
  ok?: boolean
  /** Slugs queued to roll, in roster order. */
  rolling?: string[]
  /** Slugs left alone, each with the one-phrase reason. */
  skipped?: { slug: string; why: string }[]
  batch?: string
  error?: string
}

/** Queue an in-place roll of every running managed agent (`all`: every
 *  managed enabled agent, stopped ones included). Answers as soon as the
 *  batch is decided; `useFleetRoll` carries the progress. Failures resolve
 *  as `{ error }` so the strip can render the sentence in place. */
export async function rollFleet(all = false): Promise<FleetRollStart> {
  try {
    return await postJson<FleetRollStart>('/api/fleet/roll', { all })
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
  /** Compaction, three-valued per field and that is the point: ABSENT leaves
   *  the def's current value alone (the soul-only publish must not silently
   *  reset it), a number writes it, and `null` CLEARS the key so Hermes'
   *  default stands. Writes `raw.compression.threshold` /
   *  `raw.compression.threshold_tokens`; every other raw key is kept. */
  compression?: { threshold?: number | null; thresholdTokens?: number | null }
}

export async function saveAgentEdit(
  id: string,
  edit: AgentEdit,
): Promise<{
  ok?: boolean
  version?: number
  created?: boolean
  applied?: boolean
  /** The route's own remarks about what it did with the edit — e.g. a
   *  threshold Hermes will floor on this model, or a change saved without a
   *  roll. Optional: an api that reports none simply shows nothing. */
  notes?: string[]
  error?: string
}> {
  try {
    return await postJson<{ ok?: boolean; version?: number; created?: boolean; applied?: boolean; notes?: string[] }>(
      `/api/fleet/defs/${id}/edit`,
      edit,
    )
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

export type FleetAction = 'up' | 'stop' | 'restart' | 'roll' | 'retire' | 'unretire' | 'delete'

/** What the roster shows while a hire works — the run's own phase sentences
 *  (`rendering the fleet config`, `starting the container`), never a
 *  percentage: a boot has no honest number to show. */
export interface AgentHire {
  id: string
  name: string
  slug: string
  department: string
  start: boolean
  state: 'queued' | 'running' | 'done' | 'error' | 'cancelled'
  phase: string
  error: string | null
  createdAt: string
}

export async function hireFleetAgent(input: {
  slug: string
  department: string
  displayName: string
  role?: string | null
  /** Clone this agent's config; omit for the platform defaults. */
  templateId?: string
  soul?: string
  skills?: Array<{ name: string; content: string }>
  start?: boolean
}): Promise<{ ok?: boolean; hire?: { id: string; state: string; phase: string }; error?: string }> {
  try {
    return await postJson<{ ok?: boolean; hire?: { id: string; state: string; phase: string } }>('/api/fleet/create', input)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

/** Live + recently-landed hires. Polls hard while anything is in flight and
 *  goes quiet the moment nothing is — the query the roster's hiring rows and
 *  the "the new agent has landed" invalidation both hang off. */
export function useFleetHires(enabled: MaybeGetter<boolean>) {
  return createQuery(() => ({
    queryKey: ['fleet-hires'],
    enabled: resolve(enabled),
    refetchInterval: (query: { state: { data?: { hires: AgentHire[] } } }) =>
      (query.state.data?.hires ?? []).some((h) => h.state === 'queued' || h.state === 'running') ? 2_500 : false,
    queryFn: (): Promise<{ hires: AgentHire[] }> => getJson<{ hires: AgentHire[] }>('/api/fleet/hires'),
  }))
}

/** Update an agent's editable identity (role, display name, template bindings). */
export async function patchAgentMeta(
  id: string,
  patch: { role?: string | null; displayName?: string; emailAlias?: string | null; ticketTemplateId?: string | null; planTemplateId?: string | null; developer?: boolean; pinnedSessions?: boolean; disabledToolsets?: string[]; maxChildren?: number },
): Promise<{ ok?: boolean; error?: string }> {
  try {
    return await patchJson<{ ok: boolean }>(`/api/fleet/defs/${id}`, patch)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

/** Revoke this agent's `tak_` credential and mint a fresh one (its managers,
 *  and admins). The old secret stops authenticating at once; the fleet is
 *  re-rendered and, for a managed agent, a roll is queued so the running
 *  container is recreated on the new key — `rolling` says whether one was.
 *  The new secret is never in the answer. Failures resolve as `{ error }`
 *  with the server's sentence, so the modal can show it in place. */
export async function rotateAgentKey(id: string): Promise<{ ok?: boolean; rolling?: boolean; error?: string }> {
  try {
    return await postJson<{ ok: boolean; rolling: boolean }>(`/api/fleet/defs/${id}/rotate-key`)
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
