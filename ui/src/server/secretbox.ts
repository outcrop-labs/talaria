// Symmetric encryption for secrets we must store and replay — provider API keys,
// OAuth refresh tokens, per-agent secrets. All AES-256-GCM, which is the
// post-quantum-safe choice for data at rest: Grover's algorithm only halves a
// symmetric key's strength, so AES-256 keeps ~128-bit security (NIST-endorsed as
// quantum-resistant). No asymmetric crypto is used, so Shor's algorithm has
// nothing to attack.
//
// Envelope encryption (KEK → DEK), versioned so it's safe across processes:
//   • KEK (key-encryption key) — derived via scrypt from the root secret
//     (TALARIA_SECRET_KEY, or TALARIA_SECRET_KEY_FILE contents, or AUTH_SECRET).
//     The ONLY key material outside the DB. Keep it STABLE — if it changes, the
//     wrapped DEKs can't be unwrapped and secrets are unrecoverable.
//   • DEK (data-encryption key) — a random 256-bit key that encrypts every
//     secret. Stored in `secret_keys` *wrapped* by the KEK. Every version is kept
//     forever, so old ciphertext always decrypts.
//
// Ciphertext:
//   v1:<iv>:<tag>:<data>            KEK-direct (legacy; also how a DEK is wrapped)
//   v2:<iv>:<tag>:<data>            DEK, unversioned (legacy — pre-versioning)
//   v2:<ver>:<iv>:<tag>:<data>      DEK version <ver>  ← what seal() writes now
//
// Because every current token names its DEK version, a second app instance or a
// key rotation can NEVER decrypt with the wrong key or orphan a secret: open()
// loads exactly the version the token was sealed with. All parts are base64url.

import { createCipheriv, createDecipheriv, randomBytes, scryptSync } from 'node:crypto'
import { readFileSync } from 'node:fs'
import type { Sql } from 'postgres'

const SALT = 'talaria.secretbox.v1'

// In-memory key material lives on globalThis so a Vite HMR reload of this module
// (dev) doesn't wipe the loaded DEKs while the one-time migration/init stays
// cached — which would leave seal()/open() with no key until a full restart.
const g = globalThis as unknown as {
  __sbKek?: Buffer
  __sbDeks?: Map<number, Buffer>
  __sbActive?: number
  /** Why there is no usable key, if there isn't one. Recorded rather than
   *  thrown: `initSecretbox` runs inside the migration run, so throwing from it
   *  rejects EVERY `db()` call and takes down boards, teams and agents —
   *  none of which touch a secret. It did exactly that once. The diagnosis
   *  belongs to the operations that actually need a key; see `active()`. */
  __sbFailure?: string
}
g.__sbDeks ??= new Map<number, Buffer>()

// ── KEK: the root of trust, derived from the operator secret ──────────────────
function kekMaterial(): string {
  let material = process.env.TALARIA_SECRET_KEY || ''
  if (!material && process.env.TALARIA_SECRET_KEY_FILE) {
    material = readFileSync(process.env.TALARIA_SECRET_KEY_FILE, 'utf8').trim()
  }
  material ||= process.env.AUTH_SECRET || ''
  if (!material) throw new Error('secretbox: set TALARIA_SECRET_KEY, TALARIA_SECRET_KEY_FILE, or AUTH_SECRET')
  return material
}
/** Is a DEDICATED root secret configured, as opposed to borrowing AUTH_SECRET?
 *  Creating a key is allowed only when this is true; unwrapping is not gated,
 *  so databases already sealed under the fallback keep working. */
function explicitRoot(): boolean {
  return Boolean(process.env.TALARIA_SECRET_KEY || process.env.TALARIA_SECRET_KEY_FILE)
}

function kek(): Buffer {
  if (!g.__sbKek) g.__sbKek = scryptSync(kekMaterial(), SALT, 32)
  return g.__sbKek
}
/** Derive a KEK from arbitrary root material (used when rotating the root). */
export function deriveKek(material: string): Buffer {
  return scryptSync(material, SALT, 32)
}

