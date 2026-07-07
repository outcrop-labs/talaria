// Personal assistants: each person can spin up their own Hermes agent from the
// dashboard — a real fleet agent (its own container, key, memory) owned by them,
// created from a base template and auto-allowed for its owner. The owner names
// it, picks its handle, and writes its personality — no admin role needed; every
// mutation here is scoped to `agent_defs.owner_user_id = user.id`.
import { db } from './db/pg'
import { createAgent } from './fleet-create'
import { addVersionIfChanged, listVersions, setAgentEnabled, updateAgentMeta } from './agent-defs'
import { containerStatus, fleetRestart, fleetUp, waitHealthy } from './fleet-docker'
import { renderFleet } from './fleet-render'
import { ensurePersonalCollection } from './retrieval/collections'
import { syncUserPrivateDocs } from './kb'

export interface PersonalAgent {
  id: string
  slug: string
  model: string
  department: string
  displayName: string
  enabled: boolean
  /** Owner-authored personality text (the marked soul section). */
  personality: string | null
  /** Container reality — matches the agents-roster dots. */
  running: boolean
}

/** The handle (slug) a user may pick — same alphabet as SLUG_RE in fleet-create. */
export const HANDLE_RE = /^[a-z][a-z0-9]{1,29}$/

// The owner edits one marked section of the soul; the rest of the soul (role
// scaffold, guardrails) stays out of their way. Markers are HTML comments so
// they're invisible wherever the soul renders as markdown.
const PERSONA_START = '<!-- talaria:personality -->'
const PERSONA_END = '<!-- /talaria:personality -->'

export function personalityOf(soul: string): string | null {
  const m = soul.indexOf(PERSONA_START)
  const e = soul.indexOf(PERSONA_END)
  if (m === -1 || e === -1 || e < m) return null
  return soul.slice(m + PERSONA_START.length, e).trim() || null
}

/** Replace (or append) the marked personality section of a soul. */
export function withPersonality(soul: string, personality: string): string {
  const block = `${PERSONA_START}\n${personality.trim()}\n${PERSONA_END}`
  const m = soul.indexOf(PERSONA_START)
  const e = soul.indexOf(PERSONA_END)
  if (m !== -1 && e !== -1 && e > m) return soul.slice(0, m) + block + soul.slice(e + PERSONA_END.length)
  return `${soul.trimEnd()}\n\n## Personality\n${block}\n`
}

const DEFAULT_PERSONALITY =
  'Be warm, direct, and useful. Lead with the answer, keep routine replies short, and ask rather than guess when a request is ambiguous.'

function personalSoul(displayName: string, ownerName: string, personality: string): string {
  return `# ${displayName} — ${ownerName}'s personal assistant

## Who you are
You are ${displayName}, ${ownerName}'s personal assistant inside Talaria. You work
for ${ownerName} specifically: their tasks, their notes, their preferences.

## Personality
${PERSONA_START}
${personality.trim()}
${PERSONA_END}

## How you work
- Keep ${ownerName} in the loop: create and triage tickets, never assign or close them.
- Prefer the local model tier for routine work; escalate deliberately.
- Remember durable preferences and context in your memory as you learn them.
`
}

/** The user's personal agent, if they have one. */
export async function personalAgentFor(userId: string): Promise<PersonalAgent | null> {
  const sql = await db()
  const rows = (await sql`
    select id, slug, model, department, display_name as "displayName", enabled
    from agent_defs where owner_user_id = ${userId} limit 1
  `) as unknown as Array<Omit<PersonalAgent, 'personality' | 'running'>>
  const def = rows[0]
  if (!def) return null
  const [latest, containers] = await Promise.all([
    listVersions(def.id).then((v) => v[0] ?? null),
    containerStatus([def.department]).catch(() => []),
  ])
  return {
    ...def,
    personality: latest ? personalityOf(latest.soul) : null,
    running: containers[0]?.managed?.state === 'running',
  }
}

/** slug + department derived from the user — BOTH must be unique per user, since
 *  the department names the container (agent-<department>). createAgent enforces
 *  slug uniqueness and throws on collision. */
function personalIdentity(email: string | null, name: string | null): { slug: string; department: string } {
  const base = (email?.split('@')[0] ?? name ?? 'me').toLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 24) || 'me'
  return { slug: `pa${base}`.slice(0, 30), department: `personal-${base}`.slice(0, 40) }
}

export interface PersonalAgentInput {
  /** Display name, e.g. "Maxie". */
  name?: string
  /** Handle → slug + container identity. Immutable after creation. */
  handle?: string
  personality?: string
}

