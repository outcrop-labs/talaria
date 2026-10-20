import { describe, expect, it } from 'vitest'
import { STARTER_CARDS, sendsSeed } from './starter-cards'

describe('STARTER_CARDS', () => {
  it('is four cards with unique keys', () => {
    expect(STARTER_CARDS).toHaveLength(4)
    expect(new Set(STARTER_CARDS.map((c) => c.key)).size).toBe(4)
  })

  it('covers the three kinds the harness can start', () => {
    const kinds = STARTER_CARDS.map((c) => c.kind)
    expect(kinds.filter((k) => k === 'plan')).toHaveLength(1)
    expect(kinds.filter((k) => k === 'research')).toHaveLength(1)
    expect(kinds.filter((k) => k === 'work')).toHaveLength(2)
  })

  it('names and describes every card', () => {
    for (const card of STARTER_CARDS) {
      expect(card.title.trim()).not.toBe('')
      expect(card.hint.trim()).not.toBe('')
    }
  })

  // UI-CONVENTIONS: no ellipsis in UI copy, and the seed is copy too — it
  // lands in the transcript as something the person said.
  it('writes plain words, with no ellipsis anywhere', () => {
    for (const card of STARTER_CARDS) {
      for (const text of [card.title, card.hint, card.seed]) {
        expect(text).not.toMatch(/…/)
        expect(text).not.toMatch(/\.\.\./)
      }
    }
  })

  it('carries a seed for every card that sends one, and none for research', () => {
    for (const card of STARTER_CARDS) {
      if (sendsSeed(card)) expect(card.seed.trim().length).toBeGreaterThan(20)
      else expect(card.seed).toBe('')
    }
    expect(STARTER_CARDS.filter((c) => !sendsSeed(c)).map((c) => c.key)).toEqual(['research'])
  })

  // The schedule card promises a control that exists: the clock in the
  // session header (ScheduleButton). If the seed stops saying so, the card is
  // telling somebody to look for something it never pointed at.
  it('points the schedule card at the clock in the header', () => {
    const clock = STARTER_CARDS.find((c) => c.key === 'clock')
    expect(clock?.seed).toMatch(/clock in the header/)
  })
})
