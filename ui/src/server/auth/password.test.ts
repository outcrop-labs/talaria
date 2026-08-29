import { describe, expect, it } from 'vitest'
import { scryptSync } from 'node:crypto'
import { hashPassword, verifyPasswordHash } from './password'

// The scrypt primitives every stored credential goes through (format
// scrypt$N$r$p$salt$hash, params in-band). These pin: the roundtrip, per-hash
// salting, that an entry verifies with ITS OWN params rather than the
// constants (so a future N-bump keeps old entries working), and that a
// malformed stored value is refused without throwing — verify is on the login
// path, where a 500 is worse than a failed sign-in.

describe('hashPassword / verifyPasswordHash', () => {
  it('roundtrips a password, and only that password', async () => {
    const hash = await hashPassword('hunter2')
    expect(hash.startsWith('scrypt$16384$8$1$')).toBe(true)
    expect(await verifyPasswordHash('hunter2', hash)).toBe(true)
    expect(await verifyPasswordHash('hunter3', hash)).toBe(false)
  })

  it('salts every hash — same password, different entries, both verify', async () => {
    const a = await hashPassword('same-password')
    const b = await hashPassword('same-password')
    expect(a).not.toBe(b)
    expect(await verifyPasswordHash('same-password', a)).toBe(true)
    expect(await verifyPasswordHash('same-password', b)).toBe(true)
  })

  it('verifies an entry against the params it carries, not the constants', async () => {
    // Hand-rolled entry with N=8192 — half the current constant. An entry
    // always verifies as it was hashed.
    const salt = Buffer.from('static-salt-value', 'utf8')
    const key = scryptSync('hunter2', salt, 32, { N: 8192, r: 8, p: 1 })
    const entry = `scrypt$8192$8$1$${salt.toString('base64')}$${key.toString('base64')}`
    expect(await verifyPasswordHash('hunter2', entry)).toBe(true)
    expect(await verifyPasswordHash('hunter3', entry)).toBe(false)
  })

  it('refuses malformed stored values without throwing', async () => {
    const bad = [
      'plaintext-password', // no format at all
      'scrypt$16384$8$1', // wrong arity
      'scrypt$N$8$1$c2FsdA$a2V5', // non-integer params
      'scrypt$0$8$1$c2FsdA$a2V5', // N < 1
      'scrypt$16384$0$1$c2FsdA$a2V5', // r < 1
      'scrypt$16384$8$1$$a2V5', // empty salt
      'scrypt$16384$8$1$c2FsdA$', // empty key
      'bcrypt$10$abcdefghijklmnopqrstuv', // some other scheme's entry
    ]
    for (const stored of bad) expect(await verifyPasswordHash('anything', stored)).toBe(false)
  })
})
