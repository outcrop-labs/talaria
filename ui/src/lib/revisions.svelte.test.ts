// @vitest-environment runes
//
// Read panes follow the server. The agent-refine notice used to be what
// refreshed a read pane when an agent edited the document; the notice is
// gone and the refresh stays, with no UI. These run the real follower (real
// svelte-query on Svelte's client runtime, a stubbed /api/history) and pin
// when it refreshes — and that opening a document is never a refresh.
import { focusManager, QueryClient } from '@tanstack/svelte-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { followRevisions, newestMoved, type Revision } from './revisions.svelte'

// query-core decides at import whether it runs on a server (`typeof window
// === 'undefined'`), and a server never schedules `refetchInterval` — so
// without a window this would test a follower that cannot poll. Hoisted
// above the imports: the decision is made when query-core loads.
vi.hoisted(() => {
  ;(globalThis as { window?: unknown }).window ??= globalThis
})

const rev = (id: string, createdBy = 'tiffany-personal-tiffany'): Revision => ({
  id,
  createdBy,
  createdAt: '2026-11-03T10:00:00.000Z',
  size: 10,
})

describe('newestMoved', () => {
  it('the first answer is a baseline, never a move', () => {
    expect(newestMoved(undefined, [rev('a')])).toBe(false)
    expect(newestMoved(undefined, [])).toBe(false)
  })
  it('a different newest revision is a move; the same one is not', () => {
    expect(newestMoved('a', [rev('b'), rev('a')])).toBe(true)
    expect(newestMoved('a', [rev('a')])).toBe(false)
    expect(newestMoved(null, [rev('a')])).toBe(true)
  })
})

describe('followRevisions', () => {
  // refetchInterval pauses while the window is unfocused (a hidden tab polls
  // nothing); this runtime has no document focus, so say it is visible.
  beforeEach(() => {
    focusManager.setFocused(true)
  })
  afterEach(() => {
    focusManager.setFocused(undefined)
    vi.unstubAllGlobals()
  })

  function stage(initial: Revision[]) {
    let revisions = initial
    const fetchMock = vi.fn(async (url: string | URL | Request) => {
      expect(String(url)).toBe('/api/history?kind=kb-doc&id=doc-1')
      return new Response(JSON.stringify({ revisions }), { status: 200, headers: { 'content-type': 'application/json' } })
    })
    vi.stubGlobal('fetch', fetchMock)
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    let active = $state(true)
    const onMoved = vi.fn()
    const stop = $effect.root(() => {
      followRevisions(
        () => 'kb-doc',
        () => 'doc-1',
        { active: () => active, onMoved, intervalMs: 15, client: () => client },
      )
    })
    return {
      onMoved,
      fetchMock,
      /** What a save does: the rail's list refetches. */
      refetch: () => client.invalidateQueries({ queryKey: ['history'] }),
      land: (r: Revision) => {
        revisions = [r, ...revisions]
      },
      setActive: (v: boolean) => {
        active = v
      },
      done: () => {
        stop()
        client.clear()
      },
    }
  }

  const polls = (s: { fetchMock: { mock: { calls: unknown[] } } }, n: number) =>
    vi.waitFor(() => expect(s.fetchMock.mock.calls.length).toBeGreaterThanOrEqual(n))

  it('opening a document — even one an agent just created — refreshes nothing', async () => {
    const s = stage([rev('created-by-agent')])
    await polls(s, 3)
    expect(s.onMoved).not.toHaveBeenCalled()
    s.done()
  })

  it('a revision landing refreshes the read pane once', async () => {
    const s = stage([rev('a')])
    await polls(s, 1)
    s.land(rev('b'))
    await vi.waitFor(() => expect(s.onMoved).toHaveBeenCalledTimes(1))
    const seen = s.fetchMock.mock.calls.length
    await polls(s, seen + 3)
    expect(s.onMoved).toHaveBeenCalledTimes(1)
    s.done()
  })

  it('an edit pane does not poll or refresh; a move it saw refreshes the pane when it reads again', async () => {
    const s = stage([rev('a')])
    await polls(s, 1)
    s.setActive(false)
    await new Promise((r) => setTimeout(r, 60))
    const idle = s.fetchMock.mock.calls.length
    await new Promise((r) => setTimeout(r, 60))
    expect(s.fetchMock.mock.calls.length).toBe(idle)
    // An agent saves while the person edits; the list refetches (a save, the
    // open rail) — the edit pane is not refreshed under the person.
    s.land(rev('b'))
    await s.refetch()
    await new Promise((r) => setTimeout(r, 30))
    expect(s.onMoved).not.toHaveBeenCalled()
    s.setActive(true)
    await vi.waitFor(() => expect(s.onMoved).toHaveBeenCalledTimes(1))
    s.done()
  })
})
