import { describe, expect, it, vi } from 'vitest'

// Only the zone resolution is unit-tested here, not the digest itself: driving
// `runDigest` end-to-end needs the mail transport, the approvals census, and
// the queues behind a wall of mocks, and the one behavior this feature adds —
// whose clock a recipient's send hour is read on — is one pure function.
// Everything else about the send loop is unchanged by per-user zones.
vi.mock('./db/pg', () => ({ db: vi.fn() }))
vi.mock('./scheduler', () => ({ registerJob: vi.fn() }))
vi.mock('./notifications', () => ({
  addNotification: vi.fn(),
  NOTIFY_SETTINGS_PATH: '/api/notifications',
  sendGatedMail: vi.fn(),
}))

import { recipientZone, type DigestConfig, type Recipient } from './digest'

const config = (timeZone: string): DigestConfig => ({
  hour: 8,
  timeZone,
  nagAfterMinutes: 240,
  escalateAfterMinutes: 1440,
  listLimit: 5,
})

const person = (timezone: string | null): Recipient =>
  ({ id: 'u1', email: 'a@b.c', name: null, role: 'member', prefs: {}, timezone }) as Recipient

describe('recipientZone', () => {
  it('reads the send hour in the person’s stored zone', () => {
    expect(recipientZone(person('Asia/Tokyo'), config('UTC'))).toBe('Asia/Tokyo')
  })

  it('follows the workspace zone when the person never set one', () => {
    expect(recipientZone(person(null), config('America/Denver'))).toBe('America/Denver')
  })

  it('treats a blank stored zone as unset', () => {
    // A hand-emptied row must not hand Intl a blank and stop every send —
    // same survivability rule as a typo in the org config.
    expect(recipientZone(person('  '), config('UTC'))).toBe('UTC')
  })
})
