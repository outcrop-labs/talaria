// The one answer to "what secrets does this instance hold, and can it still read
// them?" — a VIEW over the stores that already own each value, never a second
// store of its own. A provider key entered on /models keeps living in
// `llm_endpoints`; it merely *appears* here.
//
// WHY THIS EXISTS
//   Talaria seals a dozen kinds of credential across eight tables. When the
//   encryption root changes, some of that ciphertext becomes unreadable and the
//   only signal is a failure at use time — a chat that won't start, a Drive sync
//   that stops, an SMTP send that silently doesn't. Diagnosing it meant querying
//   eight tables by hand, and recovering meant `reset.sh secrets`, which
//   destroys every sealed value including the ones that were fine.
//
//   Per-row health is what makes recovery proportionate: "these three are
//   unreadable, clear them" instead of "lose everything you have."
//
// TWO RULES THIS FILE KEEPS
//   1. It never returns a plaintext secret. Not masked, not truncated — the
//      shape has no field for one, so there is nowhere for one to leak.
//   2. It never throws on an unhealthy instance. Every read here is a status
//      read, and a status read that fails when the instance is broken is
//      useless exactly when it is needed. Note this rules out the typed config
//      getters (getStorageConfig et al) — several of them call open() on read.
//      We read the app_settings rows directly instead.

import { db } from './db/pg'
import { activeKeyVersion, loadedVersions, rootSource, secretboxFailure, tokenReadable } from './secretbox'

/** ok — sealed and readable. unreadable — sealed under a key this process
 *  cannot unwrap. missing — not configured. env — lives in the process
 *  environment rather than the database, so it is not ours to judge or clear. */
export type SecretState = 'ok' | 'unreadable' | 'missing' | 'env'

/** Grouped by what they unlock, which is what an operator is actually asking
 *  when they come here ("is my chat broken or my Drive?"). */
export type SecretGroup = 'models' | 'integrations' | 'agents' | 'platform'

export interface SecretRow {
  /** Stable and parseable — `clearSecret` reads it back. Built in exactly one
   *  place (the `id()` helpers below) so the two never drift. */
  id: string
  group: SecretGroup
  /** What this is, in the operator's words. */
  label: string
  /** What stops working without it. Shown verbatim before a clear. */
  unlocks: string
  /** The surface that owns the value — where Replace sends you. */
  surface: string
  href?: string
  state: SecretState
  /** Whose it is. `instance` values belong to the org; `user` values belong to
   *  a person and an admin sees only that they exist (see the note on
   *  per-user rows below). */
  scope: 'instance' | 'user' | 'agent'
  /** For user- and agent-scoped rows: whose. Needed for the row to be
   *  actionable — "a Google connection" with no owner cannot be cleared. */
  owner?: string
  setAt?: string | null
  lastUsedAt?: string | null
  expiresAt?: string | null
  /** False where clearing would be meaningless (env-held) or destructive
   *  beyond this row. */
  clearable: boolean
}

export interface RootHealth {
  /** env — a dedicated TALARIA_SECRET_KEY. file — TALARIA_SECRET_KEY_FILE.
   *  fallback — borrowing AUTH_SECRET, which its own documentation calls safe
   *  to rotate; it is not, on this instance. absent — nothing set. */
  via: 'env' | 'file' | 'fallback' | 'absent'
  name: string
  /** ok · fallback (works, one rotation from disaster) · absent (nothing can
   *  be sealed) · unreadable (keys exist, this root cannot unwrap them). */
  state: 'ok' | 'fallback' | 'absent' | 'unreadable'
  /** The recorded diagnosis from secretbox, when there is one. */
  failure: string | null
  activeVersion: number | null
  loadedVersions: number[]
  /** How many data-key versions the database holds. loadedVersions shorter
   *  than this means some ciphertext is unreadable no matter what the rows say. */
  storedVersions: number
}

