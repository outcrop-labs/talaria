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
import { randomUUID } from 'node:crypto'
import { db } from './db/pg'
import { open, seal } from './secretbox'

/** `«secret:doc»` or `«secret:doc.entry»`. The doc-only form resolves when the
 *  doc holds exactly one entry, which is what makes a single secret feel like a
 *  single secret while sharing the bundle's machinery. */
const HANDLE = /«secret:([a-z0-9][a-z0-9_-]*)(?:\.([a-z0-9][a-z0-9_-]*))?»/gi

export const handleFor = (doc: string, entry?: string): string => `«secret:${entry ? `${doc}.${entry}` : doc}»`

/** Does this text name a handle at all? Cheap enough to ask on every turn, which
 *  is what the chat path does before spending a word of prompt on explaining the
 *  mechanism. Built fresh rather than reusing HANDLE: a `/g` regex carries
 *  `lastIndex` between calls, so `.test()` on the shared one answers differently
 *  depending on what asked before it. */
export const mentionsHandle = (text: string): boolean => new RegExp(HANDLE.source, 'i').test(text)

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

/** HOW LONG AN UNSPENT RELAY LIVES.
 *
 *  A relay is for the errand happening in this conversation, now. If the agent
 *  did not spend it while somebody was sitting there watching, the safe answer
 *  is to hand it over again rather than to leave a credential in the workspace
 *  that nobody remembers pasting. An hour is long enough for a queued turn and
 *  short enough that a changed mind costs nothing.
 *
 *  Anything that genuinely needs to outlive the conversation is a vault doc, and
 *  that is a deliberate trip through the admin panel. */
export const RELAY_TTL_MS = 60 * 60 * 1000

/** MINT A ONE-SHOT FROM A CONVERSATION.
 *
 *  The whole point is the asymmetry: the VALUE arrives here, over one request,
 *  and what goes back is a NAME. The caller — a chat composer — never holds
 *  anything it could paste into a transcript by accident, because the only thing
 *  it was given is the handle.
 *
 *  Granted to exactly one agent, spendable exactly once, expiring within the
 *  hour. Each of those three is a separate bound, and a relay has to clear all
 *  of them: an errand that needs two uses is an errand somebody should watch
 *  twice. */
export async function mintRelay(input: {
  label: string
  value: string
  agentModel: string
  createdBy?: string | null
  note?: string | null
}): Promise<{ name: string; handle: string; label: string; expiresAt: string }> {
  // Random, not derived from the label: a guessable name is one another agent
  // could ask for, and the grant check is the only thing that would stop it.
  const name = `relay-${randomUUID().replace(/-/g, '').slice(0, 12)}`
  const expiresAt = new Date(Date.now() + RELAY_TTL_MS).toISOString()
  await createSecretDoc({
    name,
    title: input.label,
    kind: 'relay',
    // ONE ENTRY, so the bare `«secret:relay-…»` form resolves — a person copying
    // a handle out of a chat message should not have to also copy a key.
    entries: [{ key: 'value', label: input.label, value: input.value }],
    note: input.note ?? null,
    createdBy: input.createdBy ?? null,
    expiresAt,
    uses: 1,
    grantTo: [input.agentModel],
  })
  return { name, handle: handleFor(name), label: input.label, expiresAt }
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
    let exhausted = false
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
      exhausted = doc.usesRemaining - 1 <= 0
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

    // A CREDENTIAL WITH NO USES LEFT IS DESTROYED, NOT RETAINED. `uses_remaining`
    // already refuses to resolve it, so keeping the ciphertext buys nothing and
    // costs the one thing worth protecting: a one-shot somebody pasted into chat
    // this morning should not still be recoverable from a database dump tonight.
    // The row survives, so the audit trail — who minted it, who spent it, when —
    // survives with it. Emptied rather than deleted because that history is the
    // only reason to keep a spent relay at all.
    if (exhausted) {
      await sql`update workspace_secret_entries set value_cipher = '' where secret_id = ${doc.id}`
    }
    used.push({ name: doc.name, key: entry.key, label: entry.label })
  }

  return { text: out, used, unresolved }
}

/** A JSON-RPC envelope, as much of one as spending a credential needs to know. */
export interface ToolCallRpc {
  method?: string
  params?: { name?: string; arguments?: Record<string, unknown> }
}

