import { describe, expect, it } from 'vitest'
import { accessLossMessage, lossName } from './access-loss'

describe('lossName', () => {
  it('prefers the name, trimmed', () => {
    expect(lossName({ name: '  Ann  ', email: 'ann@x', userId: 'u1' })).toBe('Ann')
  })

  it('falls back to the email when the name is blank', () => {
    expect(lossName({ name: '   ', email: 'ann@x', userId: 'u1' })).toBe('ann@x')
  })

  it('falls back to the user id when neither name nor email exists', () => {
    expect(lossName({ name: null, email: null, userId: 'u1' })).toBe('u1')
  })
})

describe('accessLossMessage', () => {
  it('says so plainly when nobody loses access', () => {
    expect(accessLossMessage([])).toBe('Nobody loses access.')
  })

  it('bullets each person and states when the revocation lands', () => {
    const body = accessLossMessage(['Ann', 'bob@x'])
    expect(body).toBe('• Ann\n• bob@x\n\nThey lose access the moment the move lands.')
  })
})
