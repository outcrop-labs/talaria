// The Browse view's pure state derivation, pinned here so the route body
// (routes/app/Browse.svelte) is a thin renderer. The tab-strip shape comes
// from the shell's registry (desktop/src-tauri/src/registry.rs
// `BrowserTab`, camelCase serde): the bridge module (desktop-shell.ts)
// mirrors it; nothing here imports the bridge or the svelte-query runtime —
// the repo's module-split convention (workchains.test.ts says why).
import { describe, expect, it } from 'vitest'
import {
  assertPinnedFirst,
  displayTitle,
  pinPayload,
  tabGroups,
  type BrowserTab,
} from './browse-tabs'

const tab = (over: Partial<BrowserTab> = {}): BrowserTab => ({
  id: 'tab-1',
  title: 'Outcrop',
  url: 'https://outcrop.example/work',
  pinned: false,
  createdAt: 1_000,
  lastActiveAt: 2_000,
  ...over,
})

describe('assertPinnedFirst', () => {
  it('accepts the shell contract: pinned tabs ahead of unpinned', () => {
    const tabs = [
      tab({ id: 'p', pinned: true, createdAt: 2_000 }),
      tab({ id: 'q', pinned: true, createdAt: 3_000 }),
      tab({ id: 'a', createdAt: 1_000 }),
    ]
    expect(assertPinnedFirst(tabs)).toBe(tabs)
  })

  it('throws on a list that violates the contract instead of re-sorting', () => {
    const broken = [
      tab({ id: 'a', createdAt: 1_000 }),
      tab({ id: 'p', pinned: true, createdAt: 2_000 }),
    ]
    expect(() => assertPinnedFirst(broken)).toThrow(/pinned/)
  })

  it('passes an empty list', () => {
    expect(assertPinnedFirst([])).toEqual([])
  })
})

describe('tabGroups', () => {
  it('splits pinned from unpinned without reordering either side', () => {
    const tabs = [
      tab({ id: 'p1', pinned: true, createdAt: 2 }),
      tab({ id: 'p2', pinned: true, createdAt: 3 }),
      tab({ id: 'u1', createdAt: 1 }),
      tab({ id: 'u2', createdAt: 4 }),
    ]
    expect(tabGroups(tabs)).toEqual({
      pinned: [tabs[0], tabs[1]],
      unpinned: [tabs[2], tabs[3]],
    })
  })

  it('empty list groups to two empty sides', () => {
    expect(tabGroups([])).toEqual({ pinned: [], unpinned: [] })
  })

  it('uses the groups as the strip render order: pinned, then unpinned', () => {
    const tabs = [
      tab({ id: 'u', createdAt: 1 }),
      tab({ id: 'p', pinned: true, createdAt: 5 }),
    ]
    expect(tabGroups(tabs).pinned.map((t) => t.id)).toEqual(['p'])
    expect(tabGroups(tabs).unpinned.map((t) => t.id)).toEqual(['u'])
  })
})

describe('displayTitle', () => {
  it('uses the tab title when the page said one', () => {
    expect(displayTitle(tab({ title: 'Outcrop' }))).toBe('Outcrop')
  })

  it('falls back to the URL host for an untitled tab', () => {
    expect(displayTitle(tab({ title: '', url: 'https://outcrop.example/work' }))).toBe(
      'outcrop.example'
    )
  })

  it('keeps the port in the fallback (a dev instance on 127.0.0.1)', () => {
    expect(displayTitle(tab({ title: '', url: 'http://127.0.0.1:5302/comms' }))).toBe(
      '127.0.0.1:5302'
    )
  })

  it('falls back to the raw URL when even the host does not parse', () => {
    expect(displayTitle(tab({ title: '', url: 'not a url' }))).toBe('not a url')
  })
})

describe('pinPayload', () => {
  it('names the command the bridge sends for a pin toggle', () => {
    expect(pinPayload('tab-1', true)).toEqual({ id: 'tab-1', pinned: true })
    expect(pinPayload('tab-9', false)).toEqual({ id: 'tab-9', pinned: false })
  })
})
