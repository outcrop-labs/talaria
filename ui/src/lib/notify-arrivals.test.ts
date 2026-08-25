import { describe, expect, it } from 'vitest'
import { ArrivalTracker } from './notify-arrivals'

const row = (id: string) => ({ id })

describe('ArrivalTracker', () => {
  it('primes on the first non-empty page without surfacing it', () => {
    const t = new ArrivalTracker<{ id: string }>()
    // A reload must not replay the inbox as a burst of toasts.
    expect(t.arrive([row('c'), row('b'), row('a')])).toEqual([])
    expect(t.arrive([row('d'), row('c'), row('b'), row('a')])).toEqual([row('d')])
  })

  it('does not prime on an empty feed: the first row to land is new', () => {
    const t = new ArrivalTracker<{ id: string }>()
    expect(t.arrive([])).toEqual([])
    expect(t.arrive([row('first')])).toEqual([row('first')])
  })

  it('returns new rows oldest first from a newest-first page', () => {
    const t = new ArrivalTracker<{ id: string }>()
    t.arrive([row('a')])
    // Page arrives newest-first (d, c, b); they happened b, c, d — toast
    // them in the order they happened.
    expect(t.arrive([row('d'), row('c'), row('b'), row('a')])).toEqual([row('b'), row('c'), row('d')])
  })

  it('surfaces nothing when the same page refetches', () => {
    const t = new ArrivalTracker<{ id: string }>()
    t.arrive([row('b'), row('a')])
    expect(t.arrive([row('b'), row('a')])).toEqual([])
  })

  it('trims its memory after many arrivals without resurrecting rows', () => {
    const t = new ArrivalTracker<{ id: string }>(5)
    t.arrive([row('seed')])
    for (let i = 0; i < 20; i++) expect(t.arrive([row(`n${i}`), row('seed')])).toEqual([row(`n${i}`)])
    // The set was rebuilt along the way; nothing already seen comes back.
    expect(t.arrive([row('seed'), row('n19')])).toEqual([])
    expect(t.arrive([row('n20'), row('seed')])).toEqual([row('n20')])
  })
})
