// The reactive face of lib/comms-stars.ts: one module-level slot per user, so
// the Comms rail, a conversation header's star and a row's context menu all
// read the same list and move together within the tab. Other tabs reach it
// through the `storage` event.
//
// SSR discipline as in nav-dock.svelte.ts: the slot starts empty (the server
// has no store), and the hook adopts the persisted list in an `$effect`, after
// hydration, so server markup and the client's first paint agree.

import { resolve, type MaybeGetter } from './reactive-arg'
import { readStars, starsStorageUser, toggleStar, type StarKey } from './comms-stars'

const byUser = $state<Record<string, StarKey[]>>({})

export interface Stars {
  /** The star keys, in star order (newest last). Empty while signed out. */
  readonly keys: StarKey[]
  has: (key: string) => boolean
  /** Star or unstar; a no-op while signed out. */
  toggle: (key: string) => void
}

/** Call during component init (it registers an `$effect`). */
export function useStars(userId: MaybeGetter<string | null | undefined>): Stars {
  $effect(() => {
    const id = resolve(userId)
    if (!id) return
    byUser[id] = readStars(id)
    const onStorage = (e: StorageEvent) => {
      // A null key is localStorage.clear() in another tab.
      if (e.key === null || starsStorageUser(e.key) === id) byUser[id] = readStars(id)
    }
    window.addEventListener('storage', onStorage)
    return () => window.removeEventListener('storage', onStorage)
  })

  const keys = (): StarKey[] => {
    const id = resolve(userId)
    return (id && byUser[id]) || []
  }

  return {
    get keys() {
      return keys()
    },
    has: (key) => keys().includes(key as StarKey),
    toggle: (key) => {
      const id = resolve(userId)
      if (id) byUser[id] = toggleStar(id, key)
    },
  }
}
