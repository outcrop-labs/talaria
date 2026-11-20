import { describe, expect, it } from 'vitest'
import { formatUsd } from './format'

describe('formatUsd', () => {
  it('prints the bare form by default, which the board and work screens rely on', () => {
    expect(formatUsd(1453.118)).toBe('$1453.12')
    expect(formatUsd(28.82)).toBe('$28.82')
  })

  it('groups thousands when asked', () => {
    expect(formatUsd(1453.118, { group: true })).toBe('$1,453.12')
    expect(formatUsd(1234567.5, { group: true })).toBe('$1,234,567.50')
    expect(formatUsd(28.82, { group: true })).toBe('$28.82')
  })

  it('keeps the small and empty answers whatever the grouping', () => {
    expect(formatUsd(null, { group: true })).toBe('unpriced')
    expect(formatUsd(0, { group: true })).toBe('$0')
    expect(formatUsd(0.004, { group: true })).toBe('$0.0040')
    expect(formatUsd(0.004, { group: true, unpricedAsWord: true })).toBe('<$0.01')
  })
})
