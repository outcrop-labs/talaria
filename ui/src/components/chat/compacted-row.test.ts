import { describe, expect, it } from 'vitest'
import { compactedSentence, compactedTitle, showCompactedRow } from './compacted-row'
import type { SessionOverview } from '@/lib/conversations.svelte'

const overview = (over: Partial<SessionOverview> = {}): SessionOverview => ({
  applies: true,
  compacted: 2,
  since: '2026-03-04T09:00:00.000Z',
  sessionId: 'c-1',
  messageCount: 240,
  partial: false,
  reachable: true,
  ...over,
})

// A fixed spelling, so the assertions below say what the sentence is rather
// than re-deriving today's year.
const shortDate = () => 'Mar 4'

describe('showCompactedRow', () => {
  it('draws only when the question arose and something was folded', () => {
    expect(showCompactedRow(overview())).toBe(true)
  })

  it('stays away when the overview does not apply', () => {
    // Not pinned, history no longer fits, agent unreachable — all three
    // arrive as `applies: false` with zeros behind them, and all three mean
    // nobody asked rather than "nothing was compacted".
    expect(showCompactedRow(overview({ applies: false, compacted: 0, since: null }))).toBe(false)
    expect(showCompactedRow(overview({ applies: false, compacted: 3 }))).toBe(false)
  })

  it('stays away when a pinned session has never crossed its threshold', () => {
    expect(showCompactedRow(overview({ compacted: 0 }))).toBe(false)
  })

  it('treats a failed read as nothing to show', () => {
    expect(showCompactedRow(null)).toBe(false)
    expect(showCompactedRow(undefined)).toBe(false)
  })
})

describe('compactedSentence', () => {
  it('names what happened, the count, and the date it now starts at', () => {
    expect(compactedSentence(overview(), shortDate)).toBe(
      'Earlier in this session was compacted by the agent · 2 messages folded · since Mar 4',
    )
  })

  it('says one message in the singular', () => {
    expect(compactedSentence(overview({ compacted: 1 }), shortDate)).toBe(
      'Earlier in this session was compacted by the agent · 1 message folded · since Mar 4',
    )
  })

  it('drops the date clause when there is no surviving timestamp', () => {
    expect(compactedSentence(overview({ since: null }), shortDate)).toBe(
      'Earlier in this session was compacted by the agent · 2 messages folded',
    )
  })

  it('never claims a summary exists', () => {
    // Hermes sends no summary text over the wire — the carrier row is empty.
    // This is the regression guard for copy drifting toward "summarized".
    const s = compactedSentence(overview(), shortDate).toLowerCase()
    expect(s).not.toContain('summar')
    expect(s).not.toContain('recap')
  })

  it('formats the date with the house short spelling by default', () => {
    // Built from the runner's own clock on purpose: `formatShortDate` drops
    // the year for the CURRENT one, so a hardcoded date would quietly change
    // meaning every January.
    const d = new Date()
    d.setMonth(0)
    d.setDate(4)
    const s = compactedSentence(overview({ since: d.toISOString() }))
    expect(s.startsWith('Earlier in this session was compacted by the agent · 2 messages folded · since ')).toBe(true)
    expect(s.endsWith(`since ${d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}`)).toBe(true)
  })
})

describe('compactedTitle', () => {
  it('carries the identifiers the visible line leaves out', () => {
    expect(compactedTitle(overview())).toBe('session c-1 · 240 messages in the session')
  })

  it('leaves out what the server could not tell us', () => {
    expect(compactedTitle(overview({ sessionId: null }))).toBe('240 messages in the session')
    expect(compactedTitle(overview({ messageCount: null }))).toBe('session c-1')
    expect(compactedTitle(overview({ sessionId: null, messageCount: null }))).toBe('')
  })

  it('reports a count of zero rather than dropping it', () => {
    // `0` is a real answer from Hermes; only `null` means it was not read.
    expect(compactedTitle(overview({ messageCount: 0 }))).toBe('session c-1 · 0 messages in the session')
  })
})

describe('compactedSentence — edge spellings', () => {
  const base = { applies: true, compacted: 2, since: null, sessionId: 's', messageCount: null, partial: false, reachable: true }
  it('drops a date it cannot read instead of printing Invalid Date', () => {
    const md = compactedSentence({ ...base, since: '1792201668625' } as never, () => 'never')
    expect(md).not.toContain('since')
    expect(md).not.toContain('Invalid')
  })
  it('says at least when the page was partial', () => {
    expect(compactedSentence({ ...base, partial: true } as never, () => 'x')).toContain('at least 2 messages folded')
  })
})
