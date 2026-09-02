// Auth for every agent-authenticated API — the fleet-facing "brain" endpoints
// (register / heartbeat / report) and the whole toolkit surface behind them.
//
// An agent presents ITS OWN credential: a `tak_` secret minted per agent_defs
// row, sealed in the DB, and stamped into that agent's container as
// TALARIA_AGENT_KEY (the Rust fleet render, api/src/fleet/render.rs). Identity
// is resolved FROM THE CREDENTIAL —
// x-agent-name is a cross-check that can narrow access but never grant it.
// That binding is what makes board policy, MCP allowlists, retrieval
// principals and owner-identity proxying enforceable rather than advisory.
//
// TRANSITION: containers built before per-agent keys still hold the org-wide
// TALARIA_AGENT_KEY, which names nobody. Those callers are accepted while the
// window is open (set TALARIA_AGENT_KEY_LEGACY=off to close it) and resolve
// `legacy: true` — enough for board-scoped work so a deploy never locks the
// live fleet out, refused by anything that grants privilege. To finish the
// migration: render the fleet (mints + stamps every key — api/src/fleet/render.rs),
// `roll_running_agents()` (api/src/fleet/reconcile.rs), then set the flag.
// `legacyMigrationStatus()` answers the question that decides when that flag
// is safe to flip.
//
// LEGACY MEANS IDENTIFIED BUT UNTRUSTED, and that is enforced HERE rather than
// left to each caller to remember:
//   • the claimed name must resolve to an ENABLED agent_defs row — a retired or
//     invented name is refused, so retiring an agent revokes its access and a
//     forged name can't poison attribution
//   • a name that carries HUMAN privilege (a personal assistant, or an elevated
//     assistant) is refused outright: everything such an identity buys —
//     owner-proxying, org-wide boards/channels/KB, the owner's Google token —
//     is escalation, and the shared key proves fleet membership, not identity.
//     Ordinary scoped agents keep working, which is the point of the window.
// Surfaces that grant privilege still check `legacy` themselves (users.ts
// here; the boards and Google agent engines in Rust — api/src/boards.rs,
// api/src/google/agent.rs) — defence in depth, so loosening one layer doesn't
// quietly open the other.
//
// LOAD-BEARING OFF-SITE DEPENDENCY — `GET /api/users`:
// the toolkit MCP process (mcp/src/index.ts, HTTP mode) holds no DB, so it
// authenticates every connecting agent by issuing an authenticated
// `GET ${TALARIA_URL}/api/users` and reading the status code. That route's agent
// branch is nothing but `agentCaller()` — this module — which is exactly why it
// was chosen, but it means users.ts is now an AUTHENTICATION ORACLE for the
// whole fleet toolkit. Narrow it (admin-only, session-only, moved, renamed) and
// every agent's `initialize`/`tools/list` starts failing: the toolkit goes dark
// fleet-wide, with no error that points here. If you must change users.ts,
// repoint the probe first — `TALARIA_MCP_VERIFY_PATH` overrides the path with no
// code change, and mcp/README.md ("Authentication") documents the contract the
// probe needs: agent-credential auth, cheap, GET, 401/403 on a bad credential.

import { createHash, randomBytes, timingSafeEqual } from 'node:crypto'
import { json } from '@/server/http'
import { db } from './db/pg'
import { open, seal } from './secretbox'

/** Mirrors the `tlk_` LLM-key convention, so an unrecognized Bearer token is
 *  distinguishable from an agent credential we simply don't know. */
const KEY_PREFIX = 'tak_'

const hash = (secret: string) => createHash('sha256').update(secret).digest('hex')

function eq(a: string, b: string): boolean {
  const ab = Buffer.from(a)
  const bb = Buffer.from(b)
  return ab.length === bb.length && timingSafeEqual(ab, bb)
}

