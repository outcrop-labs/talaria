// The fitness surface's queries. Split from `fitness.ts` so that module stays
// free of runtime dependencies and can be unit tested — `vitest.config.ts` runs
// in plain node with no Svelte plugin, and `@tanstack/svelte-query` ships
// `.svelte` files it cannot load.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'
import type { DetailPayload, FitnessIndexEntry, HealthSummary, MatrixPayload, ModelRow, ValuePayload } from './fitness'

/** The matrix. Polls while a run is in flight and stops the moment it isn't —
 *  the same shape `useRagAdmin` uses for the reindex. */
export const useModelFitness = () =>
  createQuery(() => ({
    queryKey: ['model-fitness'],
    queryFn: (): Promise<MatrixPayload> => getJson<MatrixPayload>('/api/admin/model-fitness'),
    // Polls while ANY run is in flight — three can be, and watching only the
    // first would freeze the strip the moment that one finished.
    refetchInterval: (q) => (q.state.data?.runs.some((r) => r.state === 'running') ? 3_000 : false),
  }))

/** One model's drill-down. POLLS WHILE THAT MODEL IS BEING TESTED, and stops the
 *  moment it isn't — the same shape the matrix uses. A sweep checkpoints every
 *  case as it lands, so this is the audit trail live rather than a report an
 *  admin waits twenty minutes for. */
export const useFitnessDetail = (model: () => string | null) =>
  createQuery(() => ({
    queryKey: ['model-fitness-detail', model()],
    enabled: model() !== null,
    queryFn: (): Promise<DetailPayload> =>
      getJson<DetailPayload>(`/api/admin/model-fitness?view=detail&model=${encodeURIComponent(model() ?? '')}`),
    refetchInterval: (q) => (q.state.data?.live ? 3_000 : false),
    // Keep the last frame while the next one loads: a list that blanked every
    // three seconds would be unreadable exactly while it is most worth reading.
    placeholderData: (prev: DetailPayload | undefined) => prev,
  }))

/** Price against performance. Its own cache entry and NOT polled: it costs a
 *  telemetry query plus a price lookup per tested model, so paying for it every
 *  three seconds while three sweeps grind through 247 fixtures each would be
 *  three hundred telemetry queries to redraw a table that cannot change until a
 *  run ARCHIVES.
 *
 *  IT REFETCHES ON `version` INSTEAD, and that is the fix for a real bug: a run
 *  that finished on its own left this tab showing the numbers from before it.
 *  Only a POST from the panel invalidated the query, so a sweep an admin
 *  started and then waited out — the normal case — updated the matrix (which
 *  polls) and left cost and value stale beside it, with no way to tell.
 *
 *  The caller passes a signature of the archive (see `valueVersion`). It changes
 *  exactly when a run lands, which is exactly when these numbers move, so the
 *  refetch is automatic and there is still no polling. */
export const useFitnessValue = (enabled: () => boolean = () => true, version: () => string = () => '') =>
  createQuery(() => ({
    queryKey: ['model-fitness-value', version()],
    enabled: enabled(),
    queryFn: (): Promise<ValuePayload> => getJson<ValuePayload>('/api/admin/model-fitness?view=value'),
    // The previous archive's numbers while the new ones load, rather than a
    // skeleton: the table is mostly unchanged and a flash of empty state after
    // every run reads as "it broke", not "it updated".
    placeholderData: (prev: ValuePayload | undefined) => prev,
  }))

/** HARNESS HEALTH — our own fixtures, across every archived model.
 *
 *  Fetched only when the tab is opened, like the value view and for the same
 *  reason: it reads one record per tested candidate, and the matrix beside it
 *  polls every three seconds while a run is in flight. Keyed off the archive so
 *  it refreshes when a run lands and not otherwise. */
export const useFitnessHealth = (enabled: () => boolean = () => true, version: () => string = () => '') =>
  createQuery(() => ({
    queryKey: ['model-fitness-health', version()],
    enabled: enabled(),
    queryFn: (): Promise<HealthSummary> => getJson<HealthSummary>('/api/admin/model-fitness?view=health'),
    placeholderData: (prev: HealthSummary | undefined) => prev,
  }))

export interface CapabilitiesPayload {
  models: ModelRow[]
  index: Record<string, FitnessIndexEntry>
}

/** Capability facts and the last run's bands, for the three panels where a
 *  model is PICKED. Its own cache entry and its own, cheaper endpoint: opening
 *  the roles panel must not pull the whole matrix payload with it. */
export const useModelCapabilities = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: ['model-capabilities'],
    enabled: enabled(),
    queryFn: (): Promise<CapabilitiesPayload> => getJson<CapabilitiesPayload>('/api/admin/model-fitness?view=capabilities'),
  }))
