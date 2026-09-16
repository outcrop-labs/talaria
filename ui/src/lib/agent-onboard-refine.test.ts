// TALA-4 (onboarding surface): the decisions behind the new-agent refine
// feedback. The component wires these in; here every branch is pinned,
// including the ones that must NOT claim a change (untouched identity fields).

import { describe, expect, it } from 'vitest'
import { appliedFields, summarizeSoul } from './agent-onboard-refine'

const order = [
  { label: 'Role', key: 'role' },
  { label: 'Soul', key: 'soul' },
  { label: 'Skills', key: 'skills' },
]

describe('appliedFields', () => {
  it('lists only fields the refine actually changed', () => {
    const fields = appliedFields(
      { role: 'Research Analyst', soul: 'old soul', skills: [] },
      { role: 'Research Analyst', soul: 'new soul', skills: [] },
      order,
    )
    expect(fields).toEqual([{ label: 'Soul', changed: true }])
  })

  it('lists several changed fields in display order', () => {
    const fields = appliedFields(
      { role: 'a', soul: 'old', skills: [] },
      { role: 'b', soul: 'new', skills: [{ name: 'x', content: 'y' }] },
      order,
    )
    expect(fields.map((f) => f.label)).toEqual(['Role', 'Soul', 'Skills'])
  })

  it('says nothing for a refine that touched no tracked field', () => {
    const fields = appliedFields({ role: 'a', soul: 's', skills: [] }, { role: 'a', soul: 's', skills: [] }, order)
    expect(fields).toEqual([])
  })

  it('treats a reordered skill list as a change', () => {
    const fields = appliedFields(
      { role: 'a', soul: 's', skills: [{ name: 'x', content: '1' }, { name: 'y', content: '2' }] },
      { role: 'a', soul: 's', skills: [{ name: 'y', content: '2' }, { name: 'x', content: '1' }] },
      order,
    )
    expect(fields.map((f) => f.label)).toEqual(['Skills'])
  })

  it('ignores keys outside the display order', () => {
    const fields = appliedFields(
      { role: 'a', soul: 's', skills: [], department: 'old' },
      { role: 'a', soul: 's', skills: [], department: 'new' },
      order,
    )
    expect(fields).toEqual([])
  })
})

describe('summarizeSoul', () => {
  it('counts adds and dels against the previous soul', () => {
    const s = summarizeSoul('one\ntwo\nthree', 'one\nTWO\nthree\nfour')
    expect(s.oversized).toBe(false)
    expect(s.adds).toBe(2)
    expect(s.dels).toBe(1)
    expect(s.text).toBe('2 added · 1 removed lines')
  })

  it('uses the singular for a one-line add', () => {
    const s = summarizeSoul('one', 'one\ntwo')
    expect(s.text).toBe('1 added · 0 removed line')
  })

  it('reports no text changes when the soul survived untouched', () => {
    const s = summarizeSoul('same\nlines', 'same\nlines')
    expect(s.text).toBe('no text changes')
    expect(s.adds).toBe(0)
    expect(s.dels).toBe(0)
  })

  it('reads a first soul as "wrote N lines", not a diff from nothing', () => {
    const s = summarizeSoul('', 'first\ndraft')
    expect(s.oversized).toBe(false)
    expect(s.text).toBe('wrote 2 lines')
  })

  it('degrades to an honest "changed" when the soul outgrew the diff', () => {
    // The client diff caps at 2M cells (line-diff.ts): two ~1500-line souls.
    const big = (ch: string) => Array.from({ length: 1500 }, (_, i) => `${ch}${i}`).join('\n')
    const s = summarizeSoul(big('a'), big('b'))
    expect(s.oversized).toBe(true)
    expect(s.text).toBe('changed')
  })
})
