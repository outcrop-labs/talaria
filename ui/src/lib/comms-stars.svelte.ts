// The reactive face of the Comms rail's PINS — what a person pinned to the
// top of their rail (lib/comms-stars.ts has the key grammar; the code says
// "star" for the history, the screen says "Pin"). The SERVER holds them
// (`/api/me/pins`, the `comms_pins` table): one query that the rail, a
// conversation header's pin and a row's menu all read, so they move together
// in every tab and on every device. They lived in each browser's
// localStorage until now; the first load signed in folds anything still
// stored there into the server once, then clears it.
//
// A toggle is optimistic — the row moves under the cursor — and settles on
// the server's answer either way, so a refused write takes the move back.

import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJson, getJson, postJson } from './fetch-json'
import { toastError } from './toast.svelte'
import { readStars, starsStorageKey, isStarKey, type StarKey } from './comms-stars'
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

/** Fold a browser's old local stars into the server, once per user per
 *  browser: each one the server does not hold yet is pinned, in order, and
 *  the local copy is cleared so it never folds in twice. */
const imported = new Set<string>()
async function importLocalStars(userId: string, held: readonly string[]): Promise<boolean> {
  if (imported.has(userId)) return false
  imported.add(userId)
  const local = readStars(userId).filter((k) => !held.includes(k))
  for (const key of local) {
    try {
      await postJson('/api/me/pins', { key })
    } catch {
      // A key the server refuses (a stale shape, the cap) is dropped with
      // the rest of the local copy: it never showed anywhere else either.
    }
  }
  writeText(starsStorageKey(userId), null)
  return local.length > 0
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
      if (!id || !isStarKey(key)) return
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
