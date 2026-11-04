// WHICH ROWS THIS BROWSER ALREADY BANNERED — shared by every tab and the
// service worker, so the two sources of OS banners (the leader tab, from the
// firehose or the poll; the worker, from a push) never show one row twice.
//
// The tag namespace already collapses a SECOND copy of the same row into the
// first. What it cannot do is stop the second one from ringing: a re-show
// under an existing tag still pops (with `renotify`) or silently replaces,
// and a banner the person already dismissed would come back. So whoever
// shows a banner records it here first, and both check before showing.
//
// IndexedDB because it is the one store a page and its service worker share.
// Keyed by row id; the value names the VERSION shown (the row's createdAt —
// a DM row refreshed in place by a newer message is a new version and is
// shown again) and when. Entries past 7 days, or past the newest 1,000, are
// pruned on write.
//
// sw.ts cannot import (it is a classic worker script), so it carries an
// inlined copy of the same open/get/put — the names below are pinned against
// it by notify-bannered.test.ts.

export const BANNERED_DB = 'talaria-notify'
export const BANNERED_STORE = 'bannered'
export const BANNERED_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000
export const BANNERED_MAX = 1000

export interface BanneredEntry {
  /** The row version shown: its createdAt (ISO, millisecond precision). */
  v: string
  /** When it was shown, ms epoch. */
  at: number
}

/** Does a recorded entry cover this version? ISO strings of one format
 *  compare as their instants do; an unversioned ask ('') is covered by any
 *  entry. */
export function covers(entry: BanneredEntry | undefined, version: string): boolean {
  if (!entry) return false
  return !version || entry.v >= version
}

/** The keys to delete: older than the age bound, then everything past the
 *  newest `BANNERED_MAX`. */
export function pruneKeys(entries: ReadonlyArray<[string, BanneredEntry]>, now: number): string[] {
  const out = new Set<string>()
  for (const [k, e] of entries) if (now - e.at > BANNERED_MAX_AGE_MS) out.add(k)
  const live = entries.filter(([k]) => !out.has(k)).sort((a, b) => b[1].at - a[1].at)
  for (const [k] of live.slice(BANNERED_MAX)) out.add(k)
  return [...out]
}

export interface BanneredStore {
  get(id: string): Promise<BanneredEntry | undefined>
  put(id: string, entry: BanneredEntry): Promise<void>
  entries(): Promise<Array<[string, BanneredEntry]>>
  remove(ids: string[]): Promise<void>
}

/** The in-memory store: tests, and a browser without IndexedDB (each context
 *  then dedupes only within itself, and the shared tag does the rest). */
export function memoryBanneredStore(): BanneredStore {
  const m = new Map<string, BanneredEntry>()
  return {
    get: async (id) => m.get(id),
    put: async (id, e) => void m.set(id, e),
    entries: async () => [...m.entries()],
    remove: async (ids) => ids.forEach((id) => m.delete(id)),
  }
}

const req = <T>(r: IDBRequest<T>): Promise<T> =>
  new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result)
    r.onerror = () => reject(r.error)
  })

/** The shared IndexedDB store. Every failure answers like an empty store —
 *  a store that cannot be read must never stop a banner from showing. */
export function idbBanneredStore(factory: IDBFactory | undefined = globalThis.indexedDB): BanneredStore {
  if (!factory) return memoryBanneredStore()
  let opened: Promise<IDBDatabase> | null = null
  const db = () => {
    opened ??= new Promise<IDBDatabase>((resolve, reject) => {
      const r = factory.open(BANNERED_DB, 1)
      r.onupgradeneeded = () => {
        if (!r.result.objectStoreNames.contains(BANNERED_STORE)) r.result.createObjectStore(BANNERED_STORE)
      }
      r.onsuccess = () => resolve(r.result)
      r.onerror = () => reject(r.error)
    })
    return opened
  }
  const tx = async (mode: IDBTransactionMode) => (await db()).transaction(BANNERED_STORE, mode).objectStore(BANNERED_STORE)
  return {
    get: async (id) => {
      try {
        return (await req((await tx('readonly')).get(id))) as BanneredEntry | undefined
      } catch {
        return undefined
      }
    },
    put: async (id, e) => {
      try {
        await req((await tx('readwrite')).put(e, id))
      } catch {
        /* see above */
      }
    },
    entries: async () => {
      try {
        const s = await tx('readonly')
        const [keys, values] = await Promise.all([req(s.getAllKeys()), req(s.getAll())])
        return keys.map((k, i) => [String(k), values[i] as BanneredEntry] as [string, BanneredEntry])
      } catch {
        return []
      }
    },
    remove: async (ids) => {
      try {
        const s = await tx('readwrite')
        await Promise.all(ids.map((id) => req(s.delete(id))))
      } catch {
        /* see above */
      }
    },
  }
}

/** Was this version of this row already bannered in this browser? */
export async function wasBannered(store: BanneredStore, id: string, version: string): Promise<boolean> {
  return covers(await store.get(id), version)
}

/** Record a banner, and prune now and then (every write is cheap; the prune
 *  reads the whole store, so it runs on one write in twenty). */
export async function markBannered(
  store: BanneredStore,
  id: string,
  version: string,
  now = Date.now(),
  rand = Math.random,
): Promise<void> {
  await store.put(id, { v: version, at: now })
  if (rand() < 0.05) {
    const drop = pruneKeys(await store.entries(), now)
    if (drop.length) await store.remove(drop)
  }
}
