// Password hashing — the credential primitives behind DB-backed password
// accounts (auth/password-accounts.ts) and the first-run claim
// (auth/claim.ts). scrypt rather than bcrypt/argon2: the API server runs
// under node in dev (vite middleware) and bun in prod, and node:crypto's
// scrypt is the one password hash both runtimes verify with zero
// dependencies. Parameters travel inside the entry, so a future bump keeps
// verifying old hashes.

import { randomBytes, scrypt, timingSafeEqual, type ScryptOptions } from 'node:crypto'

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

// A hash that exists only to be verified and discarded: an unknown email costs
// the caller a full scrypt verify (see verifyPasswordLogin in
// password-accounts.ts), so a miss fails exactly as slowly as a wrong password
// and timing never reveals which emails have accounts. Memoized — the point is
// to spend the same cost every time, not a fresh one each time.
let dummy: Promise<string> | null = null
export function dummyHash(): Promise<string> {
  dummy ??= hashPassword('talaria-account-probe')
  return dummy
}
