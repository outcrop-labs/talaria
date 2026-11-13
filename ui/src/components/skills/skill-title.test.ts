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

  it('leaves words with a vowel, a y or a digit in sentence case', () => {
    expect(skillTitle('edi-204')).toBe('Edi 204')
    expect(skillTitle('my-notes')).toBe('My notes')
    expect(skillTitle('v2')).toBe('V2')
  })

  it('keeps a name it cannot split', () => {
    expect(skillTitle('')).toBe('')
    expect(skillTitle('--')).toBe('--')
  })
})