// ── Low-level AES-256-GCM ─────────────────────────────────────────────────────
function encRaw(k: Buffer, plaintext: string): { iv: string; tag: string; data: string } {
  const iv = randomBytes(12)
  const cipher = createCipheriv('aes-256-gcm', k, iv)
  const data = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()])
  return { iv: iv.toString('base64url'), tag: cipher.getAuthTag().toString('base64url'), data: data.toString('base64url') }
}
function decRaw(k: Buffer, iv: string, tag: string, data: string): string {
  const decipher = createDecipheriv('aes-256-gcm', k, Buffer.from(iv, 'base64url'))
  decipher.setAuthTag(Buffer.from(tag, 'base64url'))
  return Buffer.concat([decipher.update(Buffer.from(data, 'base64url')), decipher.final()]).toString('utf8')
}

// ── DEK registry: every version, loaded once, kept in memory ──────────────────

function wrapDek(dek: Buffer, k: Buffer = kek()): string {
  const { iv, tag, data } = encRaw(k, dek.toString('base64'))
  return ['v1', iv, tag, data].join(':')
}
function unwrapDek(wrapped: string, k: Buffer = kek()): Buffer {
  const [, iv, tag, data] = wrapped.split(':')
  return Buffer.from(decRaw(k, iv!, tag!, data!), 'base64')
}

/** Load EVERY DEK version into memory (unwrapping each with the KEK), or create
 *  the first one. Called during migration, so seal()/open() stay synchronous. */
