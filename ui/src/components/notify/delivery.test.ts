import { describe, expect, it } from 'vitest'
import { deliveredOn, deliverySummary, kindHint } from './delivery'

describe('deliveredOn', () => {
  it('lists the channels that are on, in menu order', () => {
    expect(deliveredOn({ desktop: true, push: false, email: true }, true)).toEqual(['desktop', 'email'])
  })

  it('leaves email out while the workspace sends none', () => {
    expect(deliveredOn({ desktop: true, push: true, email: true }, false)).toEqual(['desktop', 'push'])
    expect(deliveredOn({ desktop: false, push: false, email: true }, false)).toEqual([])
  })
})

describe('deliverySummary', () => {
  it('says the inbox alone when nothing else is on', () => {
    expect(deliverySummary([])).toBe('Inbox only')
  })

  it('names one channel as the only one', () => {
    expect(deliverySummary(['desktop'])).toBe('Desktop only')
    expect(deliverySummary(['email'])).toBe('Email only')
  })

  it('joins two and three in menu order, whatever order they arrive in', () => {
    expect(deliverySummary(['email', 'desktop'])).toBe('Desktop and email')
    expect(deliverySummary(['push', 'email'])).toBe('Push and email')
    expect(deliverySummary(['email', 'push', 'desktop'])).toBe('Desktop, push and email')
  })
})

describe('kindHint', () => {
  it('uses the short line for a kind whose label leaves a question open', () => {
    expect(kindHint({ id: 'mention', blurb: 'Someone mentioned you, assigned you a ticket or shared a doc with you.' })).toBe(
      'Mentions, assigned tickets and shared docs.',
    )
  })

  it('says nothing under a kind whose label is enough', () => {
    expect(kindHint({ id: 'dm', blurb: 'A teammate or an agent messaged you directly.' })).toBeUndefined()
  })

  it('falls back to the registry sentence for a kind added later', () => {
    expect(kindHint({ id: 'budget_alert', blurb: 'A budget is nearly spent.' })).toBe('A budget is nearly spent.')
  })
})