export interface AgentCaller {
  /** agent_defs.id — null only for a legacy shared-key caller, whose identity
   *  is asserted rather than proven. Non-null therefore means PROVEN, and is
   *  what a surface should key privilege off (or `!legacy`, its mirror). */
  id: string | null
  /** Fleet model id (`<slug>-<department>`): what every per-agent control —
   *  board policy, MCP allowlists, retrieval principals, metering — keys off.
   *  Always a real, enabled agent_defs.model, legacy or not. */
  model: string
  /** True when the org-wide key authenticated this caller, so `model` came
   *  from a header. Surfaces that grant privilege must refuse these. */
  legacy: boolean
}

/** An agent identified either by a resolved caller (carries proof) or by a bare
 *  model string from a surface that hasn't been threaded yet. Privilege checks
 *  take this and consult `legacy`; a plain string is treated as proven, which
 *  is safe because a legacy caller can never present a privileged name. */
export type AgentSubject = AgentCaller | string

export const subjectModel = (subject: AgentSubject): string => (typeof subject === 'string' ? subject : subject.model)

/** False only for a legacy shared-key caller: identified, but not proven, so it
 *  gets no elevation, no owner-proxying and no OAuth tokens. */
export const subjectProven = (subject: AgentSubject): boolean => typeof subject === 'string' || !subject.legacy

/** The one instruction every legacy refusal ends with — the failure has to say
 *  what to DO, or it reads as a broken integration instead of a migration step
 *  (the variable `agent_key_var` mints — api/src/fleet/layout.rs; the Rust
 *  twin of this line lives in api/src/agent_auth.rs). */
const ROLL_IT = 'Re-render the fleet and roll this container so it presents its own TALARIA_AGENT_KEY_<SLUG>.'

/** Ready-to-return refusal for a surface that acts as a HUMAN (Google, owner
 *  proxying) when the caller is legacy. Says what is wrong AND what to do —
 *  a bare 403 here reads as a broken integration rather than a migration step. */
export function refuseLegacy(caller: AgentCaller, what: string): Response | null {
  if (!caller.legacy) return null
  return json(
    {
      error: 'forbidden',
      message: `${what} needs the agent's own credential. "${caller.model}" authenticated with the org-wide TALARIA_AGENT_KEY, which proves fleet membership but not identity. ${ROLL_IT}`,
    },
    { status: 403 },
  )
}

// ── Minting ──────────────────────────────────────────────────────────────────

/** Issue a fresh credential for an agent, invalidating any previous one. The
 *  plaintext is returned here and never again: the row keeps a sha256 (auth
 *  never decrypts) and a sealed copy (a wiped fleet/.env is recoverable).
 *
 *  FOLLOW-UP: nothing calls this yet outside first-mint, so a leaked credential
 *  has no revocation path — rotate + re-render + roll needs an admin surface
 *  (agents UI), which is out of scope for this change. */
export async function rotateAgentApiKey(agentId: string): Promise<string> {
  const secret = `${KEY_PREFIX}${randomBytes(24).toString('hex')}`
  const sql = await db()
  await sql`
    insert into agent_keys (agent_id, key_hash, key_enc) values (${agentId}, ${hash(secret)}, ${seal(secret)})
    on conflict (agent_id) do update set key_hash = excluded.key_hash, key_enc = excluded.key_enc, created_at = now()
  `
  return secret
}

/** The agent's credential, minting one on first use. Stable across renders —
 *  re-minting on every render would lock out every running container. */
export async function ensureAgentApiKey(agentId: string): Promise<string> {
  const sql = await db()
  const rows = (await sql`select key_enc as "keyEnc" from agent_keys where agent_id = ${agentId}`) as unknown as Array<{
    keyEnc: string
  }>
  const existing = rows[0]
  return existing ? open(existing.keyEnc) : rotateAgentApiKey(agentId)
}

// ── Resolution ───────────────────────────────────────────────────────────────

/** The credential as presented, for the one hop that must carry it onward:
 *  the MCP gateway → the builtin toolkit service, which calls back into this
 *  API as the same agent. Pass it through rather than substituting a
 *  server-held key, or that hop re-opens the hole this module closes. */
export function presentedCredential(request: Request): string | null {
  return presented(request)
}

function presented(request: Request): string | null {
  const xkey = request.headers.get('x-api-key')?.trim()
  if (xkey) return xkey
  const auth = request.headers.get('authorization')
  const bearer = auth?.startsWith('Bearer ') ? auth.slice(7).trim() : null
  return bearer || null
}