/** Create + start a personal agent for the user, based on a template (any
 *  enabled agent). Returns the new def. Idempotent: an existing assistant is
 *  returned as-is (re-enabled if retired) — creation options don't apply. */
export async function createPersonalAgent(
  user: { id: string; email: string | null; name: string | null },
  input: PersonalAgentInput = {},
): Promise<PersonalAgent> {
  const existing = await personalAgentFor(user.id)
  if (existing) {
    if (!existing.enabled) await setAgentEnabled(existing.id, true)
    // Make sure their personal RAG exists + their agent is bound to it.
    await ensurePersonalCollection(user.id, { agentModel: existing.model }).catch(() => {})
    void syncUserPrivateDocs(user.id).catch(() => {})
    return { ...existing, enabled: true }
  }

  const sql = await db()
  // Base template: prefer an existing personal assistant department, else any
  // enabled agent. (Role-ready base agents are a future refinement.)
  const tmpl = (await sql`
    select id from agent_defs where enabled
    order by (department = 'administrative-assistant') desc, updated_at desc limit 1
  `) as unknown as Array<{ id: string }>
  if (!tmpl[0]) throw new Error('no agent to base a personal assistant on — import a stack first')

  const ownerName = user.name?.split(' ')[0] ?? user.email?.split('@')[0] ?? 'you'
  const displayName = input.name?.trim() || `${ownerName}'s assistant`
  const identity = input.handle
    ? { slug: input.handle, department: `personal-${input.handle}`.slice(0, 40) }
    : personalIdentity(user.email, user.name)
  const personality = input.personality?.trim() || DEFAULT_PERSONALITY

  const { def } = await createAgent({
    ...identity,
    displayName,
    role: 'Personal assistant',
    templateId: tmpl[0].id,
    createdBy: user.email ?? user.name ?? 'user',
    soul: personalSoul(displayName, ownerName, personality),
  })

  // Mark ownership + grant the user access to their own agent.
  await sql`update agent_defs set owner_user_id = ${user.id} where id = ${def.id}`
  await sql`insert into user_agent_access (user_id, agent_model) values (${user.id}, ${def.model}) on conflict do nothing`

  // Personal RAG: a private collection bound to the user + this agent, seeded
  // with any private docs they already have.
  await ensurePersonalCollection(user.id, { name: `${displayName} · knowledge`, agentModel: def.model }).catch(() => {})
  void syncUserPrivateDocs(user.id).catch(() => {})

  await renderFleet()
  await fleetUp(def.department).catch(() => {})
  void waitHealthy(def.department).catch(() => {})

  return {
    id: def.id,
    slug: def.slug,
    model: def.model,
    department: def.department,
    displayName,
    enabled: true,
    personality,
    running: true,
  }
}

/** Owner-scoped edits: rename and/or rewrite the personality. A personality (or
 *  name-in-soul) change lands as a new immutable version and is applied to the
 *  running container right away — same pipeline as the admin editor. */
export async function updatePersonalAgent(
  user: { id: string; email: string | null; name: string | null },
  patch: { name?: string; personality?: string },
): Promise<PersonalAgent> {
  const sql = await db()
  const rows = (await sql`
    select id, slug, department, display_name as "displayName", managed, enabled
    from agent_defs where owner_user_id = ${user.id} limit 1
  `) as unknown as Array<{ id: string; slug: string; department: string; displayName: string; managed: boolean; enabled: boolean }>
  const def = rows[0]
  if (!def) throw new Error('no assistant yet — create one first')

  const newName = patch.name?.trim()
  if (newName && newName !== def.displayName) await updateAgentMeta(def.id, { displayName: newName })

  const latest = (await listVersions(def.id))[0]
  if (latest) {
    let soul = latest.soul
    // Renames carry into the soul so the agent knows its own name; exact-match
    // replace is safe because display names are distinctive multi-char strings.
    if (newName && newName !== def.displayName && def.displayName.length >= 3) {
      soul = soul.split(def.displayName).join(newName)
    }
    if (patch.personality !== undefined) soul = withPersonality(soul, patch.personality)
    const { created } = await addVersionIfChanged(def.id, {
      soul,
      config: latest.config,
      note: 'personalized by owner',
      createdBy: user.email ?? user.name ?? 'owner',
    })
    if (created && def.managed && def.enabled) {
      await renderFleet()
      await fleetRestart(def.department).catch(() => {})
    }
  }

  const updated = await personalAgentFor(user.id)
  if (!updated) throw new Error('assistant vanished during update')
  return updated
}
