// One-shot rotation of the data-encryption key (DEK). Generates a fresh 256-bit
// DEK and re-encrypts EVERY stored secret under it in a single transaction — the
// operator rotates once and all provider keys, agent secrets, and OAuth tokens
// move to the new key automatically. Optionally re-derives the KEK from new root
// material at the same time (the wrapped DEK is then stored under the new root).
//
// Every cipher column in the database is listed here; add new ones as secrets
// are introduced so rotation stays complete.
import { db } from './db/pg'
import { currentKeyVersion, installActiveKey, loadedVersions, newDek, open, rewrapVersion, sealWith, wrapDekFor } from './secretbox'

/** Every table+column holding a secretbox ciphertext, with its primary key. */
const CIPHER_TARGETS: Array<{ table: string; column: string; pk: string[] }> = [
  { table: 'llm_endpoints', column: 'api_key_cipher', pk: ['id'] },
  { table: 'agent_secrets', column: 'value_enc', pk: ['agent_id', 'name'] },
  { table: 'google_connections', column: 'access_token_enc', pk: ['user_id'] },
  { table: 'google_connections', column: 'refresh_token_enc', pk: ['user_id'] },
  { table: 'google_org_connection', column: 'access_token_enc', pk: ['id'] },
  { table: 'google_org_connection', column: 'refresh_token_enc', pk: ['id'] },
]

// Identifiers here are all code-defined constants, never user input.
const ident = (s: string) => `"${s.replace(/"/g, '""')}"`

export interface RotationResult {
  version: number
  reencrypted: number
  rootRewrapped: boolean
}

/** Rotate the DEK and re-encrypt all secrets. Pass newRootMaterial to also move
 *  the wrapped DEK under a new operator secret (KEK) in the same pass. */
export async function rotateSecrets(newRootMaterial?: string): Promise<RotationResult> {
  const sql = await db()
  const fresh = newDek()
  const nextVersion = currentKeyVersion() + 1
  let reencrypted = 0

  await sql.begin(async (tx) => {
    for (const { table, column, pk } of CIPHER_TARGETS) {
      const cols = [...pk.map(ident), `${ident(column)} as cipher`].join(', ')
      const rows = (await tx.unsafe(
        `select ${cols} from ${ident(table)} where ${ident(column)} is not null`,
      )) as unknown as Array<Record<string, string>>
      for (const row of rows) {
        const resealed = sealWith(fresh, nextVersion, open(row.cipher!)) // decrypt (current) → re-encrypt (new, versioned)
        const whereClause = pk.map((c, i) => `${ident(c)} = $${i + 2}`).join(' and ')
        await tx.unsafe(
          `update ${ident(table)} set ${ident(column)} = $1 where ${whereClause}`,
          [resealed, ...pk.map((c) => row[c]!)],
        )
        reencrypted++
      }
    }
    // If the root secret is changing, re-wrap every RETAINED key version under
    // the new KEK so old versions stay unwrappable (old ciphertext keeps working).
    if (newRootMaterial) {
      for (const v of loadedVersions()) {
        await tx`update secret_keys set wrapped_dek = ${rewrapVersion(v, newRootMaterial)} where version = ${v}`
      }
    }
    // Publish the new active version — prior versions are KEPT (marked inactive)
    // so their ciphertext still decrypts.
    await tx`update secret_keys set active = false where active`
    await tx`insert into secret_keys (version, wrapped_dek, active)
             values (${nextVersion}, ${wrapDekFor(fresh, newRootMaterial)}, true)`
  })

  // Only after the transaction commits do we switch the in-memory keys.
  installActiveKey(fresh, nextVersion, newRootMaterial)
  return { version: nextVersion, reencrypted, rootRewrapped: Boolean(newRootMaterial) }
}
