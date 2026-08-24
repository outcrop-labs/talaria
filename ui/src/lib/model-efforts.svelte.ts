// The composer's effort-picker feed, client side.
//
// One query answers one question for a chat surface: "which reasoning-effort
// levels may THIS model be asked for?" The server resolves the id — a fleet
// persona (base or tier) or a gateway catalog id — against the per-model
// metadata extracted at catalog refresh, and `[]` is a real answer meaning
// "no picker": a model nobody has published levels for gets no effort control
// and no effort field on its requests. A failed read also renders as no
// picker, which is the honest degradation for a dial, not a contract — better
// a missing control than a chat surface that refuses to send.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

/** A reactive argument: pass a plain model id, or a getter when the model can
 *  change under the picker (a tier switch reroutes the turn). */
type MaybeModel = string | null | undefined | (() => string | null | undefined)

const resolveModel = (v: MaybeModel): string | null | undefined => (typeof v === 'function' ? v() : v)

export function useModelEfforts(model: MaybeModel) {
  const query = createQuery(() => {
    const id = resolveModel(model)
    return {
      queryKey: ['model-efforts', id ?? ''],
      enabled: Boolean(id),
      // Same freshness the gateway-models catalog claims: catalog refreshes run
      // on a daily cadence, so a minute of cache is invisible.
      staleTime: 60_000,
      queryFn: (): Promise<{ efforts: string[]; default: string | null }> =>
        getJson<{ efforts: string[]; default: string | null }>(`/api/models/efforts?model=${encodeURIComponent(id!)}`),
    }
  })
  return {
    /** The levels this model supports, or `[]` while unknown/unavailable —
     *  see the file header for why empty is the only failure shape. */
    get efforts(): string[] {
      return query.data?.efforts ?? []
    },
    /** Whether the answer has landed. A caller that renders a "no levels"
     * hint needs to distinguish "none published" from "not asked yet". */
    get isLoading(): boolean {
      return query.isLoading
    },
    /** The AGENT-CONFIGURED default for this model id (the pick saved beside
     *  the model in the agent editor), when the id is a persona whose config
     *  names one and the level is still published. Null everywhere else. */
    get default(): string | null {
      return query.data?.default ?? null
    },
  }
}
