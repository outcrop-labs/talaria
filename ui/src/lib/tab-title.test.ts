import { describe, expect, it } from 'vitest'
import { tabTitle } from './tab-title'

describe('tabTitle', () => {
  it('is the bare product name when nothing is configured', () => {
    expect(tabTitle(null)).toBe('Talaria')
    expect(tabTitle(undefined)).toBe('Talaria')
    expect(tabTitle('')).toBe('Talaria')
  })

  it('appends a configured company name with the plain hyphen', () => {
    expect(tabTitle('Outcrop Labs')).toBe('Talaria - Outcrop Labs')
    expect(tabTitle('Acme & Co')).toBe('Talaria - Acme & Co')
  })

  it('trims — whitespace never renders as an empty suffix', () => {
    expect(tabTitle('  Outcrop Labs  ')).toBe('Talaria - Outcrop Labs')
    expect(tabTitle('   ')).toBe('Talaria')
    expect(tabTitle('\t\n')).toBe('Talaria')
  })
})
