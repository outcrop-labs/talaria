import { describe, expect, it } from 'vitest'
import { moveChangeLines, moveDestinations, moveLosesSomething, type MoveSummary } from './ticket-move'

const board = (id: string, name: string, role: string, archivedAt: string | null = null) => ({ id, name, role, archivedAt })

const summary = (over: Partial<MoveSummary> = {}): MoveSummary => ({
  preview: true,
  fromBoardId: 'a',
  fromBoardName: 'Platform',
  toBoardId: 'b',
  toBoardName: 'Ops',
  fromRef: 'PLAT-118',
  toRef: 'OPS-7',
  fromStatus: 'in_progress',
  status: 'doing',
  statusLabel: 'Doing',
  statusHow: 'same-category',
  labelsKept: [],
  labelsDropped: [],
  unassigned: [],
  parentDetached: false,
  approvalCleared: false,
  dormantWatchers: [],
  dependencies: 0,
  ...over,
})

describe('moveDestinations', () => {
  it('offers only live boards the person can add tickets to, never the one it is on', () => {
    const boards = [
      board('a', 'Platform', 'owner'),
      board('c', 'Zeta', 'editor'),
      board('b', 'Ops', 'owner'),
      board('v', 'Viewing', 'viewer'),
      board('x', 'Retired', 'owner', '2026-10-01T00:00:00Z'),
    ]
    expect(moveDestinations(boards, 'a').map((b) => b.id)).toEqual(['b', 'c'])
  })

  it('is empty when there is nowhere to go', () => {
    expect(moveDestinations([board('a', 'Platform', 'owner')], 'a')).toEqual([])
  })
})

describe('moveChangeLines', () => {
  it('says the new ref and that the old one still resolves, then the column', () => {
    expect(moveChangeLines(summary())).toEqual([
      'Its ref changes from PLAT-118 to OPS-7 — the old ref still opens it.',
      'It lands in Doing (the closest column there).',
    ])
  })

  it('names every label dropped and every person unassigned', () => {
    const lines = moveChangeLines(
      summary({ labelsDropped: ['billing', 'urgent'], unassigned: ['user:u1', 'engineer-engineering'] }),
      (id) => (id === 'user:u1' ? 'Ann' : id),
    )
    expect(lines).toContain('Labels dropped — Ops has no billing, urgent.')
    expect(lines).toContain('Unassigned — not on Ops: Ann, engineer-engineering.')
  })

  it('says a single dropped label in the singular', () => {
    expect(moveChangeLines(summary({ labelsDropped: ['billing'] }))).toContain('Label dropped — Ops has no billing.')
  })

  it('names the parent, the sign-off and the watchers that do not travel', () => {
    const lines = moveChangeLines(summary({ parentDetached: true, approvalCleared: true, dormantWatchers: ['bo@x'] }))
    expect(lines).toContain('It leaves its parent ticket behind and becomes a top-level ticket.')
    expect(lines).toContain('Its sign-off is cleared — Ops’s reviewers sign it off.')
    expect(lines).toContain('Watchers who cannot see Ops stop hearing about it: bo@x.')
  })

  it('words the column by how it was chosen', () => {
    expect(moveChangeLines(summary({ statusHow: 'requested' }))[1]).toBe('It lands in Doing.')
    expect(moveChangeLines(summary({ statusHow: 'intake', statusLabel: 'Triage' }))[1]).toBe(
      'It lands in Triage (its intake — no matching column).',
    )
  })
})

describe('moveLosesSomething', () => {
  it('is false for a clean move and true when anything does not travel', () => {
    expect(moveLosesSomething(summary())).toBe(false)
    expect(moveLosesSomething(summary({ labelsDropped: ['x'] }))).toBe(true)
    expect(moveLosesSomething(summary({ unassigned: ['a'] }))).toBe(true)
    expect(moveLosesSomething(summary({ parentDetached: true }))).toBe(true)
  })
})