/** The name the caller CLAIMS to be. Never an identity on its own. */
function declaredName(request: Request): string | null {
  const name = request.headers.get('x-agent-name')?.trim()
  return name && name.length <= 200 ? name : null
}

const legacyOpen = (): boolean => (process.env.TALARIA_AGENT_KEY_LEGACY ?? 'on').trim() !== 'off'

export interface LegacySighting {
  model: string
  count: number
  firstAt: string
  lastAt: string
}

// Who is still presenting the shared key, in THIS process. Deduped for the log
// but kept as data, because "has every agent moved over yet?" is the question
// that decides when TALARIA_AGENT_KEY_LEGACY=off is safe — and a one-shot
// console.warn an operator scrolled past can't answer it.
const legacySeen = new Map<string, LegacySighting>()
const WARN_EVERY_MS = 15 * 60 * 1000
const lastWarned = new Map<string, number>()

/** Repeat on a slow cadence rather than once per process: a single line from
 *  whenever the server started can't answer "is this still happening?". */
function warnOnce(key: string, line: string, level: 'warn' | 'error' = 'warn'): void {
  const now = Date.now()
  if (now - (lastWarned.get(key) ?? 0) < WARN_EVERY_MS) return
  lastWarned.set(key, now)
  console[level](line)
}

function warnLegacy(model: string): void {
  const now = Date.now()
  const seen = legacySeen.get(model)
  if (seen) {
    seen.count += 1
    seen.lastAt = new Date(now).toISOString()
  } else {
    legacySeen.set(model, { model, count: 1, firstAt: new Date(now).toISOString(), lastAt: new Date(now).toISOString() })
  }
  warnOnce(
    `legacy:${model}`,
    `[agent-auth] "${model}" authenticated with the org-wide TALARIA_AGENT_KEY (deprecated — self-declared identity, so no elevation, owner-proxying or OAuth). Re-render the fleet and roll this agent onto its own credential.`,
  )
}

/** Shared-key sightings since this process started. */
export function legacyUsage(): LegacySighting[] {
  return [...legacySeen.values()].sort((a, b) => a.model.localeCompare(b.model))
}

export interface LegacyMigrationStatus {
  /** TALARIA_AGENT_KEY_LEGACY is not 'off'. */
  windowOpen: boolean
  agents: Array<{ model: string; keyMinted: boolean; lastUsedAt: string | null; migrated: boolean }>
  /** Enabled managed agents that have NEVER authenticated with their own
   *  credential — flipping the flag locks exactly these out. */
  pending: string[]
  /** Shared-key callers this process has served. */
  legacySeen: LegacySighting[]
}

/** The migration answer, from the data rather than from memory: agent_keys
 *  .last_used_at is written on every per-agent authentication, so an agent that
 *  has one has proved it is running on its own secret. */
export async function legacyMigrationStatus(): Promise<LegacyMigrationStatus> {
  const sql = await db()
  const rows = (await sql`
    select d.model, k.agent_id is not null as "keyMinted", k.last_used_at as "lastUsedAt"
    from agent_defs d left join agent_keys k on k.agent_id = d.id
    where d.enabled and d.managed order by d.model
  `) as unknown as Array<{ model: string; keyMinted: boolean; lastUsedAt: string | Date | null }>
  const agents = rows.map((r) => ({
    model: r.model,
    keyMinted: r.keyMinted,
    lastUsedAt: r.lastUsedAt ? new Date(r.lastUsedAt).toISOString() : null,
    migrated: !!r.lastUsedAt,
  }))
  return {
    windowOpen: legacyOpen(),
    agents,
    pending: agents.filter((a) => !a.migrated).map((a) => a.model),
    legacySeen: legacyUsage(),
  }
}

/** One operator-readable line, or null when there is nothing to say. Rendered
 *  into the fleet render's warnings (where an admin actually looks) and logged.
 *  This is the guard for the migration order — flipping the flag before the
 *  fleet is rolled is a total outage, and nothing else says so. */