export async function initSecretbox(sql: Sql): Promise<void> {
  const rows = (await sql`
    select version, wrapped_dek as "wrappedDek", active from secret_keys order by version asc
  `) as unknown as Array<{ version: number; wrappedDek: string; active: boolean }>

  if (rows.length === 0) {
    // The FIRST key is only ever created under an EXPLICIT root secret. The
    // AUTH_SECRET fallback below stays for databases already sealed with it,
    // but sealing something NEW under a value whose own documentation calls it
    // safe to rotate is how a database ends up unable to read itself.
    //
    // It is also how the two run modes diverged: `vite dev` loads ui/.env and
    // sees TALARIA_SECRET_KEY, a bare `node server-entry.js` did not — and
    // `talaria setup` generates the two secrets as separate randoms, so whichever
    // happened to be visible when this row was written became the key. Refusing
    // here means a database can never be created under an accident.
    if (!explicitRoot()) {
      // Recorded, not thrown — same reason as below. A fresh install with no
      // root secret should still come up, show its UI and say what to set; it
      // should not 500 every endpoint including the ones that would tell you.
      g.__sbFailure =
        "secretbox: refusing to create this database's encryption key from AUTH_SECRET. " +
        'Set TALARIA_SECRET_KEY (or TALARIA_SECRET_KEY_FILE) to a dedicated, stable value and keep it for the life of the database — ' +
        'every provider key, agent secret and OAuth token will be sealed with it, and there is no recovery if it changes. ' +
        'Generate one with: openssl rand -base64 48   (see docs/ENCRYPTION.md)'
      console.error(`[secretbox] ${g.__sbFailure}`)
      return
    }
    const dek = randomBytes(32) // 256-bit
    await sql`insert into secret_keys (version, wrapped_dek, active) values (1, ${wrapDek(dek)}, true)`
    g.__sbDeks!.set(1, dek)
    g.__sbActive = 1
    // Clear any recorded diagnosis: we just minted a key we can read, so the
    // old one is stale. Without this, an instance that failed to unwrap and then
    // had its keys cleared (Admin → Secrets, or `talaria reset secrets`) would keep throwing
    // the previous boot's message at every seal() despite being healthy.
    g.__sbFailure = undefined
    return
  }
  // Sealed under the fallback already: works, but one AUTH_SECRET rotation from
  // being unreadable. Say so every boot until it is pinned.
  if (!explicitRoot()) {
    console.warn(
      '[secretbox] this database is sealed with AUTH_SECRET because TALARIA_SECRET_KEY is not set. ' +
        'Rotating AUTH_SECRET will make every stored secret unrecoverable. ' +
        'Pin it: set TALARIA_SECRET_KEY to the CURRENT AUTH_SECRET value (`bun talaria setup` does this for you).',
    )
  }
  for (const r of rows) {
    if (g.__sbDeks!.has(r.version)) continue
    try {
      g.__sbDeks!.set(r.version, unwrapDek(r.wrappedDek))
      if (r.active) g.__sbActive = r.version
    } catch {
      // Loud, but don't brick the app: a version we can't unwrap means the root
      // secret changed. Its ciphertext will fail to decrypt (callers fall back);
      // secrets sealed under loadable versions keep working.
      console.error(
        `[secretbox] cannot unwrap data key v${r.version} — TALARIA_SECRET_KEY/AUTH_SECRET differs from when it was created. ` +
          `Restore the original root secret to recover secrets sealed with v${r.version}.`,
      )
    }
  }
  // SOME versions unwrapping and others not is survivable and deliberate (see
  // the catch above): the readable ones keep working and callers fall back on
  // the rest. NONE unwrapping is not survivable — every seal() and open() in
  // the process will throw, and it will throw as "secretbox: not initialized",
  // which names the wrong cause and sends whoever reads it looking at the
  // migration runner instead of at their root secret.
  //
  // So refuse to boot, here, while we still know why. This database has keys;
  // this process cannot read any of them; the only fix is the operator's.
  if (!g.__sbActive && g.__sbDeks!.size === 0) {
    // RECORDED, NOT THROWN. This function runs inside the migration run, so a
    // throw here rejects every db() call in the process — boards, teams, agents
    // and the whole app, none of which need a key. An earlier version of this
    // check did throw, and turned "secrets are unreadable" into "nothing
    // works", which is both worse and much harder to diagnose from the browser.
    g.__sbFailure =
      `secretbox: this database has ${rows.length} data key(s) and none can be unwrapped with the current root secret. ` +
      'TALARIA_SECRET_KEY (or TALARIA_SECRET_KEY_FILE, or AUTH_SECRET if neither is set) is not the value these keys were created with. ' +
      'Restore the original root secret — every provider key, agent secret and OAuth token in this database is sealed with it. ' +
      'If AUTH_SECRET is doing this job, set TALARIA_SECRET_KEY to that same value and stop rotating AUTH_SECRET (see docs/ENCRYPTION.md).'
    console.error(`[secretbox] ${g.__sbFailure}`)
    return
  }
  g.__sbFailure = undefined
  if (!g.__sbActive) g.__sbActive = Math.max(...g.__sbDeks!.keys())
}

function dekFor(version: number): Buffer {
  const key = g.__sbDeks!.get(version)
  if (!key) throw new Error(`secretbox: data key v${version} not loaded (rotated by another process? restart to pick it up)`)
  return key
}
function active(): { key: Buffer; version: number } {
  // The recorded diagnosis wins: it says the root secret changed, which is the
  // real cause, instead of pointing at the migration runner that ran fine.
  if (g.__sbFailure) throw new Error(g.__sbFailure)
  if (!g.__sbActive) throw new Error('secretbox: not initialized (initSecretbox must run during DB migration)')
  return { key: dekFor(g.__sbActive), version: g.__sbActive }
}

// ── Public API ────────────────────────────────────────────────────────────────
/** Encrypt with the active DEK; the token records its version. */
export function seal(plaintext: string): string {
  const { key, version } = active()
  const { iv, tag, data } = encRaw(key, plaintext)
  return ['v2', String(version), iv, tag, data].join(':')
}
/** Decrypt any token this module has produced. */
export function open(token: string): string {
  const p = token.split(':')
  if (p[0] === 'v1') return decRaw(kek(), p[1]!, p[2]!, p[3]!)
  if (p[0] === 'v2' && p.length === 5) return decRaw(dekFor(Number(p[1])), p[2]!, p[3]!, p[4]!) // versioned
  if (p[0] === 'v2' && p.length === 4) return decRaw(active().key, p[1]!, p[2]!, p[3]!) // legacy unversioned
  throw new Error('secretbox: unrecognized token')
}

