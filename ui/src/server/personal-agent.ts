// Personal assistants: each person can spin up their own Hermes agent from the
// dashboard — a real fleet agent (its own container, key, memory) owned by them,
// created from a base template and auto-allowed for its owner. The owner names
// it, picks its handle, and writes its personality — no admin role needed; every
// mutation here is scoped to `agent_defs.owner_user_id = user.id`.
import { rename as fsRename } from 'node:fs/promises'
import { join } from 'node:path'
import { db } from './db/pg'
import { createAgent, ensureAgentKey, restampSlug } from './fleet-create'
import { addVersionIfChanged, applyConfigEdits, listVersions, setAgentEnabled, updateAgentMeta, type AgentConfig } from './agent-defs'
import { containerStatus, fleetRestart, fleetUp, waitHealthy } from './fleet-docker'
import { FLEET_DIR, renderFleet } from './fleet-render'
import { ensurePersonalCollection } from './retrieval/collections'
import { syncUserPrivateDocs } from './kb'

export interface AssistantTier {
  name: string
  model: string
  /** This tier is the current default (main) target. */
  active: boolean
}

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
  /** The main target's model, e.g. "qwen3-a3b" — what powers it right now. */
  currentModel: string | null
  /** Named model tiers the owner can switch between (the config aliases). */
  tiers: AssistantTier[]
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
  `) as unknown as Array<Omit<PersonalAgent, 'personality' | 'running' | 'currentModel' | 'tiers'>>
  const def = rows[0]
  if (!def) return null
  const [latest, containers] = await Promise.all([
    listVersions(def.id).then((v) => v[0] ?? null),
    containerStatus([def.department]).catch(() => []),
  ])
  const main = latest?.config.main
  return {
    ...def,
    personality: latest ? personalityOf(latest.soul) : null,
    running: containers[0]?.managed?.state === 'running',
    currentModel: main?.model ?? null,
    tiers: (latest?.config.aliases ?? []).map((a) => ({
      name: a.name,
      model: a.model,
      active: !!main && a.endpoint === main.endpoint && a.model === main.model,
    })),
  }
}

/** Does this user own the agent (by slug or def id)? Used to open selected
 *  admin surfaces (skills, memory, start/stop) to an assistant's owner. */
export async function ownsAgent(userId: string, ref: { slug?: string; defId?: string }): Promise<boolean> {
  const sql = await db()
  try {
    const rows = ref.slug
      ? await sql`select 1 from agent_defs where owner_user_id = ${userId} and slug = ${ref.slug}`
      : ref.defId
        ? await sql`select 1 from agent_defs where owner_user_id = ${userId} and id = ${ref.defId}`
        : []
    return rows.length > 0
  } catch {
    return false // e.g. a non-uuid defId
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
  // CHASSIS, not identity. The only thing an existing agent supplies here is
  // model tiers, tools and plugins — the assistant's soul, role and department
  // are all written below. Cloning one when it exists is a convenience: a new
  // assistant inherits whatever endpoint the fleet already runs on.
  //
  // It used to be a REQUIREMENT, and it failed the case it most needed to
  // serve: a fresh install has no agents, so the first thing a new user asked
  // for — their own assistant — answered "import a stack first". There is
  // nothing to import; Talaria is the stack. `createAgent` has always fallen
  // back to platform defaults (the first local endpoint, else any), so the
  // absence of a template is simply the absence of a shortcut. If there is no
  // endpoint either, createAgent says THAT, which is the real missing piece.
  const tmpl = (await sql`
    select id from agent_defs where enabled
    order by (department = 'administrative-assistant') desc, updated_at desc limit 1
  `) as unknown as Array<{ id: string }>

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
    ...(tmpl[0] ? { templateId: tmpl[0].id } : {}),
    createdBy: user.email ?? user.name ?? 'user',
    soul: personalSoul(displayName, ownerName, personality),
  })

  // Mark ownership + grant the user access to their own agent.
  await sql`update agent_defs set owner_user_id = ${user.id} where id = ${def.id}`
  await sql`insert into user_agent_access (user_id, agent_model) values (${user.id}, ${def.model}) on conflict do nothing`
  // ...and start it ALLOWED on the boards it will be told it owns: GET /api/boards
  // owner-proxies the owner's boards to a personal assistant from the moment it
  // exists, and every board-scoped route answers the allowlist — without this,
  // a fresh assistant sees boards it can only ever 403 against (boards created
  // before it existed). Boards the owner does NOT own stay the board owner's
  // call, and a removal via set_board_agents is never re-added.
  await sql`
    insert into board_agents (board_id, agent_model)
    select id, ${def.model} from boards where owner_id = ${user.id}
    on conflict do nothing
  `

  // Personal RAG: a private collection bound to the user + this agent, seeded
  // with any private docs they already have.
  await ensurePersonalCollection(user.id, { name: `${displayName} · knowledge`, agentModel: def.model }).catch(() => {})
  void syncUserPrivateDocs(user.id).catch(() => {})

  await renderFleet()
  await fleetUp(def.department).catch(() => {})
  void waitHealthy(def.department).catch(() => {})

  const created = await personalAgentFor(user.id)
  if (!created) throw new Error('assistant vanished during creation')
  return created
}

/** Rename the slug (@handle). The department — container name + state volume —
 *  stays put, so the agent keeps its memory and workspace. Everything keyed by
 *  the model string follows: access grants, chat history, board/channel
 *  membership, usage attribution, the heartbeat registry. The old
 *  HERMES_KEY_<SLUG> line is left in the stack .env (harmless orphan). */
async function renameAgentSlug(
  def: { id: string; slug: string; model: string; department: string },
  newSlug: string,
): Promise<string> {
  const sql = await db()
  const exists = await sql`select 1 from agent_defs where slug = ${newSlug}`
  if (exists.length) throw new Error(`agent "${newSlug}" already exists`)
  const newModel = `${newSlug}-${def.department}`
  await ensureAgentKey(newSlug)
  // Created agents keep their skills under fleet/agents/<slug>/ — carry them.
  await fsRename(join(FLEET_DIR(), 'agents', def.slug), join(FLEET_DIR(), 'agents', newSlug)).catch(() => {})
  await sql`update agent_defs set slug = ${newSlug}, model = ${newModel}, updated_at = now() where id = ${def.id}`
  await sql`update user_agent_access set agent_model = ${newModel} where agent_model = ${def.model}`
  await sql`update conversations set agent_model = ${newModel} where agent_model = ${def.model}`
  await sql`update board_agents set agent_model = ${newModel} where agent_model = ${def.model}`
  await sql`update channel_agents set agent_model = ${newModel} where agent_model = ${def.model}`
  await sql`update usage_events set agent_model = ${newModel} where agent_model = ${def.model}`
  await sql`update rag_collection_access set principal_id = ${newModel} where principal_type = 'agent' and principal_id = ${def.model}`
  await sql`delete from fleet_agents where name = ${newModel}`
  await sql`update fleet_agents set name = ${newModel} where name = ${def.model}`
  return newModel
}

/** Owner-scoped edits: rename, change the @handle, rewrite the personality, or
 *  switch the default model tier. Config/soul changes land as one new immutable
 *  version and are applied to the running container right away — same pipeline
 *  as the admin editor. */
export async function updatePersonalAgent(
  user: { id: string; email: string | null; name: string | null },
  patch: { name?: string; handle?: string; personality?: string; model?: string },
): Promise<PersonalAgent> {
  const sql = await db()
  const rows = (await sql`
    select id, slug, model, department, display_name as "displayName", managed, enabled
    from agent_defs where owner_user_id = ${user.id} limit 1
  `) as unknown as Array<{
    id: string
    slug: string
    model: string
    department: string
    displayName: string
    managed: boolean
    enabled: boolean
  }>
  const def = rows[0]
  if (!def) throw new Error('no assistant yet. Create one first.')

  const newName = patch.name?.trim()
  const renamed = !!newName && newName !== def.displayName
  if (renamed) await updateAgentMeta(def.id, { displayName: newName })

  const newHandle = patch.handle?.trim()
  const rehandled = !!newHandle && newHandle !== def.slug
  if (rehandled) {
    if (!HANDLE_RE.test(newHandle)) throw new Error('handles are 2–30 lowercase letters/numbers, starting with a letter')
    await renameAgentSlug(def, newHandle)
  }

  const latest = (await listVersions(def.id))[0]
  if (latest) {
    let soul = latest.soul
    let config = latest.config
    if (rehandled) config = restampSlug(config, def.slug, newHandle) as AgentConfig
    if (patch.model) {
      const aliases = config.aliases ?? []
      const target = aliases.find((a) => a.name === patch.model)
      if (!target) throw new Error(`unknown model tier "${patch.model}"`)
      config = await applyConfigEdits(config, {
        main: { endpoint: target.endpoint, model: target.model, ...(target.contextLength ? { contextLength: target.contextLength } : {}) },
        aliases,
        fallbacks: config.fallbacks ?? [],
      })
    }
    // Renames carry into the soul so the agent knows its own name; exact-match
    // replace is safe because display names are distinctive multi-char strings.
    if (renamed && def.displayName.length >= 3) soul = soul.split(def.displayName).join(newName)
    if (patch.personality !== undefined) soul = withPersonality(soul, patch.personality)
    const { created } = await addVersionIfChanged(def.id, {
      soul,
      config,
      note: 'personalized by owner',
      createdBy: user.email ?? user.name ?? 'owner',
    })
    if ((created || rehandled) && def.managed && def.enabled) {
      await renderFleet()
      // A handle rename changes the service definition (key env, model name,
      // config-mount path), so the container must be RECREATED (up -d), not
      // restarted — restart keeps the old mounts and crashes on the moved dir.
      if (rehandled) await fleetUp(def.department).catch(() => {})
      else await fleetRestart(def.department).catch(() => {})
    }
  }

  const updated = await personalAgentFor(user.id)
  if (!updated) throw new Error('assistant vanished during update')
  return updated
}
