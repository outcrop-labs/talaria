import { describe, expect, it } from 'vitest'
import { orgGoogleLoginAllowed } from './google'

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
