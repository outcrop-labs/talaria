import { describe, expect, it } from 'vitest'
import { skillTitle } from './skill-title'

describe('skillTitle', () => {
  it('reads a dashed name as a sentence-case title', () => {
    expect(skillTitle('load-matching')).toBe('Load matching')
    expect(skillTitle('appointment_scheduling')).toBe('Appointment scheduling')
  })

  it('capitalises a word with no vowel as an initialism', () => {
    expect(skillTitle('tms-migrations')).toBe('TMS migrations')
    expect(skillTitle('pdf')).toBe('PDF')
  })

  it('capitalises a known initialism that has a vowel', () => {
    expect(skillTitle('bol-check')).toBe('BOL check')
    expect(skillTitle('edi-204')).toBe('EDI 204')
    expect(skillTitle('carrier-coi-review')).toBe('Carrier COI review')
    for (const word of ['eta', 'pod', 'tms', 'pdf', 'api', 'sql', 'csv', 'coi']) expect(skillTitle(word)).toBe(word.toUpperCase())
  })

  it('leaves an ordinary word, a y or a digit in sentence case', () => {
    expect(skillTitle('dot-inspection')).toBe('Dot inspection')
    expect(skillTitle('podcast-notes')).toBe('Podcast notes')
    expect(skillTitle('my-notes')).toBe('My notes')
    expect(skillTitle('v2')).toBe('V2')
  })

  it('keeps a name it cannot split', () => {
    expect(skillTitle('')).toBe('')
    expect(skillTitle('--')).toBe('--')
  })
})