export function legacyMigrationWarning(s: LegacyMigrationStatus): string | null {
  if (!s.pending.length) return null
  const who = s.pending.join(', ')
  if (!s.windowOpen) {
    // The trap sprung: every pending agent is ALREADY failing auth. Say it
    // instead of leaving a pile of undiagnosable 401s to be read.
    return `TALARIA_AGENT_KEY_LEGACY=off but ${s.pending.length} agent(s) have never authenticated with their own credential — they are locked out right now: ${who}. Roll their containers (render → roll_running_agents), or set the flag back to 'on' until they have.`
  }
  return `per-agent credential migration: ${s.agents.length - s.pending.length}/${s.agents.length} done. Still on the org-wide key (or never seen since): ${who}. Roll their containers before setting TALARIA_AGENT_KEY_LEGACY=off — flipping it first is a fleet-wide outage.`
}

/** Resolve the calling agent from its credential:
 *   • `null`        no agent credential presented — dual-auth routes fall
 *                   through to session auth exactly as before
 *   • `Response`    a credential WAS presented and rejected (unknown, retired,
 *                   contradicted by x-agent-name, or a legacy caller claiming a
 *                   name that doesn't exist / carries human privilege); return
 *                   it, because falling through would turn a forgery into a
 *                   quiet 401
 *   • `AgentCaller` identified (always a real, enabled agent)
 *
 *  Usage matches api-guard's guards:
 *
 *    const agent = await agentCaller(request)
 *    if (agent instanceof Response) return agent
 *    if (agent) { … }
 */
export async function agentCaller(request: Request): Promise<AgentCaller | Response | null> {
  return resolve(request, { requireName: true })
}

/** Identity for the FLEET-PLANE endpoints whose subject is in the URL
 *  (register, heartbeat). Same validation as `agentCaller`, except a legacy
 *  caller that sent no x-agent-name resolves to `{ model: null }` instead of a
 *  400: the pre-per-key plugin doesn't send the header, and the subject is the
 *  URL anyway. A caller we CAN name must still match the subject — that is what
 *  stops agent A reading agent B's work. */
export async function fleetCaller(request: Request): Promise<{ model: string | null } | Response | null> {
  const caller = await resolve(request, { requireName: false })
  if (caller instanceof Response || caller === null) return caller
  return { model: caller.model || null }
}

/** Shared resolution. `requireName: false` allows an unnamed legacy caller
 *  through with an EMPTY model — never an identity, only "a fleet credential". */