/** SPEND HANDLES INSIDE AN MCP TOOL CALL — the agent-facing half of
 *  `resolveHandles`, and the one that was missing.
 *
 *  `callMcpTool` carries the same substitution, but nothing on the AGENT path
 *  reaches it: an agent's tool call arrives at `/api/mcp/gw/$server` as JSON-RPC
 *  over its own credential, and that route forwarded the body it was handed. So
 *  an agent granted `«secret:deploy.github_pat»` — told in its soul that it may
 *  push with it — sent the literal handle upstream and got an auth failure it
 *  could not explain. Advertising a credential we do not substitute is the same
 *  class of lie as advertising a tool we cannot dispatch: nothing crashes, and
 *  the agent looks incapable of a thing the platform promised it.
 *
 *  ARGUMENTS ONLY, AND ONLY ON `tools/call`. A tool RESULT comes back untouched:
 *  it re-enters the model's context, and resolving a handle there would undo the
 *  whole arrangement in one line.
 *
 *  MUTATES `rpc.params.arguments` in place and returns whether anything changed,
 *  because the two dispatch paths downstream want different things — the
 *  in-process ones take the object, the HTTP one re-serializes — and only when
 *  something was actually spent. Every other call forwards byte-for-byte as
 *  before, so a re-serialization round trip is not something a working tool has
 *  to survive. */
export async function spendHandlesInToolCall(rpc: ToolCallRpc | null, caller: string): Promise<{ changed: boolean; used: Resolution['used']; unresolved: UnresolvedHandle[] }> {
  const none = { changed: false, used: [], unresolved: [] }
  if (rpc?.method !== 'tools/call' || !rpc.params?.arguments) return none
  const resolved = await resolveHandles(JSON.stringify(rpc.params.arguments), caller)
  if (resolved.used.length === 0) return { ...none, unresolved: resolved.unresolved }
  try {
    rpc.params.arguments = JSON.parse(resolved.text) as Record<string, unknown>
  } catch {
    // A credential containing something that broke the round trip. Forward the
    // call unresolved rather than a malformed body and let the tool refuse —
    // but still report the spend, because it happened.
    return { changed: false, used: resolved.used, unresolved: resolved.unresolved }
  }
  return { changed: true, used: resolved.used, unresolved: resolved.unresolved }
}

/** WHAT A HANDLE IN A CONVERSATION MEANS — the per-turn twin of the standing
 *  soul line below.
 *
 *  A STANDING GRANT AND A RELAY ARRIVE DIFFERENTLY, so they are told differently.
 *  A grant is a fact about the agent and belongs in its soul, rendered with the
 *  rest of what it is. A relay is minted mid-conversation for one errand, and an
 *  agent whose soul was written this morning has never heard of it — worse, an
 *  agent granted NOTHING is told nothing at all, by design, so it has never
 *  heard of handles either. Without this it would read `«secret:relay-…»` as a
 *  typo and either paste it verbatim into a tool call and fail, or ask the human
 *  to send the real value — which is precisely the paste this whole arrangement
 *  exists to prevent. */
export const HANDLE_TURN_NOTE =
  'A handle written «secret:name» in this conversation is a credential you may USE without ever seeing it. ' +
  'Pass it exactly as written wherever the value would go — in a tool call, a command, a URL — and Talaria substitutes the real value at the boundary that spends it. ' +
  'Never ask anybody to send you the value instead, and do not treat the handle as a placeholder to fill in: it IS the credential as far as you are concerned. ' +
  'A one-shot handle works once, so use it for the errand it was given for and nothing else.'

/** THE BRIEFING ITSELF, over rows rather than over a database.
 *
 *  SPLIT OUT SO THE BENCHMARK CANNOT DRIFT FROM PRODUCTION. `fitness` grades
 *  models on using handles correctly, and a fixture that briefed them with its
 *  own hand-written paraphrase would be measuring a prompt no agent has ever
 *  been given — the sweep would go green while the real soul line, worded
 *  differently, failed. One definition, both callers: the agent reads this and
 *  the eval hands the model this. */
/** THE HANDLES A BRIEFING NAMES, as strings — the doc-only form for a
 *  single-entry doc, the qualified form for a bundle.
 *
 *  ONE RULE, TWO READERS. `handleBriefing` renders these into the sentence an
 *  agent reads, and the fitness fixtures assert the model wrote one of them.
 *  Spelling that rule twice is how a benchmark comes to grade models against a
 *  handle no agent was ever offered — which is exactly what happened here the
 *  first time, and what this function exists to make impossible. */
export function briefedHandles(rows: Array<{ name: string; key: string; label: string }>): string[] {
  const byDoc = new Map<string, Array<{ key: string; label: string }>>()
  for (const r of rows) byDoc.set(r.name, [...(byDoc.get(r.name) ?? []), { key: r.key, label: r.label }])
  return [...byDoc].flatMap(([name, es]) => (es.length === 1 ? [handleFor(name)] : es.map((e) => handleFor(name, e.key))))
}

export function handleBriefing(rows: Array<{ name: string; key: string; label: string }>): string {
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
  return handleBriefing(rows)
}
