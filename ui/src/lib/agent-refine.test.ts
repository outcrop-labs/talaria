// TALA-4: the decisions behind the agent-refine notice. The component wires
// these into effects; here every branch is pinned, including the ones that
// must NOT announce (your own save, a revision you already saw).

import { describe, expect, it } from 'vitest'
import { refineToAnnounce, snapshotBody, summarize, viewerStamp, type RefineRev } from './agent-refine'

const rev = (createdAt: string, createdBy: string | null): RefineRev => ({
  id: `rev-${createdAt}`,
  createdBy,
  createdAt,
  size: 10,
})

describe('refineToAnnounce', () => {
  it('announces a revision written by someone else', () => {
    const revs = [rev('2026-09-15T10:00:00Z', 'doug-engineering')]
    expect(refineToAnnounce(revs, 'jon@outcroplabs.com', null)).toEqual(revs[0])
  })

  it('never announces the viewer’s own save', () => {
    const revs = [rev('2026-09-15T10:00:00Z', 'jon@outcroplabs.com')]
    expect(refineToAnnounce(revs, 'jon@outcroplabs.com', null)).toBeNull()
  })

  it('still announces agent-written revisions with no local viewer identity', () => {
    const revs = [rev('2026-09-15T10:00:00Z', 'doug-engineering')]
    expect(refineToAnnounce(revs, null, null)).toEqual(revs[0])
  })

  it('announces nothing when the document has no revisions', () => {
    expect(refineToAnnounce([], 'jon@outcroplabs.com', null)).toBeNull()
  })

  it('does not re-announce a revision already seen', () => {
    const revs = [rev('2026-09-15T10:00:00Z', 'doug-engineering')]
    expect(refineToAnnounce(revs, 'jon@outcroplabs.com', '2026-09-15T10:00:00Z')).toBeNull()
  })

  it('announces a NEWER unseen revision after an older one was seen', () => {
    const revs = [rev('2026-09-15T11:00:00Z', 'doug-engineering'), rev('2026-09-15T10:00:00Z', 'doug-engineering')]
    expect(refineToAnnounce(revs, 'jon@outcroplabs.com', '2026-09-15T10:00:00Z')).toEqual(revs[0])
  })

  it('announces a second agent refine after the viewer saved in between', () => {
    const revs = [rev('2026-09-15T12:00:00Z', 'muse-agent'), rev('2026-09-15T11:00:00Z', 'jon@outcroplabs.com')]
    expect(refineToAnnounce(revs, 'jon@outcroplabs.com', '2026-09-15T11:00:00Z')).toEqual(revs[0])
  })

  it('treats an unattributed revision as an announce (fail loud, not silent)', () => {
    const revs = [rev('2026-09-15T10:00:00Z', null)]
    expect(refineToAnnounce(revs, 'jon@outcroplabs.com', null)).toEqual(revs[0])
  })
})

describe('viewerStamp', () => {
  it('prefers the email, mirroring the API’s actor rule', () => {
    expect(viewerStamp({ email: 'jon@outcroplabs.com', name: 'Jon' })).toBe('jon@outcroplabs.com')
  })

  it('falls back to the name', () => {
    expect(viewerStamp({ email: null, name: 'Jon' })).toBe('Jon')
  })

  it('is null with no session', () => {
    expect(viewerStamp(null)).toBeNull()
  })
})

describe('summarize', () => {
  it('counts additions and removals', () => {
    expect(summarize('a\nb\nc', 'a\nB\nc\nd')).toEqual({ adds: 2, dels: 1, text: '2 added · 1 removed lines', oversized: false })
  })

  it('singularizes when only one line changed', () => {
    expect(summarize('a\nb', 'a\nB').text).toBe('1 added · 1 removed lines')
    expect(summarize('a\nb', 'a\nb\nc').text).toBe('1 added · 0 removed line')
  })

  it('reports the no-op honestly', () => {
    expect(summarize('same', 'same').text).toBe('no text changes')
  })

  it('degrades to "changed" when the document outgrows the client diff', () => {
    const big = Array.from({ length: 2000 }, (_, i) => `line ${i}`).join('\n')
    expect(summarize(big, `${big}\nmore`).oversized).toBe(true)
    expect(summarize(big, `${big}\nmore`).text).toBe('changed')
  })
})

describe('snapshotBody', () => {
  it('splits the stored "# Title\n\n<body>" shape', () => {
    expect(snapshotBody('# My doc\n\nHello\nworld')).toBe('Hello\nworld')
  })

  it('passes through content that is not title-prefixed', () => {
    expect(snapshotBody('just a body')).toBe('just a body')
  })
})
