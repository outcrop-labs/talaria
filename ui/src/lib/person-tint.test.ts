import { describe, expect, it } from 'vitest'
import { PERSON_TINTS, personTint } from './person-tint'

describe('personTint', () => {
  it('gives a name one of the palette’s tints', () => {
    for (const name of ['Maya Chen', 'Jon Avery', 'Doug', 'Priya Raman', 'Zoë Ødegård', '李雷', '']) {
      const tint = personTint(name)
      expect(Number.isInteger(tint)).toBe(true)
      expect(tint).toBeGreaterThanOrEqual(0)
      expect(tint).toBeLessThan(PERSON_TINTS)
    }
  })

  it('gives one person the same tint however their name is cased or spaced', () => {
    expect(personTint('  maya   CHEN ')).toBe(personTint('Maya Chen'))
    expect(personTint('Maya\tChen')).toBe(personTint('Maya Chen'))
  })

  it('tells four people in one list apart', () => {
    const tints = ['Maya Chen', 'Jon Avery', 'Doug', 'Priya Raman'].map(personTint)
    expect(new Set(tints).size).toBe(4)
  })

  it('uses the whole palette over many names', () => {
    const tints = new Set(Array.from({ length: 200 }, (_, i) => personTint(`Person ${i}`)))
    expect(tints.size).toBe(PERSON_TINTS)
  })
})
