// Symmetric encryption for secrets we must store and replay — provider API keys,
// OAuth refresh tokens, per-agent secrets. All AES-256-GCM, which is the
// post-quantum-safe choice for data at rest: Grover's algorithm only halves a
// symmetric key's strength, so AES-256 keeps ~128-bit security (NIST-endorsed as
// quantum-resistant). No asymmetric crypto is used, so Shor's algorithm has
// nothing to attack.
//
// Envelope encryption (KEK → DEK):
//   • KEK (key-encryption key) — derived via scrypt from the root secret
//     (TALARIA_SECRET_KEY, or TALARIA_SECRET_KEY_FILE contents, or AUTH_SECRET).
//     This is the ONLY key material that lives outside the database.
//   • DEK (data-encryption key) — a random 256-bit key that actually encrypts
//     every secret. It is stored in the DB *wrapped* by the KEK, so the key that
//     "unlocks everything" is never in a config file.
//
// Rotation re-generates the DEK and re-encrypts every secret under it in one
// pass (see secret-rotation.ts) — the root secret can be rotated cheaply by
// re-wrapping the DEK. Ciphertext is `v1:<iv>:<tag>:<data>` (KEK-direct legacy)
// or `v2:<iv>:<tag>:<data>` (DEK) — base64url parts.

import { createCipheriv, createDecipheriv, randomBytes, scryptSync } from 'node:crypto'
import { readFileSync } from 'node:fs'
import type { Sql } from 'postgres'

const SALT = 'talaria.secretbox.v1'

// ── KEK: the root of trust, derived from the operator secret ──────────────────
let cachedKek: Buffer | null = null
function kekMaterial(): string {
  let material = process.env.TALARIA_SECRET_KEY || ''
  if (!material && process.env.TALARIA_SECRET_KEY_FILE) {
    // A key-file keeps the root secret out of the app config / repo entirely.
    material = readFileSync(process.env.TALARIA_SECRET_KEY_FILE, 'utf8').trim()
  }
  material ||= process.env.AUTH_SECRET || ''
  if (!material) throw new Error('secretbox: set TALARIA_SECRET_KEY, TALARIA_SECRET_KEY_FILE, or AUTH_SECRET')
  return material
}
function kek(): Buffer {
  if (!cachedKek) cachedKek = scryptSync(kekMaterial(), SALT, 32)
  return cachedKek
}
/** Derive a KEK from arbitrary root material (used when rotating the root). */
export function deriveKek(material: string): Buffer {
  return scryptSync(material, SALT, 32)
}

// ── Low-level AES-256-GCM ─────────────────────────────────────────────────────
function encWith(k: Buffer, plaintext: string, version: 'v1' | 'v2'): string {
  const iv = randomBytes(12)
  const cipher = createCipheriv('aes-256-gcm', k, iv)
  const data = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()])
  const tag = cipher.getAuthTag()
  return [version, iv.toString('base64url'), tag.toString('base64url'), data.toString('base64url')].join(':')
}
function decWith(k: Buffer, token: string): string {
  const parts = token.split(':')
  if (parts.length !== 4 || (parts[0] !== 'v1' && parts[0] !== 'v2')) throw new Error('secretbox: unrecognized token')
  const decipher = createDecipheriv('aes-256-gcm', k, Buffer.from(parts[1]!, 'base64url'))
  decipher.setAuthTag(Buffer.from(parts[2]!, 'base64url'))
  return Buffer.concat([decipher.update(Buffer.from(parts[3]!, 'base64url')), decipher.final()]).toString('utf8')
}

// ── DEK: the data key, wrapped in the DB under the KEK, cached in memory ───────
let cachedDek: { key: Buffer; version: number } | null = null

/** Wrap a DEK for storage (a v1 token whose plaintext is the base64 DEK). */
function wrapDek(dek: Buffer, k: Buffer = kek()): string {
  return encWith(k, dek.toString('base64'), 'v1')
}
function unwrapDek(wrapped: string, k: Buffer = kek()): Buffer {
  return Buffer.from(decWith(k, wrapped), 'base64')
}

/** Load (or, on first run, create) the active DEK. Called once during migration
 *  so seal()/open() stay synchronous in every request path afterwards. */
export async function initSecretbox(sql: Sql): Promise<void> {
  if (cachedDek) return
  const rows = (await sql`
    select version, wrapped_dek as "wrappedDek" from secret_keys where active order by version desc limit 1
  `) as unknown as Array<{ version: number; wrappedDek: string }>
  if (rows[0]) {
    cachedDek = { key: unwrapDek(rows[0].wrappedDek), version: rows[0].version }
    return
  }
  const dek = randomBytes(32) // 256-bit — post-quantum-safe
  await sql`insert into secret_keys (version, wrapped_dek, active) values (1, ${wrapDek(dek)}, true)`
  cachedDek = { key: dek, version: 1 }
}

function dek(): { key: Buffer; version: number } {
  if (!cachedDek) throw new Error('secretbox: not initialized (initSecretbox must run during DB migration)')
  return cachedDek
}

// ── Public API ────────────────────────────────────────────────────────────────
/** Encrypt a UTF-8 string with the current DEK. */
export function seal(plaintext: string): string {
  return encWith(dek().key, plaintext, 'v2')
}
/** Decrypt a token from seal(). Handles legacy v1 (KEK-direct) and v2 (DEK). */
export function open(token: string): string {
  return decWith(token.startsWith('v1:') ? kek() : dek().key, token)
}

// ── Rotation support (used by secret-rotation.ts) ─────────────────────────────
export function currentKeyVersion(): number {
  return dek().version
}
/** Encrypt with an explicit DEK — for re-encrypting rows under a fresh key. */
export function sealWith(dekKey: Buffer, plaintext: string): string {
  return encWith(dekKey, plaintext, 'v2')
}
/** A fresh random 256-bit DEK. */
export function newDek(): Buffer {
  return randomBytes(32)
}
/** Wrap a DEK under the current KEK, or a KEK derived from new root material. */
export function wrapDekFor(dekKey: Buffer, rootMaterial?: string): string {
  return wrapDek(dekKey, rootMaterial ? deriveKek(rootMaterial) : kek())
}
/** Swap the in-memory active DEK (and, if the root changed, the KEK) after a
 *  successful rotation, so subsequent seal()/open() use the new key. */
export function installActiveKey(dekKey: Buffer, version: number, rootMaterial?: string): void {
  cachedDek = { key: dekKey, version }
  if (rootMaterial) cachedKek = deriveKek(rootMaterial)
}
