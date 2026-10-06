// The keys you have drafts under, live — for the Comms rail's draft marks: a
// pencil and a count on "Drafts & sent", and a pencil beside each conversation
// with a half-written message. Drafts live in localStorage
// (comms-drafts.ts); this re-reads them whenever one is saved or cleared here
// (DRAFTS_CHANGED) or in another tab (`storage`).

import { DRAFTS_CHANGED, isDraftStorageKey, listDrafts } from './comms-drafts'

export function useDraftKeys(userId: () => string | null) {
  let keys = $state<ReadonlySet<string>>(new Set())

  $effect(() => {
    const id = userId()
    const read = () => {
      keys = new Set(id ? listDrafts(id).map((d) => d.key) : [])
    }
    // A composer can save while Svelte is mid-update (a teardown flushing its
    // draft); writing state there is an unsafe mutation, so the re-read waits
    // for the microtask after.
    let queued = false
    const refresh = () => {
      if (queued) return
      queued = true
      queueMicrotask(() => {
        queued = false
        read()
      })
    }
    read()
    const onStorage = (e: StorageEvent) => {
      if (e.key === null || isDraftStorageKey(e.key)) refresh()
    }
    window.addEventListener(DRAFTS_CHANGED, refresh)
    window.addEventListener('storage', onStorage)
    return () => {
      window.removeEventListener(DRAFTS_CHANGED, refresh)
      window.removeEventListener('storage', onStorage)
    }
  })

  return {
    get keys() {
      return keys
    },
    has: (key: string | null | undefined) => !!key && keys.has(key),
  }
}
