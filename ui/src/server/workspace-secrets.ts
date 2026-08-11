// THE CREDENTIALS AN AGENT MAY USE WITHOUT EVER READING.
//
// `secret-vault.ts` stops a credential in context from reaching a provider. This
// is the other half: a way for an agent to USE one anyway — push with a PAT,
// pull from a private registry, sign a build — while the value stays on this
// side of the wire.
//
// HOW IT WORKS, in one line: the agent is given a NAME, and the platform
// substitutes the VALUE at the boundary that spends it.
//
//   in context      "you may push with «secret:deploy.github_pat»"
//   on the wire     the same string — the model never holds anything else
//   at the boundary `resolveHandles` swaps it for the real token, once, for a
//                   caller that has been granted it
//
// A DOC, NOT A ROW, because that is how credentials arrive. A deploy needs a
// PAT, a registry password and a signing key TOGETHER; making somebody create
// three unrelated secrets and remember which three go together is how the wrong
// one gets used. A single secret is a doc with one entry, so there is one shape
// to grant, audit and revoke.
//
// TWO LIFETIMES. A `vault` doc belongs to the workspace and persists. A `relay`
// is one-shot: somebody pastes a credential into chat so an agent can do one
// thing with it, and it is consumed on first resolve. The distinction is the
// whole reason `uses_remaining` exists — a relay that outlived its errand would
// be a durable secret nobody remembers creating.
//
// WHAT THIS DELIBERATELY DOES NOT DO: hand a value to anything that will show it
// to a model. `resolveHandles` is for OUTBOUND boundaries — a header, a git
// credential helper, a tool invocation — and its result must never be written
// back into a transcript, a tool RESULT, or a record. The guard's `secret_leak`
// remains the backstop for when something does.
import { db } from './db/pg'
import { open, seal } from './secretbox'

/** `«secret:doc»` or `«secret:doc.entry»`. The doc-only form resolves when the
 *  doc holds exactly one entry, which is what makes a single secret feel like a
 *  single secret while sharing the bundle's machinery. */
const HANDLE = /«secret:([a-z0-9][a-z0-9_-]*)(?:\.([a-z0-9][a-z0-9_-]*))?»/gi

export const handleFor = (doc: string, entry?: string): string => `«secret:${entry ? `${doc}.${entry}` : doc}»`

export type SecretKind = 'vault' | 'relay'

export interface SecretEntryInput {
  key: string
  /** What KIND of credential — shown to humans and written to audit lines. Never
   *  derived from the value, so it cannot leak one character of it. */
  label: string
  value: string
}

export interface SecretDoc {
  id: string
  name: string
  title: string
  kind: SecretKind
  note: string | null
  createdBy: string | null
  createdAt: string
  expiresAt: string | null
  usesRemaining: number | null
  lastUsedAt: string | null
  /** KEYS AND LABELS ONLY. There is no shape of this type that carries a value —
   *  every listing surface, every API response and every log line is built from
   *  this, so the value has nowhere to escape to by accident. */
  entries: Array<{ key: string; label: string }>
  /** Agent models granted use. */
  grants: string[]
}

/** Create a doc. `relay` docs default to a single use, which is what makes them
 *  relays rather than secrets somebody forgot to delete. */
export async function createSecretDoc(input: {
  name: string
  title: string
  entries: SecretEntryInput[]
  kind?: SecretKind
  note?: string | null
  createdBy?: string | null
  expiresAt?: string | null
  uses?: number | null
  grantTo?: string[]
}): Promise<SecretDoc> {
  if (input.entries.length === 0) throw new Error('a secret needs at least one entry')
  const sql = await db()
  const kind: SecretKind = input.kind ?? 'vault'
  const uses = input.uses !== undefined ? input.uses : kind === 'relay' ? 1 : null
  const rows = (await sql`
    insert into workspace_secrets (name, title, kind, note, created_by, expires_at, uses_remaining)
    values (${input.name}, ${input.title}, ${kind}, ${input.note ?? null}, ${input.createdBy ?? null}, ${input.expiresAt ?? null}, ${uses})
    returning id
  `) as unknown as Array<{ id: string }>
  const id = rows[0]!.id
  for (const e of input.entries) {
    await sql`
      insert into workspace_secret_entries (secret_id, key, label, value_cipher)
      values (${id}, ${e.key}, ${e.label}, ${seal(e.value)})
    `
  }
  for (const agent of input.grantTo ?? []) {
    await sql`insert into workspace_secret_grants (secret_id, agent_model, granted_by) values (${id}, ${agent}, ${input.createdBy ?? null})
              on conflict do nothing`
  }
  const doc = await getSecretDoc(input.name)
  if (!doc) throw new Error('secret was created but could not be read back')
  return doc
}