export interface SecretHealth {
  root: RootHealth
  rows: SecretRow[]
  counts: { ok: number; unreadable: number; missing: number; env: number }
}

// ── Per-user rows: what an admin sees ────────────────────────────────────────
// DECISION (the open question in docs/SECRETS-PLAN.md): an admin sees that a
// user's connection exists, whose it is, when it was set, and whether it still
// decrypts — enough to recover the instance. They do NOT see the granted
// scopes, the account it points at, or when it was last used. Those describe
// the person's relationship with a third party rather than the instance's
// health, and recovery does not need them.
const USER_SCOPED_METADATA = false

const iso = (v: unknown): string | null => (v instanceof Date ? v.toISOString() : typeof v === 'string' ? v : null)

/** One row's state from its ciphertext. `envName` covers the provider-key case
 *  where the value is a variable name rather than a sealed value. */
function stateOf(cipher: string | null | undefined, envName?: string | null): SecretState {
  if (cipher) return tokenReadable(cipher) ? 'ok' : 'unreadable'
  if (envName) return 'env'
  return 'missing'
}

// ── Ids ──────────────────────────────────────────────────────────────────────
// `<store>:<key…>`. Every id is minted by one of these and parsed by exactly
// one branch of `clearSecret`, so a new store cannot be added to the inventory
// without deciding what clearing it means.
const id = {
  llm: (endpointId: string) => `llm:${endpointId}`,
  agentSecret: (agentId: string, name: string) => `agent-secret:${agentId}:${name}`,
  // Not clearable — see the row below for why a half-deleted bundle is worse
  // than none — but it still needs a stable id so the list can key on it.
  workspaceSecret: (name: string, key: string) => `workspace-secret:${name}:${key}`,
  agentKey: (agentId: string) => `agent-key:${agentId}`,
  googleUser: (userId: string) => `google-user:${userId}`,
  googleOrg: () => 'google-org',
  mcpOauth: (serverId: string, subject: string) => `mcp-oauth:${serverId}:${subject}`,
  mcpHeaders: (serverId: string, userId: string) => `mcp-headers:${serverId}:${userId}`,
  /** `path` is the dotted path INTO the stored jsonb, not a display name —
   *  `clearSecret` walks it literally. A friendlier id would need a
   *  translation table, and a translation table is a thing that drifts. */
  setting: (key: string, path: string) => `setting:${key}:${path}`,
} as const