// ── Introspection (used by secret-health.ts) ──────────────────────────────────
/** Can this process read this specific token? Cheap where the token says so.
 *
 *  A `v2:<ver>:…` token names its own DEK version, so the answer is a Map
 *  lookup — no crypto, safe to call once per row of an inventory. v1 (KEK-direct)
 *  and legacy unversioned v2 name no key, so the only honest answer is to try;
 *  both predate versioning and are rare. */
export function tokenReadable(token: string | null | undefined): boolean {
  if (!token) return false
  const p = token.split(':')
  if (p[0] === 'v2' && p.length === 5) return g.__sbDeks!.has(Number(p[1]))
  try {
    open(token)
    return true
  } catch {
    return false
  }
}

/** Why there is no usable key, if there isn't one — the diagnosis `active()`
 *  throws. Returns null when the secretbox is healthy. */
export function secretboxFailure(): string | null {
  return g.__sbFailure ?? null
}

/** Where the root secret is coming from. `fallback` is the dangerous one: the
 *  database is sealed with AUTH_SECRET, whose own documentation calls it safe
 *  to rotate. `absent` means neither is set and nothing can be sealed at all. */
export function rootSource(): { via: 'env' | 'file' | 'fallback' | 'absent'; name: string } {
  if (process.env.TALARIA_SECRET_KEY) return { via: 'env', name: 'TALARIA_SECRET_KEY' }
  if (process.env.TALARIA_SECRET_KEY_FILE) return { via: 'file', name: process.env.TALARIA_SECRET_KEY_FILE }
  if (process.env.AUTH_SECRET) return { via: 'fallback', name: 'AUTH_SECRET' }
  return { via: 'absent', name: '' }
}

/** The active DEK version, or null when there isn't one. Unlike
 *  `currentKeyVersion()` this never throws — it is a status read, and a status
 *  read that throws on an unhealthy instance is useless exactly when needed. */
export function activeKeyVersion(): number | null {
  return g.__sbActive ?? null
}

// ── Rotation support (used by secret-rotation.ts) ─────────────────────────────
export function currentKeyVersion(): number {
  return active().version
}
/** A fresh random 256-bit DEK. */
export function newDek(): Buffer {
  return randomBytes(32)
}
/** Every DEK version currently loaded (for re-wrapping on a root-key rotation). */
export function loadedVersions(): number[] {
  return [...g.__sbDeks!.keys()]
}
/** Re-wrap an existing version's DEK under a KEK derived from new root material. */
export function rewrapVersion(version: number, rootMaterial: string): string {
  return wrapDek(dekFor(version), deriveKek(rootMaterial))
}
/** Encrypt with an explicit DEK + version — for re-encrypting under a new key. */
export function sealWith(dekKey: Buffer, version: number, plaintext: string): string {
  const { iv, tag, data } = encRaw(dekKey, plaintext)
  return ['v2', String(version), iv, tag, data].join(':')
}
/** Wrap a DEK under the current KEK, or a KEK derived from new root material. */
export function wrapDekFor(dekKey: Buffer, rootMaterial?: string): string {
  return wrapDek(dekKey, rootMaterial ? deriveKek(rootMaterial) : kek())
}
/** Register a freshly-rotated DEK as active (keeping all prior versions). If the
 *  root changed, the KEK moves too — every retained version stays wrapped under
 *  whatever the DB holds, so re-wrap them there in the same transaction. */
export function installActiveKey(dekKey: Buffer, version: number, rootMaterial?: string): void {
  if (rootMaterial) g.__sbKek = deriveKek(rootMaterial)
  g.__sbDeks!.set(version, dekKey)
  g.__sbActive = version
}