/** One doc, keys and labels only. */
export async function getSecretDoc(name: string): Promise<SecretDoc | null> {
  const sql = await db()
  const rows = (await sql`
    select id, name, title, kind, note, created_by as "createdBy", created_at as "createdAt",
           expires_at as "expiresAt", uses_remaining as "usesRemaining", last_used_at as "lastUsedAt"
      from workspace_secrets where name = ${name}
  `) as unknown as Array<Omit<SecretDoc, 'entries' | 'grants'>>
  const row = rows[0]
  if (!row) return null
  const entries = (await sql`select key, label from workspace_secret_entries where secret_id = ${row.id} order by key`) as unknown as Array<{
    key: string
    label: string
  }>
  const grants = (await sql`select agent_model from workspace_secret_grants where secret_id = ${row.id}`) as unknown as Array<{ agent_model: string }>
  return { ...row, entries, grants: grants.map((g) => g.agent_model) }
}

export async function listSecretDocs(): Promise<SecretDoc[]> {
  const sql = await db()
  const rows = (await sql`select name from workspace_secrets order by created_at desc`) as unknown as Array<{ name: string }>
  const out: SecretDoc[] = []
  for (const r of rows) {
    const doc = await getSecretDoc(r.name)
    if (doc) out.push(doc)
  }
  return out
}

export async function grantSecret(name: string, agentModel: string, grantedBy?: string | null): Promise<void> {
  const sql = await db()
  await sql`
    insert into workspace_secret_grants (secret_id, agent_model, granted_by)
    select id, ${agentModel}, ${grantedBy ?? null} from workspace_secrets where name = ${name}
    on conflict do nothing
  `
}

export async function revokeSecret(name: string, agentModel: string): Promise<void> {
  const sql = await db()
  await sql`
    delete from workspace_secret_grants
     where agent_model = ${agentModel}
       and secret_id in (select id from workspace_secrets where name = ${name})
  `
}

export async function deleteSecretDoc(name: string): Promise<void> {
  const sql = await db()
  await sql`delete from workspace_secrets where name = ${name}`
}

/** Why a handle did not resolve. Reported so an operator can fix it — and so a
 *  caller never has to guess whether an unresolved handle meant "no such secret"
 *  or "not yours". */
export interface UnresolvedHandle {
  handle: string
  reason: 'unknown' | 'not-granted' | 'expired' | 'spent' | 'ambiguous'
}

export interface Resolution {
  text: string
  /** Docs actually spent, for the audit line. Names, never values. */
  used: Array<{ name: string; key: string; label: string }>
  unresolved: UnresolvedHandle[]
}

/** SUBSTITUTE HANDLES FOR VALUES, at an outbound boundary and nowhere else.
 *
 *  `caller` is the agent model asking. A handle it has not been granted does not
 *  resolve, and the difference between "no such secret" and "not yours" is
 *  reported to the OPERATOR rather than to the model — a caller that learns which
 *  names exist has been handed a map of the workspace's credentials.
 *
 *  A RELAY IS SPENT HERE. `uses_remaining` decrements atomically in the same
 *  statement that reads it, so two concurrent tool calls cannot both spend the
 *  last use of a one-shot. */
