// The reactive face of the Comms rail's PINS — what a person pinned to the
// top of their rail (lib/comms-stars.ts has the key grammar; the code says
// "star" for the history, the screen says "Pin"). The SERVER holds them
// (`/api/me/pins`, the `comms_pins` table): one query that the rail, a
// conversation header's pin and a row's menu all read, so they move together
// in every tab and on every device. They lived in each browser's
// localStorage until now; a load signed in folds anything still stored there
// into the server, and keeps in the browser only what is worth another try —
// a star the server refused for good goes, one that met a 5xx, a lapsed
// session or no answer at all stays for the next load. A pin is never lost to
// a bad moment.
//
// A toggle is optimistic — the row moves under the cursor — and settles on
// the server's answer either way, so a refused write takes the move back.

import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJson, getJson, postJson } from './fetch-json'
import { toastError } from './toast.svelte'
import {
  readStars,
  starsStorageKey,
  starsToKeep,
  uploadOutcome,
  isStarKey,
  type StarKey,
  type UploadOutcome,
} from './comms-stars'
import { writeText } from './persist'
import { resolve, type MaybeGetter } from './reactive-arg'

export interface Stars {
  /** The pinned keys, in pin order (newest last). Empty while signed out
   *  or before the first read lands. */
  readonly keys: StarKey[]
  has: (key: string) => boolean
  /** Pin or unpin; a no-op while signed out. */
  toggle: (key: string) => void
}

export const pinsQueryKey = (userId: string) => ['comms-pins', userId] as const

/** Fold this browser's old local stars into the server. Each one the server
 *  does not hold yet is pinned, in order; the local copy then keeps only the
 *  stars that met a failure worth retrying (comms-stars.ts, uploadOutcome),
 *  so the next load tries those again and a pass where everything landed or
 *  was refused for good clears it. One pass per user at a time in a tab.
 *  Answers whether anything landed, so the caller re-reads the list. */
const importing = new Set<string>()
async function importLocalStars(userId: string, held: readonly string[]): Promise<boolean> {
  const local = readStars(userId)
  if (local.length === 0 || importing.has(userId)) return false
  importing.add(userId)
  try {
    const results: { key: StarKey; outcome: UploadOutcome }[] = []
    for (const key of local) {
      if (held.includes(key)) {
        results.push({ key, outcome: 'landed' })
        continue
      }
      try {
        await postJson('/api/me/pins', { key })
        results.push({ key, outcome: 'landed' })
      } catch (e) {
        results.push({ key, outcome: uploadOutcome(e) })
      }
    }
    const keep = starsToKeep(results)
    writeText(starsStorageKey(userId), keep.length > 0 ? JSON.stringify(keep) : null)
    return results.some((r) => r.outcome === 'landed' && !held.includes(r.key))
  } finally {
    importing.delete(userId)
  }
}

/** Call during component init. */
export function useStars(userId: MaybeGetter<string | null | undefined>): Stars {
  const qc = useQueryClient()
  const query = createQuery(() => {
    const id = resolve(userId) ?? null
    return {
      queryKey: pinsQueryKey(id ?? 'anon'),
      enabled: id !== null,
      staleTime: 30_000,
      queryFn: async (): Promise<StarKey[]> => {
        const { pins } = await getJson<{ pins: string[] }>('/api/me/pins')
        const keys = pins.filter(isStarKey)
        if (id && (await importLocalStars(id, keys))) {
          const again = await getJson<{ pins: string[] }>('/api/me/pins')
          return again.pins.filter(isStarKey)
        }
        return keys
      },
    }
  })

  const keys = (): StarKey[] => (resolve(userId) ? (query.data ?? []) : [])

  return {
    get keys() {
      return keys()
    },
    has: (key) => keys().includes(key as StarKey),
    toggle: (key) => {
      const id = resolve(userId)
      // Before the first read lands, "pinned" is unknown: a toggle then would
      // guess, and an unpin would go out as a pin.
      if (!id || !isStarKey(key) || query.data === undefined) return
      const qk = pinsQueryKey(id)
      const pinned = keys().includes(key)
      qc.setQueryData<StarKey[]>(qk, (old = []) => (pinned ? old.filter((k) => k !== key) : [...old, key]))
      const write = pinned ? delJson('/api/me/pins', { key }) : postJson('/api/me/pins', { key })
      void write
        .catch((e: unknown) => toastError(pinned ? 'Unpin failed' : 'Pin failed', e))
        .finally(() => void qc.invalidateQueries({ queryKey: qk }))
    },
  }
}
