import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  BANNERED_DB,
  BANNERED_MAX,
  BANNERED_MAX_AGE_MS,
  BANNERED_STORE,
  covers,
  idbBanneredStore,
  markBannered,
  memoryBanneredStore,
  pruneKeys,
  wasBannered,
  type BanneredEntry,
} from './notify-bannered'

const NOW = 1_800_000_000_000

describe('covers — one version of one row is bannered once', () => {
  it('covers the version shown and anything older, never a newer one', () => {
    const e: BanneredEntry = { v: '2026-10-31T12:00:05.000Z', at: NOW }
    expect(covers(undefined, '2026-10-31T12:00:05.000Z')).toBe(false)
    expect(covers(e, '2026-10-31T12:00:05.000Z')).toBe(true)
    expect(covers(e, '2026-10-31T12:00:01.000Z')).toBe(true)
    // A DM row refreshed by a newer message is a new version: shown again.
    expect(covers(e, '2026-10-31T12:00:09.000Z')).toBe(false)
    // An unversioned ask is covered by any entry.
    expect(covers(e, '')).toBe(true)
  })
})

describe('pruneKeys', () => {
  it('drops entries past seven days, then past the newest thousand', () => {
    const old: [string, BanneredEntry] = ['old', { v: '', at: NOW - BANNERED_MAX_AGE_MS - 1 }]
    const fresh: [string, BanneredEntry] = ['fresh', { v: '', at: NOW - 1000 }]
    expect(pruneKeys([old, fresh], NOW)).toEqual(['old'])
    const many = Array.from({ length: BANNERED_MAX + 3 }, (_, i) => [`k${i}`, { v: '', at: NOW - i }] as [string, BanneredEntry])
    expect(pruneKeys(many, NOW).sort()).toEqual([`k${BANNERED_MAX}`, `k${BANNERED_MAX + 1}`, `k${BANNERED_MAX + 2}`].sort())
  })
})

describe('markBannered / wasBannered', () => {
  it('records what was shown and prunes on the occasional write', async () => {
    const store = memoryBanneredStore()
    await store.put('stale', { v: '', at: NOW - BANNERED_MAX_AGE_MS - 1 })
    expect(await wasBannered(store, 'n1', 'v1')).toBe(false)
    await markBannered(store, 'n1', 'v1', NOW, () => 0.9)
    expect(await wasBannered(store, 'n1', 'v1')).toBe(true)
    expect(await store.get('stale')).toBeDefined()
    await markBannered(store, 'n2', 'v1', NOW, () => 0)
    expect(await store.get('stale')).toBeUndefined()
  })

  it('without IndexedDB, falls back to memory instead of throwing', async () => {
    const store = idbBanneredStore(undefined)
    await markBannered(store, 'n1', 'v', NOW, () => 1)
    expect(await wasBannered(store, 'n1', 'v')).toBe(true)
  })
})

describe('the service worker carries the same store', () => {
  it('names the same database and object store', () => {
    // sw.ts is a classic worker script and cannot import this module; its
    // inlined copy must open the very store the pages write.
    const sw = readFileSync(join(__dirname, '../sw.ts'), 'utf8')
    expect(sw).toContain(`'${BANNERED_DB}'`)
    expect(sw).toContain(`'${BANNERED_STORE}'`)
  })
})
