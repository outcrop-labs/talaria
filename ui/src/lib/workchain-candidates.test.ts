import { describe, expect, it } from 'vitest'
import { filterCandidates, type WorkchainCandidate } from '@/lib/workchain-rules'

// Pure-helper tests for the "+ Add ticket" picker's filter: which of the
// board's unchained tickets a draft leaves in the list. The match is
// case-insensitive over ticketRef and title; a blank (or all-whitespace)
// draft shows every candidate. The candidate list itself stays the
// caller's — these tests pin only the narrowing.

const candidate = (id: string, title: string, ticketRef: string | null): WorkchainCandidate => ({
  id,
  ticketRef,
  title,
})

const rows = [
  candidate('t_1', 'Wire the power feed', 'ENG-41'),
  candidate('t_2', 'Ship the changelog', null),
  candidate('t_3', 'Power audit', 'OPS-7'),
]

describe('filterCandidates', () => {
  it('a blank draft shows every candidate', () => {
    expect(filterCandidates(rows, '')).toEqual(rows)
  })

  it('whitespace-only drafts are blank drafts', () => {
    expect(filterCandidates(rows, '   ')).toEqual(rows)
  })

  it('the match is case-insensitive over the title', () => {
    expect(filterCandidates(rows, 'POWER')).toEqual([rows[0], rows[2]])
  })

  it('the match also reads the ticketRef', () => {
    expect(filterCandidates(rows, 'eng-41')).toEqual([rows[0]])
  })

  it('a draft matching nothing leaves nothing', () => {
    expect(filterCandidates(rows, 'zzz')).toEqual([])
  })

  it('a trimmed draft filters — the trim is the filter’s job', () => {
    expect(filterCandidates(rows, '  ship  ')).toEqual([rows[1]])
  })
})