export async function secretHealth(): Promise<SecretHealth> {
  const sql = await db()
  const rows: SecretRow[] = []

  // ── Models ─────────────────────────────────────────────────────────────────
  // Every endpoint, not only the sealed ones: an endpoint with no key at all is
  // a real gap ("why won't this model answer?") and belongs in the same list.
  const endpoints = (await sql`
    select id, name, provider, api_key_cipher as "cipher", api_key_env as "envName",
           created_at as "createdAt", updated_at as "updatedAt"
    from llm_endpoints order by name asc
  `) as unknown as Array<{
    id: string
    name: string
    provider: string
    cipher: string | null
    envName: string | null
    createdAt: unknown
    updatedAt: unknown
  }>
  for (const e of endpoints) {
    rows.push({
      id: id.llm(e.id),
      group: 'models',
      label: e.name,
      // The endpoint's NAME, not its provider: `provider` is 'custom' for
      // anything self-hosted or OpenAI-compatible, and "routed to custom" tells
      // an operator nothing about which of their endpoints just broke.
      unlocks: `Chat, Plan, Research and every agent turn routed to ${e.name}`,
      surface: 'Models',
      href: '/models',
      state: stateOf(e.cipher, e.envName),
      scope: 'instance',
      setAt: iso(e.updatedAt) ?? iso(e.createdAt),
      clearable: Boolean(e.cipher),
    })
  }

  // ── Workspace credentials (the handle store) ───────────────────────────────
  //
  // ADDED BECAUSE ITS ABSENCE WAS THE BUG. This page answers "what secret
  // material does this instance hold, and can it still be decrypted" — and the
  // newest store in the tree was not in the answer. An operator rotating the
  // encryption key would have watched every other row go unreadable with no
  // idea that a dozen agent credentials had gone with them.
  //
  // ONE ROW PER ENTRY, not per doc: a bundle can be half-readable if it was
  // written across a rotation, and a doc-level row would report the whole thing
  // on the state of whichever entry happened to sort first.
  const workspaceSecrets = (await sql`
    select s.name, s.title, s.revealable, s.owner_user_id as "ownerUserId", s.updated_at as "updatedAt",
           e.key, e.label, e.value_cipher as "cipher",
           (select count(*)::int from workspace_secret_grants g where g.secret_id = s.id) as "grants"
      from workspace_secrets s join workspace_secret_entries e on e.secret_id = s.id
     order by s.title asc, e.key asc
  `) as unknown as Array<{
    name: string
    title: string
    revealable: boolean
    ownerUserId: string | null
    updatedAt: unknown
    key: string
    label: string
    cipher: string
    grants: number
  }>
  for (const w of workspaceSecrets) {
    rows.push({
      id: id.workspaceSecret(w.name, w.key),
      group: 'agents',
      label: `${w.title} · ${w.label}`,
      unlocks: w.revealable
        ? `A working secret. ${w.grants > 0 ? `${w.grants} agent${w.grants === 1 ? '' : 's'} can spend it; ` : ''}the people it is shared with can read it`
        : `${w.grants > 0 ? `${w.grants} agent${w.grants === 1 ? '' : 's'} can SPEND it` : 'Granted to no agent, so nobody can spend it'} — and nobody can read it`,
      surface: w.revealable ? 'Files → Secrets' : 'Admin → Secrets',
      href: w.revealable ? '/artifacts?p=secrets' : '/admin?tab=secrets',
      // A SPENT ONE-SHOT IS NOT BROKEN. `resolveHandles` empties the ciphertext
      // once the last use is gone, deliberately — reporting that as an
      // encryption failure would send an operator hunting a key problem that
      // does not exist.
      // A SPENT ONE-SHOT reads as 'missing' rather than 'unreadable': the value
      // is gone because we destroyed it on purpose, not because a key rotation
      // broke it, and sending an operator hunting an encryption problem that
      // does not exist is the whole failure this row exists to avoid.
      state: w.cipher === '' ? 'missing' : stateOf(w.cipher),
      scope: w.revealable ? 'user' : 'instance',
      ...(w.revealable && w.ownerUserId ? { owner: w.ownerUserId } : {}),
      setAt: iso(w.updatedAt),
      // NOT CLEARABLE FROM HERE. Deleting one entry out of a bundle leaves a
      // credential that half-works, and both owning surfaces already delete the
      // whole doc behind a confirmation that names who loses access.
      clearable: false,
    })
  }

  // ── Agents ─────────────────────────────────────────────────────────────────
  const agentSecrets = (await sql`
    select s.agent_id as "agentId", s.name, s.value_enc as "cipher", s.updated_at as "updatedAt",
           d.display_name as "agentName"
    from agent_secrets s join agent_defs d on d.id = s.agent_id
    order by d.display_name asc, s.name asc
  `) as unknown as Array<{ agentId: string; name: string; cipher: string; updatedAt: unknown; agentName: string }>
  for (const s of agentSecrets) {
    rows.push({
      id: id.agentSecret(s.agentId, s.name),
      group: 'agents',
      label: s.name,
      // SAID PLAINLY, because the difference between this row and a handle row
      // above is the whole security story and it used to be invisible. An env
      // var is materialised into the container as plaintext, and every fleet
      // agent runs a harness with a shell — so `echo $NAME` returns the value,
      // and from there it is in the model's context and on its way to a
      // provider. Handles exist so that sentence is not true.
      unlocks: `Whatever ${s.agentName} uses ${s.name} for. Materialised into the container as a PLAINTEXT environment variable, so ${s.agentName} can read it — prefer a credential handle where the value only has to be spent, not seen`,
      surface: 'Agents',
      href: '/agents',
      state: stateOf(s.cipher),
      scope: 'agent',
      owner: s.agentName,
      setAt: iso(s.updatedAt),
      clearable: true,
    })
  }

  const agentKeys = (await sql`
    select k.agent_id as "agentId", k.key_enc as "cipher", k.created_at as "createdAt",
           k.last_used_at as "lastUsedAt", d.display_name as "agentName"
    from agent_keys k join agent_defs d on d.id = k.agent_id
    order by d.display_name asc
  `) as unknown as Array<{
    agentId: string
    cipher: string
    createdAt: unknown
    lastUsedAt: unknown
    agentName: string
  }>
  for (const k of agentKeys) {
    rows.push({
      id: id.agentKey(k.agentId),
      group: 'agents',
      label: `${k.agentName} credential`,
      // The hash stays usable for AUTH even when the sealed copy is not — the
      // agent keeps working, but nothing can re-render its config. Say that
      // rather than implying the agent is down.
      unlocks: `Re-rendering ${k.agentName}'s container config. The agent authenticates against a hash, so it keeps working; the fleet just cannot reissue its key without a re-render`,
      surface: 'Agents',
      href: '/agents',
      state: stateOf(k.cipher),
      scope: 'agent',
      owner: k.agentName,
      setAt: iso(k.createdAt),
      lastUsedAt: iso(k.lastUsedAt),
      clearable: true,
    })
  }

  // ── Integrations ───────────────────────────────────────────────────────────
  const googleUsers = (await sql`
    select g.user_id as "userId", g.refresh_token_enc as "cipher", g.updated_at as "updatedAt",
           u.email
    from google_connections g join users u on u.id = g.user_id
    order by u.email asc
  `) as unknown as Array<{ userId: string; cipher: string | null; updatedAt: unknown; email: string }>
  for (const c of googleUsers) {
    rows.push({
      id: id.googleUser(c.userId),
      group: 'integrations',
      label: 'Google connection',
      unlocks: 'Drive and Docs for this person, and their assistant acting as them',
      surface: 'Settings',
      state: stateOf(c.cipher),
      scope: 'user',
      owner: c.email,
      setAt: iso(c.updatedAt),
      clearable: true,
    })
  }

  const googleOrg = (await sql`
    select refresh_token_enc as "cipher", email, updated_at as "updatedAt",
           access_expires_at as "expiresAt"
    from google_org_connection where id = 1
  `) as unknown as Array<{ cipher: string | null; email: string | null; updatedAt: unknown; expiresAt: unknown }>
  rows.push({
    id: id.googleOrg(),
    group: 'integrations',
    label: 'Org Google connection',
    unlocks: 'Drive and Docs for every fleet agent without a human owner',
    surface: 'Admin → Organization',
    href: '/admin',
    state: stateOf(googleOrg[0]?.cipher),
    scope: 'instance',
    owner: googleOrg[0]?.email ?? undefined,
    setAt: iso(googleOrg[0]?.updatedAt),
    expiresAt: iso(googleOrg[0]?.expiresAt),
    clearable: Boolean(googleOrg[0]?.cipher),
  })

  const mcpTokens = (await sql`
    select t.server_id as "serverId", t.subject, t.tokens_enc as "cipher", t.updated_at as "updatedAt",
           s.label, u.email
    from mcp_oauth_tokens t
    join mcp_servers s on s.id = t.server_id
    left join users u on u.id::text = t.subject
    order by s.label asc, t.subject asc
  `) as unknown as Array<{
    serverId: string
    subject: string
    cipher: string
    updatedAt: unknown
    label: string
    email: string | null
  }>
  for (const t of mcpTokens) {
    const org = t.subject === 'org'
    rows.push({
      id: id.mcpOauth(t.serverId, t.subject),
      group: 'integrations',
      label: `${t.label} — connected account`,
      unlocks: org ? `${t.label} for everyone who uses it` : `${t.label} for this person`,
      surface: 'MCP',
      href: '/mcp',
      state: stateOf(t.cipher),
      scope: org ? 'instance' : 'user',
      owner: org ? undefined : (t.email ?? t.subject),
      setAt: iso(t.updatedAt),
      clearable: true,
    })
  }

  const mcpHeaders = (await sql`
    select c.server_id as "serverId", c.user_id as "userId", c.headers_enc as "cipher",
           c.updated_at as "updatedAt", s.label, u.email
    from mcp_user_credentials c
    join mcp_servers s on s.id = c.server_id
    join users u on u.id = c.user_id
    order by s.label asc, u.email asc
  `) as unknown as Array<{
    serverId: string
    userId: string
    cipher: string
    updatedAt: unknown
    label: string
    email: string
  }>
  for (const h of mcpHeaders) {
    rows.push({
      id: id.mcpHeaders(h.serverId, h.userId),
      group: 'integrations',
      label: `${h.label} — credentials`,
      unlocks: `${h.label} for this person`,
      surface: 'MCP',
      href: '/mcp',
      state: stateOf(h.cipher),
      scope: 'user',
      owner: h.email,
      setAt: iso(h.updatedAt),
      clearable: true,
    })
  }

  // ── Platform (app_settings) ────────────────────────────────────────────────
  // Read raw. The typed getters for these keys call open() on read — one of
  // them swallows the failure into an empty string, which is precisely the
  // silent unreadability this inventory exists to surface.
  const settingRows = (await sql`
    select key, value, updated_at as "updatedAt" from app_settings
    where key in ('email_config', 'storage_config', 'github_config', 'rag_rerank_config')
  `) as unknown as Array<{ key: string; value: Record<string, unknown>; updatedAt: unknown }>
  const setting = (k: string) => settingRows.find((r) => r.key === k)
  const dig = (v: unknown, ...path: string[]): string | null => {
    let cur: unknown = v
    for (const p of path) cur = (cur as Record<string, unknown> | undefined)?.[p]
    return typeof cur === 'string' && cur ? cur : null
  }

  const email = setting('email_config')
  const emailProvider = dig(email?.value, 'provider')
  rows.push({
    id: id.setting('email_config', emailProvider === 'resend' ? 'resend.apiKeyEnc' : 'smtp.passEnc'),
    group: 'platform',
    label: emailProvider === 'resend' ? 'Resend API key' : 'SMTP password',
    unlocks: 'Invites, password resets, and any notification delivered by email',
    surface: 'Admin → Organization',
    href: '/admin',
    state: stateOf(
      emailProvider === 'resend' ? dig(email?.value, 'resend', 'apiKeyEnc') : dig(email?.value, 'smtp', 'passEnc'),
    ),
    scope: 'instance',
    setAt: iso(email?.updatedAt),
    clearable: Boolean(email),
  })

  const storage = setting('storage_config')
  const storageMode = dig(storage?.value, 'mode')
  // Only external buckets hold a secret worth inventorying — 'local' writes to
  // disk and 'internal' uses the bundled MinIO's env-held credentials.
  if (storageMode && storageMode !== 'local' && storageMode !== 'internal') {
    rows.push({
      id: id.setting('storage_config', 'secretAccessKey'),
      group: 'platform',
      label: 'Object storage secret key',
      unlocks: 'Uploads, attachments and artifact files',
      surface: 'Admin → Storage',
      href: '/admin?tab=storage',
      state: stateOf(dig(storage?.value, 'secretAccessKey')),
      scope: 'instance',
      setAt: iso(storage?.updatedAt),
      clearable: true,
    })
    if (dig(storage?.value, 'replica', 'secretAccessKey')) {
      rows.push({
        id: id.setting('storage_config', 'replica.secretAccessKey'),
        group: 'platform',
        label: 'Object storage replica secret key',
        unlocks: 'Mirroring uploads to the replica bucket',
        surface: 'Admin → Storage',
        href: '/admin?tab=storage',
        state: stateOf(dig(storage?.value, 'replica', 'secretAccessKey')),
        scope: 'instance',
        setAt: iso(storage?.updatedAt),
        clearable: true,
      })
    }
  }

  const github = setting('github_config')
  const githubMode = dig(github?.value, 'mode')
  if (githubMode) {
    rows.push({
      id: id.setting('github_config', githubMode === 'app' ? 'app.privateKeyEnc' : 'pat.tokenEnc'),
      group: 'platform',
      label: githubMode === 'app' ? 'GitHub App private key' : 'GitHub access token',
      unlocks: 'Workbench: cloning repos, pushing branches, opening pull requests',
      surface: 'Admin → Organization',
      href: '/admin',
      state: stateOf(
        githubMode === 'app' ? dig(github?.value, 'app', 'privateKeyEnc') : dig(github?.value, 'pat', 'tokenEnc'),
      ),
      scope: 'instance',
      setAt: iso(github?.updatedAt),
      clearable: true,
    })
  }

  const rerank = setting('rag_rerank_config')
  if (dig(rerank?.value, 'keySealed')) {
    rows.push({
      id: id.setting('rag_rerank_config', 'keySealed'),
      group: 'platform',
      label: 'Reranker API key',
      unlocks: 'Reranking retrieval results — search still works without it, less well',
      surface: 'Admin → Retrieval',
      href: '/admin?tab=retrieval',
      state: stateOf(dig(rerank?.value, 'keySealed')),
      scope: 'instance',
      setAt: iso(rerank?.updatedAt),
      clearable: true,
    })
  }

  if (!USER_SCOPED_METADATA) {
    for (const r of rows) {
      if (r.scope === 'user') {
        r.lastUsedAt = undefined
        r.expiresAt = undefined
      }
    }
  }

  // ── Root ───────────────────────────────────────────────────────────────────
  const stored = (await sql`select count(*)::int as n from secret_keys`) as unknown as Array<{ n: number }>
  const storedVersions = stored[0]?.n ?? 0
  const src = rootSource()
  const failure = secretboxFailure()
  const loaded = loadedVersions()
  const root: RootHealth = {
    ...src,
    state:
      // Ordered by what the operator must do first. An unreadable root makes
      // every other judgement here moot, so it wins over "you're on the
      // fallback" — which is a warning about the future, not the present.
      failure || (storedVersions > 0 && loaded.length === 0)
        ? 'unreadable'
        : src.via === 'absent'
          ? 'absent'
          : src.via === 'fallback'
            ? 'fallback'
            : 'ok',
    failure,
    activeVersion: activeKeyVersion(),
    loadedVersions: loaded,
    storedVersions,
  }

  const counts = { ok: 0, unreadable: 0, missing: 0, env: 0 }
  for (const r of rows) counts[r.state]++

  return { root, rows, counts }
}

