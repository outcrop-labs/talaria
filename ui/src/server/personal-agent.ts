// Personal assistants: each person can spin up their own Hermes agent from the
// dashboard — a real fleet agent (its own container, key, memory) owned by them,
// created from a base template and auto-allowed for its owner. This makes the
// "per-person agent" vision one button for the average user.
import { db } from './db/pg'
import { createAgent } from './fleet-create'
import { setAgentEnabled } from './agent-defs'
import { fleetUp, waitHealthy } from './fleet-docker'
import { renderFleet } from './fleet-render'
import { ensurePersonalCollection } from './retrieval/collections'
import { syncUserPrivateDocs } from './kb'

export interface PersonalAgent {
  id: string
  slug: string
  model: string
  displayName: string
  enabled: boolean
}

/** The user's personal agent, if they have one. */
export async function personalAgentFor(userId: string): Promise<PersonalAgent | null> {
  const sql = await db()
  const rows = (await sql`
    select id, slug, model, display_name as "displayName", enabled
    from agent_defs where owner_user_id = ${userId} limit 1
  `) as unknown as PersonalAgent[]
  return rows[0] ?? null
}

/** slug + department derived from the user — BOTH must be unique per user, since
 *  the department names the container (agent-<department>). createAgent enforces
 *  slug uniqueness and throws on collision. */
function personalIdentity(email: string | null, name: string | null): { slug: string; department: string } {
  const base = (email?.split('@')[0] ?? name ?? 'me').toLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 24) || 'me'
  return { slug: `pa${base}`.slice(0, 30), department: `personal-${base}`.slice(0, 40) }
}

/** Create + start a personal agent for the user, based on a template (any
 *  enabled agent). Returns the new def. */
export async function createPersonalAgent(user: {
  id: string
  email: string | null
  name: string | null
}): Promise<PersonalAgent> {
  const existing = await personalAgentFor(user.id)
  if (existing) {
    if (!existing.enabled) await setAgentEnabled(existing.id, true)
    // Make sure their personal RAG exists + their agent is bound to it.
    await ensurePersonalCollection(user.id, { agentModel: existing.model }).catch(() => {})
    void syncUserPrivateDocs(user.id).catch(() => {})
    return existing
  }

  const sql = await db()
  // Base template: prefer an existing personal assistant department, else any
  // enabled agent. (Role-ready base agents are a future refinement.)
  const tmpl = (await sql`
    select id from agent_defs where enabled
    order by (department = 'administrative-assistant') desc, updated_at desc limit 1
  `) as unknown as Array<{ id: string }>
  if (!tmpl[0]) throw new Error('no agent to base a personal assistant on — import a stack first')

  const displayName = (user.name?.split(' ')[0] ?? user.email?.split('@')[0] ?? 'My') + "'s assistant"
  const { slug, department } = personalIdentity(user.email, user.name)
  const { def } = await createAgent({
    slug,
    department,
    displayName,
    role: 'Personal assistant',
    templateId: tmpl[0].id,
    createdBy: user.email ?? user.name ?? 'user',
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

  return { id: def.id, slug: def.slug, model: def.model, displayName, enabled: true }
}
