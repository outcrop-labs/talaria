// ONE SUBJECT, ONE DOCUMENT — extending a report instead of writing a second.
//
// A follow-up used to mint its own report, with its own source list numbered
// from [1] and nothing linking the two, so the answer to one question lived in
// two places and the reader assembled it. Extending is better, and it is also
// the operation that can silently corrupt a document somebody has already read
// and quoted — which is what these tests are for.
import { describe, expect, it, vi } from 'vitest'

vi.mock('@/server/db/pg', () => ({ db: async () => (() => Promise.resolve([])) as never }))

const { SourceRegistry, extendReport, reportBodyOnly } = await import('@/server/research')

const src = (idx: number, host: string) => ({ idx, url: `https://${host}/p`, title: host, snippet: null })

const PARENT = [
  '# Agent seat pricing',
  '',
  'Comparable platforms charge per seat [1], and most bundle a number of agents [2].',
  '',
  '## Sources',
  '',
  '1. [a.test](https://a.test/p)',
  '2. [b.test](https://b.test/p)',
  '',
].join('\n')

describe('extending a report', () => {
  it('leaves the prose that was already there exactly as it was', () => {
    // Somebody has read this and may have quoted it.
    const out = extendReport(PARENT, { question: 'What about enterprise tiers?', markdown: '# Enterprise\n\nThey negotiate [3].' }, [
      src(1, 'a.test'),
      src(2, 'b.test'),
      src(3, 'c.test'),
    ])
    expect(out).toContain('Comparable platforms charge per seat [1], and most bundle a number of agents [2].')
  })

  it('says what asked for the new section', () => {
    // A reader coming back next week needs to see that the last paragraphs
    // answer a different question than the top of the document does.
    const out = extendReport(PARENT, { question: 'What about enterprise tiers?', markdown: 'They negotiate [3].' }, [src(3, 'c.test')])
    expect(out).toContain('## Follow-up: What about enterprise tiers?')
  })

  it('drops the follow-up’s own H1, because the document already has one', () => {
    // A second H1 mid-document reads as a new document to every renderer and
    // every table of contents.
    const out = extendReport(PARENT, { question: 'q', markdown: '# Enterprise tiers\n\nThey negotiate [3].' }, [src(3, 'c.test')])
    expect(out.match(/^# /gm)).toHaveLength(1)
    expect(out).toContain('They negotiate [3].')
  })

  it('rebuilds ONE sources section rather than appending a second', () => {
    const out = extendReport(PARENT, { question: 'q', markdown: 'More [3].' }, [src(1, 'a.test'), src(2, 'b.test'), src(3, 'c.test')])
    expect(out.match(/^## Sources$/gm)).toHaveLength(1)
    // And it is the last thing in the document.
    expect(out.trimEnd().endsWith('3. [c.test](https://c.test/p)')).toBe(true)
  })

  it('keeps a parent-only citation OUT of "(consulted)"', () => {
    // THE SUBTLE ONE. `cited` is recomputed over the WHOLE document, so a source
    // the parent cites and the follow-up does not stays a citation. Computing it
    // from the new section alone would quietly demote half the report's
    // references to "consulted".
    const out = extendReport(PARENT, { question: 'q', markdown: 'Only this one matters [3].' }, [
      src(1, 'a.test'),
      src(2, 'b.test'),
      src(3, 'c.test'),
    ])
    expect(out).toContain('1. [a.test](https://a.test/p)\n')
    expect(out).not.toMatch(/1\. \[a\.test\].*consulted/)
    // A source nobody cites is still marked, which is the honest label.
    const out2 = extendReport(PARENT, { question: 'q', markdown: 'Nothing new.' }, [src(1, 'a.test'), src(2, 'b.test'), src(9, 'z.test')])
    expect(out2).toMatch(/9\. \[z\.test\].*\*\(consulted\)\*/)
  })

  it('does not truncate a report that TALKS about sources', () => {
    // The section matcher is anchored to a line start and the end of the
    // document; a loose match would cut the report at the first mention.
    const chatty = ['# Title', '', 'Our sources disagree. See the Sources below.', '', '## Sources', '', '1. [a](https://a.test/p)', ''].join('\n')
    expect(reportBodyOnly(chatty)).toContain('Our sources disagree. See the Sources below.')
    expect(reportBodyOnly(chatty)).not.toContain('1. [a]')
  })

  it('handles a parent that has no sources section at all', () => {
    const out = extendReport('# Title\n\nA claim.', { question: 'q', markdown: 'More.' }, [src(1, 'a.test')])
    expect(out).toContain('A claim.')
    expect(out.match(/^## Sources$/gm)).toHaveLength(1)
  })
})

describe('continuing the parent’s source numbering', () => {
  it('keeps the parent’s indices verbatim and starts new ones above the highest', () => {
    // THIS IS WHAT KEEPS THE OLD TEXT TRUE. Every [n] in the parent's prose
    // points at a row in its list; renumbering would re-aim citations a human
    // already read and believed.
    const reg = SourceRegistry.from([src(1, 'a.test'), src(2, 'b.test')])
    expect(reg.add({ url: 'https://c.test/p', title: 'c', snippet: null })).toBe(3)
    // A URL the parent already has keeps ITS number rather than getting a new one.
    expect(reg.add({ url: 'https://a.test/p', title: 'a', snippet: null })).toBe(1)
  })

  it('does not reuse an index a deleted source left behind', () => {
    // A seeded registry can carry gaps. `size + 1` would hand [3] to a new URL
    // while the parent's prose still cites [3] meaning something else — the
    // quietest possible corruption, and the reason `add` uses highest + 1.
    const reg = SourceRegistry.from([src(1, 'a.test'), src(2, 'b.test'), src(5, 'e.test')])
    expect(reg.add({ url: 'https://new.test/p', title: 'new', snippet: null })).toBe(6)
  })

  it('starts at 1 when there is no parent', () => {
    const reg = new SourceRegistry()
    expect(reg.add({ url: 'https://a.test/p', title: 'a', snippet: null })).toBe(1)
  })
})
