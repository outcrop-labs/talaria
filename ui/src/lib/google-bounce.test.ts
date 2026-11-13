import { describe, expect, it } from 'vitest'
import { googleBounceText, googleSaid, readGoogleBounce } from './google-bounce'

const texts = { exchange_failed: 'Google rejected the connection. Try again.', denied: 'Connection cancelled.' }

describe('readGoogleBounce', () => {
  it('reads the status and Google’s code', () => {
    expect(readGoogleBounce('?googleOrg=exchange_failed&reason=invalid_client', 'googleOrg')).toEqual({
      status: 'exchange_failed',
      reason: 'invalid_client',
    })
  })

  it('is null when the page was not a bounce', () => {
    expect(readGoogleBounce('?tab=org', 'googleOrg')).toBeNull()
    expect(readGoogleBounce('', 'google')).toBeNull()
  })

  it('drops a reason that is not a bare OAuth code — the URL is anyone’s to craft', () => {
    expect(readGoogleBounce('?google=exchange_failed&reason=Call%20555-0100', 'google')?.reason).toBeNull()
    expect(readGoogleBounce('?google=exchange_failed&reason=%3Cb%3Ex%3C%2Fb%3E', 'google')?.reason).toBeNull()
    expect(readGoogleBounce(`?google=denied&reason=${'a'.repeat(65)}`, 'google')?.reason).toBeNull()
  })
})

describe('googleBounceText', () => {
  it('quotes Google’s code after the page’s own sentence', () => {
    expect(googleBounceText({ status: 'exchange_failed', reason: 'invalid_client' }, texts)).toBe(
      'Google rejected the connection. Try again. Google said: invalid_client.',
    )
    expect(googleBounceText({ status: 'denied', reason: 'access_denied' }, texts)).toBe(
      'Connection cancelled. Google said: access_denied.',
    )
  })

  it('is the page’s sentence alone when Google said nothing', () => {
    expect(googleBounceText({ status: 'exchange_failed', reason: null }, texts)).toBe(
      'Google rejected the connection. Try again.',
    )
    expect(googleSaid(null)).toBe('')
  })
})
