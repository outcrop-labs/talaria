// thread-read.ts: what an open thread panel posts as read, and when.
import { describe, expect, it } from 'vitest'
import { NOTHING_POSTED, THREAD_PAGE_CAP, postedAfter, threadReadFor, trailingReads, type ThreadRead } from './thread-read'

const at = (input: Partial<Parameters<typeof threadReadFor>[0]>) =>
  threadReadFor({ rootId: 'r1', count: 3, latest: 9, fetchedAt: 100, posted: NOTHING_POSTED, looking: true, ...input })

describe('threadReadFor', () => {
  it('posts the newest seq of a page that holds the whole thread', () => {
    expect(at({})).toEqual({ rootId: 'r1', seq: 9 })
    expect(at({ posted: { id: 'r1', seq: 9, at: 100 } })).toBeNull()
    expect(at({ latest: 12, posted: { id: 'r1', seq: 9, at: 100 } })).toEqual({ rootId: 'r1', seq: 12 })
  })

  it('posts nothing while the person is not looking', () => {
    expect(at({ looking: false })).toBeNull()
    expect(at({ count: THREAD_PAGE_CAP, looking: false })).toBeNull()
  })

  it('posts no seq for a full page, whose last row is not the newest reply', () => {
    // 300 rows: the root and the 299 oldest replies. A cursor at the 300th's seq would never cover the rest.
    expect(at({ count: THREAD_PAGE_CAP, latest: 4000 })).toEqual({ rootId: 'r1', seq: null })
  })

  it('posts a full page again on each refetch, and not twice for one', () => {
    const posted = postedAfter({ rootId: 'r1', seq: null }, 100)
    expect(at({ count: THREAD_PAGE_CAP, latest: 4000, fetchedAt: 100, posted })).toBeNull()
    expect(at({ count: THREAD_PAGE_CAP, latest: 4000, fetchedAt: 250, posted })).toEqual({ rootId: 'r1', seq: null })
  })

  it("posts another thread's read whatever was posted for the first", () => {
    const posted = postedAfter({ rootId: 'r1', seq: 50 }, 900)
    expect(at({ rootId: 'r2', latest: 30, fetchedAt: 100, posted })).toEqual({ rootId: 'r2', seq: 30 })
    expect(at({ rootId: 'r2', count: THREAD_PAGE_CAP, fetchedAt: 100, posted })).toEqual({ rootId: 'r2', seq: null })
  })
})

/** Hand-cranked timers: `tick(ms)` runs what falls due. */
function clock() {
  let now = 0
  let next = 1
  const due = new Map<number, { at: number; fn: () => void }>()
  return {
    timers: {
      set: (fn: () => void, ms: number) => {
        const id = next++
        due.set(id, { at: now + ms, fn })
        return id
      },
      clear: (id: unknown) => void due.delete(id as number),
      now: () => now,
    },
    tick(ms: number) {
      const until = now + ms
      for (;;) {
        const first = [...due.entries()].filter(([, d]) => d.at <= until).sort((a, b) => a[1].at - b[1].at)[0]
        if (!first) break
        due.delete(first[0])
        now = first[1].at
        first[1].fn()
      }
      now = until
    },
  }
}

describe('trailingReads', () => {
  const setup = () => {
    const c = clock()
    const sent: ThreadRead[] = []
    const reads = trailingReads((r) => sent.push(r), { waitMs: 750, maxWaitMs: 3000, timers: c.timers })
    return { c, sent, reads }
  }

  it('sends one read, the newest, for a burst of replies', () => {
    const { c, sent, reads } = setup()
    for (const seq of [10, 11, 12, 13]) {
      reads.push({ rootId: 'r1', seq })
      c.tick(200)
    }
    expect(sent).toEqual([])
    c.tick(750)
    expect(sent).toEqual([{ rootId: 'r1', seq: 13 }])
  })

  it('still reads a steady stream, at most every max wait', () => {
    const { c, sent, reads } = setup()
    for (let seq = 1; seq <= 20; seq++) {
      reads.push({ rootId: 'r1', seq })
      c.tick(500)
    }
    // 20 pushes 500 ms apart never leave a 750 ms gap; the 3 s ceiling sends anyway.
    expect(sent.length).toBeGreaterThanOrEqual(3)
    expect(sent.every((r, i) => i === 0 || (r.seq ?? 0) > (sent[i - 1]?.seq ?? 0))).toBe(true)
  })

  it("sends the held read at once when the panel switches to another thread", () => {
    const { c, sent, reads } = setup()
    reads.push({ rootId: 'r1', seq: 50 })
    c.tick(100)
    reads.push({ rootId: 'r2', seq: 30 })
    expect(sent).toEqual([{ rootId: 'r1', seq: 50 }])
    c.tick(750)
    expect(sent).toEqual([
      { rootId: 'r1', seq: 50 },
      { rootId: 'r2', seq: 30 },
    ])
  })

  it('flush sends what is held, once (the panel closing)', () => {
    const { c, sent, reads } = setup()
    reads.push({ rootId: 'r1', seq: 7 })
    reads.flush()
    reads.flush()
    c.tick(5000)
    expect(sent).toEqual([{ rootId: 'r1', seq: 7 }])
  })
})
