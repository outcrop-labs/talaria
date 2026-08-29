// Username / password provider.
//
// An AUTH_USERS password may be plaintext (transition — env.ts warns at boot)
// or `scrypt$…`-hashed via hashPassword(). scrypt rather than bcrypt/argon2:
// the API server runs under node in dev (vite middleware) and bun in prod, and
// node:crypto's scrypt is the one password hash both runtimes verify with zero
// dependencies. Parameters travel inside the entry, so a future bump keeps
// verifying old hashes.

import { randomBytes, scrypt, timingSafeEqual, type ScryptOptions } from 'node:crypto'
import { getAuthConfig } from './config'
import { isHashedCredential } from '../env'
import type { Identity } from '../users'

export type LoginResult = Identity & { provider: 'password' }

const scryptAsync = (password: string, salt: Buffer, keylen: number, opts: ScryptOptions): Promise<Buffer> =>
  new Promise((resolve, reject) => scrypt(password, salt, keylen, opts, (err, key) => (err ? reject(err) : resolve(key))))

// OWASP interactive-login parameters; encoded per-entry (see hashPassword).
const SCRYPT_N = 16384
const SCRYPT_R = 8
const SCRYPT_P = 1
const KEYLEN = 32

/** Hash a password for storage: `scrypt$N$r$p$salt$hash` (base64 salt + key). */
export async function hashPassword(password: string): Promise<string> {
  const salt = randomBytes(16)
  const key = await scryptAsync(password, salt, KEYLEN, { N: SCRYPT_N, r: SCRYPT_R, p: SCRYPT_P })
  return `scrypt$${SCRYPT_N}$${SCRYPT_R}$${SCRYPT_P}$${salt.toString('base64')}$${key.toString('base64')}`
}

/** Verify a password against a `scrypt$…` entry. Parameters are read from the
 *  entry, not the constants — an entry always verifies as it was hashed. */
export async function verifyPasswordHash(password: string, stored: string): Promise<boolean> {
  const parts = stored.split('$')
  if (parts.length !== 6 || parts[0] !== 'scrypt') return false
  const N = Number(parts[1])
  const r = Number(parts[2])
  const p = Number(parts[3])
  if (!Number.isInteger(N) || !Number.isInteger(r) || !Number.isInteger(p) || N < 1 || r < 1 || p < 1) return false
  const saltB64 = parts[4]
  const keyB64 = parts[5]
  if (!saltB64 || !keyB64) return false
  const salt = Buffer.from(saltB64, 'base64')
  const expected = Buffer.from(keyB64, 'base64')
  if (!salt.length || !expected.length) return false
  const key = await scryptAsync(password, salt, expected.length, { N, r, p, maxmem: 128 * N * r * 2 })
  return key.length === expected.length && timingSafeEqual(key, expected)
}

// The hashed-format detector lives in env.ts (the app-import-free leaf) so its
// boot warning and this verify path can never drift apart.
export { isHashedCredential }

function constantTimeEquals(a: string, b: string): boolean {
  const ab = Buffer.from(a)
  const bb = Buffer.from(b)
  if (ab.length !== bb.length) return false
  return timingSafeEqual(ab, bb)
}

/** Verify credentials against AUTH_USERS (hashed or plaintext entries, mixed
 *  freely). Returns the identity or null. */
export async function verifyPasswordLogin(username: string, password: string): Promise<LoginResult | null> {
  const cfg = getAuthConfig()
  if (!cfg.password.enabled) return null

  // Walk every user so timing doesn't leak which usernames exist — including
  // a hash verify per hashed row on every attempt, matched row or not. That
  // keeps the walk flat (N verifies whether the username exists or not);
  // AUTH_USERS is a short break-glass list, so paying N×scrypt per attempt is
  // the right trade against a timing oracle.
  let matched: LoginResult | null = null
  for (const u of cfg.password.users) {
    const userOk = constantTimeEquals(u.username.toLowerCase(), username.trim().toLowerCase())
    const passOk = isHashedCredential(u.password)
      ? await verifyPasswordHash(password, u.password)
      : constantTimeEquals(u.password, password)
    if (userOk && passOk) {
      matched = {
        sub: `password:${u.username.toLowerCase()}`,
        email: u.username.includes('@') ? u.username.toLowerCase() : null,
        name: u.username,
        picture: null,
        provider: 'password' as const,
      }
    }
  }
  return matched
}
