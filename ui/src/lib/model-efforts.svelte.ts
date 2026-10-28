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
import { resolve } from '@/lib/reactive-arg'
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

/** A reactive argument: pass a plain model id, or a getter when the model can
 *  change under the picker (a tier switch reroutes the turn). */
type MaybeModel = string | null | undefined | (() => string | null | undefined)


export function useModelEfforts(model: MaybeModel) {
  const query = createQuery(() => {
    const id = resolve(model)
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
  // DO NOT DESTRUCTURE THIS. Every field is a getter over the live query, so
  // `const { efforts } = useModelEfforts(...)` reads the query once, at mount
  // — `[]` on a fresh page — and the chip never appears. Keep the object and
  // read through it (`const efforts = $derived(x.efforts)`);
  // model-efforts.svelte.test.ts holds every caller to that.
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
    /** Whether THIS model's answer is in: read, and not failed. A caller that
     *  clears a saved level the model does not publish waits for it, because
     *  `efforts` is `[]` while the read is in flight (and after a failed one),
     *  and judged against that every saved level reads as retired. */
    get answered(): boolean {
      return query.isSuccess
    },
    /** The AGENT-CONFIGURED default for this model id (the pick saved beside
     *  the model in the agent editor), when the id is a persona whose config
     *  names one and the level is still published. Null everywhere else. */
    get default(): string | null {
      return query.data?.default ?? null
    },
  }
}
