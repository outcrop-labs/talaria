// Shared shapes + the inference-plane query for the compute panel
// (ComputePanel.svelte + LiveSection.svelte).
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface LocalBackend {
  id: string
  name: string
  baseUrl: string | null
  models: string[]
  health: { ok: boolean; latencyMs: number | null; servingNow: string[]; note: string | null }
}

export interface InferenceLive {
  generating: Array<{ agentModel: string; count: number }>
  lastHour: Array<{ agentModel: string; generations: number; tokens: number; lastAt: string }>
  gateway: { requests: number; errors: number; p50: number | null; p95: number | null }
  fleet: { running: number; warming: number; unhealthy: number; down: number }
}

export interface InferenceData {
  live: InferenceLive
  backends: LocalBackend[]
  usage: { today: number; month: number; generations: number; perModel: Array<{ llmModel: string | null; tokens: number }> }
}

export const useInference = () =>
  createQuery(() => ({
    queryKey: ['inference'],
    queryFn: (): Promise<InferenceData> => getJson<InferenceData>('/api/inference'),
    // Lean in while something is actually generating or warming.
    refetchInterval: (q) => {
      const live = q.state.data?.live
      return live && (live.generating.length > 0 || live.fleet.warming > 0) ? 5_000 : 15_000
    },
  }))