// ── Clearing ─────────────────────────────────────────────────────────────────
// The in-app half of `scripts/reset.sh secrets`, scoped to one row. Clearing is
// a DELETE of ciphertext and nothing else: the endpoint, the agent, the person
// and the server all survive, missing a credential they can be given again.
//
// The script stays as the backstop for an instance that will not boot. This is
// the path for one that boots fine and cannot read three of its secrets.

export class UnknownSecretId extends Error {}

/** Clear exactly one row's ciphertext. Returns false when the id was
 *  well-formed but matched nothing (already cleared, or the row went away
 *  between the read and the click) — the caller reports that as a no-op rather
 *  than an error, because the desired end state has been reached either way. */
export async function clearSecret(secretId: string): Promise<boolean> {
  const sql = await db()
  const [store, ...rest] = secretId.split(':')

  switch (store) {
    case 'llm': {
      // `is not null` is what makes the return value mean "something changed"
      // rather than "a row with that id exists" — an UPDATE counts rows it
      // matched, not rows it altered, so without it a second clear reports a
      // success it did not perform.
      const r = await sql`
        update llm_endpoints set api_key_cipher = null, updated_at = now()
        where id = ${rest[0]!} and api_key_cipher is not null
      `
      return r.count > 0
    }
    case 'agent-secret': {
      // The name can itself contain no colon (agent secrets are env-var names),
      // but rejoin anyway so a future looser name cannot silently truncate.
      const r = await sql`delete from agent_secrets where agent_id = ${rest[0]!} and name = ${rest.slice(1).join(':')}`
      return r.count > 0
    }
    case 'agent-key': {
      // Deletes the hash along with the sealed copy, so the agent's current key
      // stops authenticating. That is the honest recovery: an unreadable
      // key_enc means the fleet can never reissue this credential, and leaving
      // the hash behind would keep a key alive that nothing can reproduce. The
      // confirmation says "re-render the fleet" for exactly this reason.
      const r = await sql`delete from agent_keys where agent_id = ${rest[0]!}`
      return r.count > 0
    }
    case 'google-user': {
      const r = await sql`delete from google_connections where user_id = ${rest[0]!}`
      return r.count > 0
    }
    case 'google-org': {
      const r = await sql`delete from google_org_connection where id = 1`
      return r.count > 0
    }
    case 'mcp-oauth': {
      const r = await sql`delete from mcp_oauth_tokens where server_id = ${rest[0]!} and subject = ${rest[1]!}`
      return r.count > 0
    }
    case 'mcp-headers': {
      const r = await sql`delete from mcp_user_credentials where server_id = ${rest[0]!} and user_id = ${rest[1]!}`
      return r.count > 0
    }
    case 'setting': {
      const key = rest[0]!
      const path = rest.slice(1).join(':').split('.')
      if (!key || !path.length || path.some((p) => !p)) throw new UnknownSecretId(secretId)
      // Read-modify-write rather than a jsonb path update: these documents are
      // small, and doing it in JS keeps the "delete only this leaf" logic
      // readable. Guarded by a row lock so a concurrent config save cannot be
      // clobbered by our stale copy.
      return await sql.begin(async (tx) => {
        const cur = (await tx`
          select value from app_settings where key = ${key} for update
        `) as unknown as Array<{ value: Record<string, unknown> }>
        const value = cur[0]?.value
        if (!value) return false
        let node: Record<string, unknown> = value
        for (const p of path.slice(0, -1)) {
          const next = node[p]
          if (!next || typeof next !== 'object') return false
          node = next as Record<string, unknown>
        }
        const leaf = path[path.length - 1]!
        if (node[leaf] === undefined || node[leaf] === null || node[leaf] === '') return false
        // null, not delete: several of these configs read the key's presence
        // and a missing key would fall back to a DEFAULT rather than to
        // "unset". `secretAccessKey` in particular is typed as a string.
        node[leaf] = typeof node[leaf] === 'string' ? '' : null
        await tx`update app_settings set value = ${tx.json(value as never)}, updated_at = now() where key = ${key}`
        return true
      })
    }
    default:
      throw new UnknownSecretId(secretId)
  }
}

/** Clear every row the probe reports as unreadable. Re-probes first, so what is
 *  cleared is what is broken NOW — not what a stale page thought was broken. */
export async function clearUnreadable(): Promise<{ cleared: string[]; failed: string[] }> {
  const { rows } = await secretHealth()
  const cleared: string[] = []
  const failed: string[] = []
  for (const r of rows) {
    if (r.state !== 'unreadable' || !r.clearable) continue
    try {
      if (await clearSecret(r.id)) cleared.push(r.label)
    } catch (e) {
      // One store refusing must not strand the rest — the operator came here
      // to get unstuck, and a partial clear beats an aborted one.
      console.error('[secrets] clear failed', r.id, e)
      failed.push(r.label)
    }
  }
  return { cleared, failed }
}