async function resolve(request: Request, opts: { requireName: boolean }): Promise<AgentCaller | Response | null> {
  const secret = presented(request)
  if (!secret) return null
  const claimed = declaredName(request)

  if (secret.startsWith(KEY_PREFIX)) {
    const sql = await db()
    const rows = (await sql`
      select d.id, d.model, d.enabled from agent_keys k
      join agent_defs d on d.id = k.agent_id
      where k.key_hash = ${hash(secret)}
    `) as unknown as Array<{ id: string; model: string; enabled: boolean }>
    const row = rows[0]
    if (!row) return json({ error: 'unknown agent credential' }, { status: 401 })
    if (!row.enabled) return json({ error: 'this agent is retired' }, { status: 403 })
    // The cross-check. A credential that says one thing and a header that says
    // another is a misconfiguration at best and impersonation at worst; refuse
    // rather than silently pick one of the two identities.
    if (claimed && claimed !== row.model) {
      return json({ error: `x-agent-name "${claimed}" does not match the presenting agent` }, { status: 403 })
    }
    void sql`update agent_keys set last_used_at = now() where agent_id = ${row.id}`.catch(() => {})
    return { id: row.id, model: row.model, legacy: false }
  }

  const shared = (process.env.TALARIA_AGENT_KEY ?? '').trim()
  // Not a credential we issued — leave the request to whatever else
  // authenticates this route (session cookie, gateway key).
  if (!shared || !eq(secret, shared)) return null
  if (!legacyOpen()) {
    return json(
      {
        error: "the org-wide agent key is retired — present the agent's own credential",
        message: `The org-wide TALARIA_AGENT_KEY no longer authenticates anyone${claimed ? ` (claimed: "${claimed}")` : ''}. ${ROLL_IT}`,
      },
      { status: 401 },
    )
  }
  // The shared key proves fleet membership and nothing else, so the header is
  // all there is. An unnamed caller gets no identity and therefore no
  // per-agent access — it is refused, not waved through (the fleet plane, whose
  // subject is the URL, opts out via fleetCaller).
  if (!claimed) {
    if (opts.requireName) return json({ error: 'x-agent-name required' }, { status: 400 })
    return { id: null, model: '', legacy: true }
  }
  // The claimed name is resolved, never taken on faith: a retired or invented
  // name must not authenticate, or retiring an agent revokes nothing and a
  // forged string lands in usage rows, task activity and channel authorship.
  const sql = await db()
  const rows = (await sql`
    select id, slug, model, enabled, owner_user_id is not null as "personal", elevated
    from agent_defs where model = ${claimed}
  `) as unknown as Array<{ id: string; slug: string; model: string; enabled: boolean; personal: boolean; elevated: boolean }>
  const def = rows[0]
  if (!def) return json({ error: `unknown agent "${claimed}"` }, { status: 403 })
  if (!def.enabled) return json({ error: 'this agent is retired' }, { status: 403 })
  // A name that carries HUMAN privilege can't be asserted, only proven. Every
  // capability such an identity has — its owner's boards, KB, Google account,
  // org-wide reach — is escalation, so there is nothing left to keep working;
  // refuse with the fix instead of half-serving it (the operator would
  // otherwise see unexplained 401/403s from the owner-scoped tools).
  if (def.personal || def.elevated) {
    // Also LOG it: dual-auth routes turn a rejection into a plain 401 for the
    // caller (actingUser swallows the Response), so the log is where an
    // operator finds out that an assistant needs its container rolled.
    warnOnce(
      `refused:${def.model}`,
      `[agent-auth] "${def.model}" presented the org-wide TALARIA_AGENT_KEY but acts for a human — refused. Re-render the fleet and roll its container onto TALARIA_AGENT_KEY_${def.slug.toUpperCase()}.`,
      'error',
    )
    return json(
      {
        error: 'this agent must present its own credential',
        message: `"${def.model}" acts for a human (personal assistant / elevated), so the org-wide TALARIA_AGENT_KEY cannot authenticate it — it proves fleet membership, not identity. Re-render the fleet and roll this container so it presents TALARIA_AGENT_KEY_${def.slug.toUpperCase()}.`,
      },
      { status: 403 },
    )
  }
  warnLegacy(def.model)
  // id stays null: the identity is asserted, not proven, and `id` is what a
  // surface keys proof off.
  return { id: null, model: def.model, legacy: true }
}

/** The calling agent or a ready-to-return 401/403, for agent-only routes. */
export async function requireAgent(request: Request): Promise<AgentCaller | Response> {
  const caller = await agentCaller(request)
  if (!caller) return json({ error: 'unauthorized' }, { status: 401 })
  return caller
}

/** Any credential the fleet holds — a per-agent one, or the org-wide key while
 *  the window is open. Only for the fleet-plane endpoints that carry their
 *  subject in the URL (register, heartbeat) and so need no caller identity.
 *
 *  Retirement is `agent_defs.enabled = false` (agent-defs.ts) and it does NOT
 *  delete the agent_keys row, so the join onto agent_defs is what makes
 *  "retiring an agent revokes its access" true here as well as in `resolve()`.
 *  Checking agent_keys alone left a retired agent's credential working on every
 *  route that guards with this. */
export async function checkFleetKey(request: Request): Promise<boolean> {
  const secret = presented(request)
  if (!secret) return false
  if (secret.startsWith(KEY_PREFIX)) {
    const sql = await db()
    const rows = await sql`
      select 1 from agent_keys k join agent_defs d on d.id = k.agent_id
      where k.key_hash = ${hash(secret)} and d.enabled
    `
    return rows.length > 0
  }
  const shared = (process.env.TALARIA_AGENT_KEY ?? '').trim()
  return !!shared && legacyOpen() && eq(secret, shared)
}