export async function resolveHandles(text: string, caller: string): Promise<Resolution> {
  const found = [...text.matchAll(HANDLE)]
  if (found.length === 0) return { text, used: [], unresolved: [] }

  const sql = await db()
  const used: Resolution['used'] = []
  const unresolved: UnresolvedHandle[] = []
  let out = text

  for (const m of found) {
    const handle = m[0]
    const docName = (m[1] ?? '').toLowerCase()
    const entryKey = m[2]?.toLowerCase() ?? null

    const rows = (await sql`
      select s.id, s.name, s.kind, s.expires_at as "expiresAt", s.uses_remaining as "usesRemaining",
             (select count(*) from workspace_secret_grants g where g.secret_id = s.id and g.agent_model = ${caller}) as "granted"
        from workspace_secrets s
       where lower(s.name) = ${docName}
    `) as unknown as Array<{ id: string; name: string; kind: SecretKind; expiresAt: string | null; usesRemaining: number | null; granted: string }>
    const doc = rows[0]
    if (!doc) {
      unresolved.push({ handle, reason: 'unknown' })
      continue
    }
    if (Number(doc.granted) === 0) {
      unresolved.push({ handle, reason: 'not-granted' })
      continue
    }
    if (doc.expiresAt && new Date(doc.expiresAt).getTime() <= Date.now()) {
      unresolved.push({ handle, reason: 'expired' })
      continue
    }

    const entries = (await sql`
      select key, label, value_cipher as "cipher" from workspace_secret_entries where secret_id = ${doc.id} order by key
    `) as unknown as Array<{ key: string; label: string; cipher: string }>
    // The doc-only form is only unambiguous when there is one entry. Guessing
    // which of four credentials was meant is how the wrong one gets spent.
    const entry = entryKey ? entries.find((e) => e.key.toLowerCase() === entryKey) : entries.length === 1 ? entries[0] : undefined
    if (!entry) {
      unresolved.push({ handle, reason: entryKey ? 'unknown' : 'ambiguous' })
      continue
    }

    // SPEND IT IN THE SAME STATEMENT THAT CHECKS IT. Two concurrent tool calls
    // must not both take the last use of a one-shot, and a read-then-write would
    // let them.
    if (doc.usesRemaining !== null) {
      const spent = (await sql`
        update workspace_secrets set uses_remaining = uses_remaining - 1, last_used_at = now()
         where id = ${doc.id} and uses_remaining > 0
        returning id
      `) as unknown as Array<{ id: string }>
      if (spent.length === 0) {
        unresolved.push({ handle, reason: 'spent' })
        continue
      }
    } else {
      await sql`update workspace_secrets set last_used_at = now() where id = ${doc.id}`
    }

    let value: string
    try {
      value = open(entry.cipher)
    } catch {
      // Key material changed under us. Refusing is the only safe answer — a
      // half-decrypted credential is worse than none.
      unresolved.push({ handle, reason: 'unknown' })
      continue
    }
    out = out.split(handle).join(value)
    used.push({ name: doc.name, key: entry.key, label: entry.label })
  }

  return { text: out, used, unresolved }
}

/** WHAT AN AGENT IS TOLD IT HAS — names and labels, never values.
 *
 *  This is the string a prompt can carry: it tells a model which handles exist
 *  for it, so it can use one deliberately rather than inventing a name. A model
 *  that has been granted nothing is told nothing, which is also correct. */
export async function grantedHandlesFor(caller: string): Promise<string> {
  const sql = await db()
  const rows = (await sql`
    select s.name, e.key, e.label
      from workspace_secrets s
      join workspace_secret_grants g on g.secret_id = s.id and g.agent_model = ${caller}
      join workspace_secret_entries e on e.secret_id = s.id
     where (s.expires_at is null or s.expires_at > now())
       and (s.uses_remaining is null or s.uses_remaining > 0)
     order by s.name, e.key
  `) as unknown as Array<{ name: string; key: string; label: string }>
  if (rows.length === 0) return ''
  const byDoc = new Map<string, Array<{ key: string; label: string }>>()
  for (const r of rows) byDoc.set(r.name, [...(byDoc.get(r.name) ?? []), { key: r.key, label: r.label }])
  const lines = [...byDoc].map(([name, es]) =>
    es.length === 1 && es[0] ? `${handleFor(name)} (${es[0].label})` : es.map((e) => `${handleFor(name, e.key)} (${e.label})`).join(', '),
  )
  return (
    `Credentials you may USE without seeing: ${lines.join('; ')}. ` +
    'Pass the handle exactly as written wherever the value would go — Talaria substitutes it at the boundary. ' +
    'You will never be shown the value, and a handle you invent resolves to nothing.'
  )
}
