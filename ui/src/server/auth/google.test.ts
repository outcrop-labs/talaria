import { describe, expect, it, vi } from 'vitest'
import { exchangeGoogleCode, orgGoogleLoginAllowed } from './google'

// The org-domain login gate as a contract. The route composes this with the
// connected flag before the allow-list/invite doors; these tests pin the
// policy itself: a wired workspace is domain-members-only, an unwired install
// gates nothing, and a leftover unusable row cannot lock everyone out.

describe('orgGoogleLoginAllowed', () => {
  it('allows everything when no org account is connected', () => {
    expect(orgGoogleLoginAllowed(null, 'anyone@gmail.com')).toBe(true)
    expect(orgGoogleLoginAllowed(undefined, null)).toBe(true)
  })

  it('allows members of the org domain — any local part', () => {
    expect(orgGoogleLoginAllowed('jon@outcroplabs.com', 'priya@outcroplabs.com')).toBe(true)
    expect(orgGoogleLoginAllowed('jon@outcroplabs.com', 'JON@OutcropLabs.com')).toBe(true)
  })

  it('refuses every other domain — even a consumer account, even with matching local part', () => {
    expect(orgGoogleLoginAllowed('jon@outcroplabs.com', 'jon@gmail.com')).toBe(false)
    expect(orgGoogleLoginAllowed('jon@outcroplabs.com', 'friend@othercorp.io')).toBe(false)
  })

  it('refuses a login with no email rather than waving it through', () => {
    expect(orgGoogleLoginAllowed('jon@outcroplabs.com', null)).toBe(false)
  })

  it('gates nothing when the connected row has no usable email', () => {
    // Only reachable as a hand-edited/degenerate row — but a row that cannot
    // name a domain must not become a total lockout.
    expect(orgGoogleLoginAllowed('not-an-email', 'anyone@gmail.com')).toBe(true)
  })
})

// The code exchange's identity checks, against a scripted userinfo response.
// The leaf's fetches are mocked away (the oauth leaf has its own contract
// tests); what these pin is the POLICY on the resolved identity: an email
// Google hasn't verified must never become a Talaria account (#269), and the
// hd restriction keeps holding beside it.
const state = vi.hoisted(() => ({
  info: { sub: 'sub-1', email: 'jon@outcroplabs.com', email_verified: true } as Record<string, unknown>,
  client: { clientId: 'cid', clientSecret: 'sec', hd: null as string | null },
}))

vi.mock('../google/client-config', () => ({
  resolveGoogleClient: async () => state.client,
}))

vi.mock('../google/oauth', () => ({
  AUTH_ENDPOINT: 'https://accounts.google.com/o/oauth2/v2/auth',
  resolveOrigin: () => 'http://localhost',
  exchangeGoogleTokens: async () => ({ tokens: { access_token: 'at' }, info: state.info }),
}))

describe('exchangeGoogleCode', () => {
  it('shapes a verified identity into a Talaria identity', async () => {
    const identity = await exchangeGoogleCode('code', 'http://localhost/cb')
    expect(identity).toMatchObject({ sub: 'google:sub-1', email: 'jon@outcroplabs.com', provider: 'google' })
  })

  it('refuses an email Google has not verified', async () => {
    state.info = { sub: 'sub-2', email: 'forged@outcroplabs.com', email_verified: false }
    await expect(exchangeGoogleCode('code', 'http://localhost/cb')).rejects.toThrow('google email not verified')
  })

  it('treats an absent email_verified claim as unverified — absent is not yes', async () => {
    state.info = { sub: 'sub-3', email: 'claimless@outcroplabs.com' }
    await expect(exchangeGoogleCode('code', 'http://localhost/cb')).rejects.toThrow('google email not verified')
  })

  it('still enforces the hosted-domain restriction on the resolved identity', async () => {
    state.info = { sub: 'sub-4', email: 'jon@gmail.com', email_verified: true, hd: 'gmail.com' }
    state.client = { clientId: 'cid', clientSecret: 'sec', hd: 'outcroplabs.com' }
    await expect(exchangeGoogleCode('code', 'http://localhost/cb')).rejects.toThrow('not in required domain')
    state.client = { clientId: 'cid', clientSecret: 'sec', hd: null }
  })
})
