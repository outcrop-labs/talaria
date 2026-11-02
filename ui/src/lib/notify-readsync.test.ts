import { describe, expect, it, vi } from 'vitest'
import { SHELL_CLOSE_MAX, applyReadSync, type ReadSyncDeps, type ShownBanner } from './notify-readsync'
import { SUMMARY_TAG } from './notify-wire'

const A = '00000000-0000-4000-8000-00000000000a'
const B = '00000000-0000-4000-8000-00000000000b'
const C = '00000000-0000-4000-8000-00000000000c'

function rig(tags: string[], toasts: string[] = [], opts: { shell?: boolean; summary?: string[] } = {}) {
  let up = tags.slice()
  let onScreen = toasts.slice()
  let summary = opts.summary ?? []
  const shellCalls: string[][] = []
  const banners = (): ShownBanner[] =>
    up.map((tag) => ({
      tag,
      close: () => {
        up = up.filter((t) => t !== tag)
      },
    }))
  const deps: ReadSyncDeps = {
    shown: async () => banners(),
    dismiss: (keys) => {
      onScreen = keys === 'all' ? [] : onScreen.filter((k) => !keys.includes(k))
    },
    toastKeys: () => onScreen,
    shellClose: opts.shell
      ? async (t) => {
          shellCalls.push(t)
          return true
        }
      : undefined,
    recentTags: async () => [A, B, C],
    summaryIds: () => summary,
    summaryClosed: () => {
      summary = []
    },
  }
  return { deps, up: () => up, toasts: () => onScreen, shellCalls, summary: () => summary }
}

describe('applyReadSync (I-16)', () => {
  it('readIds: closes exactly those banners and toasts, nothing else', async () => {
    const r = rig([A, B, 'talaria-enabled'], [A, C])
    const out = await applyReadSync({ type: 'notifications', unread: 1, readIds: [A] }, r.deps)
    expect(r.up()).toEqual([B, 'talaria-enabled'])
    expect(r.toasts()).toEqual([C])
    expect(out.closed).toEqual([A])
  })

  it('all: every banner and every notification toast', async () => {
    const r = rig([A, B, SUMMARY_TAG], [A, C], { summary: [A, B] })
    await applyReadSync({ type: 'notifications', unread: 0, all: true }, r.deps)
    expect(r.up()).toEqual([])
    expect(r.toasts()).toEqual([])
    expect(r.summary()).toEqual([])
  })

  it('a counts-only event closes nothing (it does not say which rows)', async () => {
    const r = rig([A, B], [A])
    await applyReadSync({ type: 'notifications', unread: 3 }, r.deps)
    expect(r.up()).toEqual([A, B])
    expect(r.toasts()).toEqual([A])
  })

  it('the summary goes once every row it stands for is read — not before', async () => {
    const r = rig([SUMMARY_TAG], [SUMMARY_TAG], { summary: [A, B] })
    await applyReadSync({ type: 'notifications', unread: 1, readIds: [A] }, r.deps)
    expect(r.up()).toEqual([SUMMARY_TAG])
    await applyReadSync({ type: 'notifications', unread: 0, readIds: [B, A] }, r.deps)
    expect(r.up()).toEqual([])
    expect(r.toasts()).toEqual([])
  })

  it('inside the shell: asks it to close the ids (desktop_notify_close), and the recent tags on all', async () => {
    const r = rig([], [], { shell: true })
    await applyReadSync({ type: 'notifications', unread: 1, readIds: [A, B] }, r.deps)
    await applyReadSync({ type: 'notifications', unread: 0, all: true }, r.deps)
    expect(r.shellCalls).toEqual([[A, B], [SUMMARY_TAG, A, B, C]])
  })

  it('inside the shell: never more than the shell takes in one call (100), the summary tag first', async () => {
    const many = Array.from({ length: 200 }, (_, i) => `00000000-0000-4000-8000-${String(i).padStart(12, '0')}`)
    const calls: string[][] = []
    const deps: ReadSyncDeps = {
      shown: async () => [],
      dismiss: () => {},
      toastKeys: () => [],
      shellClose: async (t) => {
        calls.push(t)
        return true
      },
      recentTags: async () => many,
      summaryIds: () => many.slice(0, 100),
      summaryClosed: () => {},
    }
    await applyReadSync({ type: 'notifications', unread: 0, all: true }, deps)
    expect(calls.map((c) => c.length)).toEqual([SHELL_CLOSE_MAX, SHELL_CLOSE_MAX, 1])
    expect(calls[0]?.[0]).toBe(SUMMARY_TAG)
    expect(calls.flat()).toHaveLength(201)
    calls.length = 0
    // 100 read ids that complete the summary: 101 tags, two calls, none dropped.
    await applyReadSync({ type: 'notifications', unread: 0, readIds: many.slice(0, 100) }, deps)
    expect(calls.map((c) => c.length)).toEqual([SHELL_CLOSE_MAX, 1])
    expect(calls[0]?.[0]).toBe(SUMMARY_TAG)
  })

  it('resync: refetch, then close every banner whose row is no longer unread', async () => {
    const r = rig([A, B, C, 'talaria-enabled'], [B])
    const refetch = vi.fn(async () => ({ unread: new Set([C]), read: new Set([A]), complete: false }))
    await applyReadSync({ type: 'notifications', unread: 1, resync: true }, { ...r.deps, refetch })
    // A is read; B was not in an incomplete list — it stays; C is unread.
    expect(r.up()).toEqual([B, C, 'talaria-enabled'])
    // With a complete list, absence is read.
    await applyReadSync({ type: 'notifications', unread: 1, resync: true }, { ...r.deps, refetch: async () => ({ unread: new Set([C]), read: new Set(), complete: true }) })
    expect(r.up()).toEqual([C, 'talaria-enabled'])
    expect(r.toasts()).toEqual([])
  })

  it('a registration that cannot be read closes nothing and throws nothing', async () => {
    const r = rig([A], [A])
    const out = await applyReadSync({ type: 'notifications', unread: 0, readIds: [A] }, {
      ...r.deps,
      shown: async () => {
        throw new Error('no registration')
      },
    })
    expect(out.closed).toEqual([])
    expect(r.toasts()).toEqual([])
  })
})
