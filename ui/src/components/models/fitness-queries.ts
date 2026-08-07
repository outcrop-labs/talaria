// The fitness surface's queries. Split from `fitness.ts` so that module stays
// free of runtime dependencies and can be unit tested — `vitest.config.ts` runs
// in plain node with no Svelte plugin, and `@tanstack/svelte-query` ships
// `.svelte` files it cannot load.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'
import type { DetailPayload, FitnessIndexEntry, MatrixPayload, ModelRow } from './fitness'

/** The matrix. Polls while a run is in flight and stops the moment it isn't —
 *  the same shape `useRagAdmin` uses for the reindex. */
export const useModelFitness = () =>
  createQuery(() => ({
    queryKey: ['model-fitness'],
    queryFn: (): Promise<MatrixPayload> => getJson<MatrixPayload>('/api/admin/model-fitness'),
    refetchInterval: (q) => (q.state.data?.status.state === 'running' ? 3_000 : false),
  }))

export const useFitnessDetail = (model: () => string | null) =>
  createQuery(() => ({
    queryKey: ['model-fitness-detail', model()],
    enabled: model() !== null,
    queryFn: (): Promise<DetailPayload> =>
      getJson<DetailPayload>(`/api/admin/model-fitness?view=detail&model=${encodeURIComponent(model() ?? '')}`),
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
