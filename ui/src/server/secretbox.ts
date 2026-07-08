// Symmetric encryption for secrets we must store and later replay — OAuth
// refresh tokens, above all. Unlike API keys (where we persist only the env-var
// NAME), a per-user refresh token is a live credential minted at runtime: there
// is no name to point at, so it has to live in the database. It lives there
// encrypted.
//
// AES-256-GCM (authenticated) with a key derived from AUTH_SECRET via scrypt, so
// no new operator config is required. Set TALARIA_SECRET_KEY to override the
// source key material. Ciphertext is stored as `v1:<iv>:<tag>:<data>` (base64url
// parts) — the version tag lets us rotate the scheme later.

import { createCipheriv, createDecipheriv, randomBytes, scryptSync } from 'node:crypto'

const VERSION = 'v1'
// Fixed salt: the derived key must be stable across restarts to decrypt old
// rows. Confidentiality rests on AUTH_SECRET, not on salt secrecy.
const SALT = 'talaria.secretbox.v1'

let cachedKey: Buffer | null = null

function key(): Buffer {
  if (cachedKey) return cachedKey
  const material = process.env.TALARIA_SECRET_KEY || process.env.AUTH_SECRET || ''
  if (!material) {
    throw new Error('secretbox: AUTH_SECRET (or TALARIA_SECRET_KEY) must be set to encrypt secrets')
  }
  cachedKey = scryptSync(material, SALT, 32)
  return cachedKey
}

/** Encrypt a UTF-8 string. Returns an opaque, self-describing token. */
export function seal(plaintext: string): string {
  const iv = randomBytes(12)
  const cipher = createCipheriv('aes-256-gcm', key(), iv)
  const data = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()])
  const tag = cipher.getAuthTag()
  return [VERSION, iv.toString('base64url'), tag.toString('base64url'), data.toString('base64url')].join(':')
}

/** Decrypt a token produced by seal(). Throws on tamper or wrong key. */
export function open(token: string): string {
  const parts = token.split(':')
  if (parts.length !== 4 || parts[0] !== VERSION) throw new Error('secretbox: unrecognized token')
  const ivB = parts[1] as string
  const tagB = parts[2] as string
  const dataB = parts[3] as string
  const decipher = createDecipheriv('aes-256-gcm', key(), Buffer.from(ivB, 'base64url'))
  decipher.setAuthTag(Buffer.from(tagB, 'base64url'))
  return Buffer.concat([decipher.update(Buffer.from(dataB, 'base64url')), decipher.final()]).toString('utf8')
}
