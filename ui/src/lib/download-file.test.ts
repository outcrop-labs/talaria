import { describe, expect, it } from 'vitest'
import { sanitizeFilename } from './download-file'

describe('sanitizeFilename', () => {
  it('passes a normal name through', () => {
    expect(sanitizeFilename('notes.pdf')).toBe('notes.pdf')
  })

  it('strips path and reserved characters so the download never writes a path', () => {
    expect(sanitizeFilename('a/b\\c:d*.pdf')).toBe('a_b_c_d_.pdf')
  })

  it('never returns empty — a blank title still downloads as a file', () => {
    expect(sanitizeFilename('')).toBe('download')
    expect(sanitizeFilename('   ')).toBe('download')
  })
})
