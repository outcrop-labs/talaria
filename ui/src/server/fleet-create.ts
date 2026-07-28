// Create a brand-new agent — from a template (an existing agent's definition,
// model tiers/tools cloned with identity re-stamped) or, with no template, from
// the platform defaults (first local endpoint's model). The soul is a scaffold
// (or supplied, e.g. Muse-designed). A fresh gateway key is allocated into the
// fleet .env (which the renderer + manifest read).
import { randomBytes } from 'node:crypto'
import { appendFile, readFile } from 'node:fs/promises'
import { db } from './db/pg'
import { addVersionIfChanged, applyConfigEdits, listEndpoints, listVersions, upsertAgentDef, type AgentConfig, type AgentDef } from './agent-defs'
import { FLEET_ENV } from './fleet-render'

const SLUG_RE = /^[a-z][a-z0-9]{1,30}$/
const DEPT_RE = /^[a-z][a-z0-9-]{1,40}$/

/** Re-stamp identity: replace a slug in every string value of the raw config
 *  (X-Agent-Name headers, hook args like "outline_org_gate.py sam"). Used for
 *  template cloning and for handle renames. */
export function restampSlug(value: unknown, from: string, to: string): unknown {
  if (typeof value === 'string') return value.replace(new RegExp(`\\b${from}\\b`, 'g'), to)
  if (Array.isArray(value)) return value.map((v) => restampSlug(v, from, to))
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([k, v]) => [k, restampSlug(v, from, to)]))
  }
  return value
}

function starterSoul(displayName: string, department: string): string {
  const role = department.replace(/-/g, ' ')
  return `# ${displayName} — ${role}

## Who you are
You are ${displayName}, the ${role} agent. (Written by Talaria's template —
replace this section with a real personality and operating principles.)

## How you work
- Keep humans in the loop: create and triage tickets, never assign or close them.
- Prefer the local model tier for routine work; escalate deliberately.
- When unsure, ask in the channel instead of guessing.
`
}

/** Ensure a HERMES_KEY_<SLUG> exists in the fleet .env; returns whether created. */
export async function ensureAgentKey(slug: string): Promise<boolean> {
  const envPath = FLEET_ENV()
  const name = `HERMES_KEY_${slug.toUpperCase()}`
  const content = await readFile(envPath, 'utf8')
  if (new RegExp(`^${name}=`, 'm').test(content)) return false
  const key = randomBytes(32).toString('hex')
  await appendFile(envPath, `\n# added by Talaria (agent create)\n${name}=${key}\n`)
  return true
}

export async function createAgent(input: {
  slug: string
  department: string
  displayName: string
  role?: string | null
  /** Clone this agent's config; omit to start from the platform defaults. */
  templateId?: string
  createdBy: string
  /** Override the starter-soul scaffold (e.g. a personalized assistant soul). */
  soul?: string
}): Promise<{ def: AgentDef; keyCreated: boolean }> {
  if (!SLUG_RE.test(input.slug)) throw new Error('slug must be short lowercase alphanumeric (e.g. "remy")')
  if (!DEPT_RE.test(input.department)) throw new Error('department must be lowercase-kebab (e.g. "research")')

  const sql = await db()
  const exists = await sql`select 1 from agent_defs where slug = ${input.slug}`
  if (exists.length) throw new Error(`agent "${input.slug}" already exists`)

  let config: AgentConfig
  let note: string
  if (input.templateId) {
    const templateVersion = (await listVersions(input.templateId))[0]
    if (!templateVersion) throw new Error('template agent has no versions')
    const tmplDef = (await sql`select slug from agent_defs where id = ${input.templateId}`) as unknown as Array<{
      slug: string
    }>
    const fromSlug = tmplDef[0]?.slug
    if (!fromSlug) throw new Error('template agent not found')
    config = restampSlug(templateVersion.config, fromSlug, input.slug) as AgentConfig
    note = `created from template ${fromSlug}`
  } else {
    // Platform defaults: main model from the first local endpoint (else any).
    const eps = await listEndpoints()
    const ep = eps.find((e) => e.class === 'local' && e.models.length > 0) ?? eps.find((e) => e.models.length > 0)
    if (!ep) throw new Error('no models configured — add an LLM endpoint first')
    const main = {
      endpoint: ep.name,
      model: ep.models[0]!,
      ...(ep.contextLength ? { contextLength: ep.contextLength } : {}),
    }
    config = await applyConfigEdits({ main, aliases: [], fallbacks: [] }, { main, aliases: [], fallbacks: [] })
    note = 'created from platform defaults'
  }

  const keyCreated = await ensureAgentKey(input.slug)

  const def = await upsertAgentDef({ slug: input.slug, department: input.department, displayName: input.displayName, role: input.role, source: 'created' })
  await sql`update agent_defs set managed = true, updated_at = now() where id = ${def.id}`

  await addVersionIfChanged(def.id, {
    soul: input.soul ?? starterSoul(input.displayName, input.department),
    config,
    note,
    createdBy: input.createdBy,
  })
  return { def: { ...def, managed: true }, keyCreated }
}

/** Permanently delete a RETIRED agent: the def row (versions + secrets
 *  cascade), any leftover containers, the rendered agent dir, its fleet key,
 *  and — for Talaria-created agents — the state volume. Imported agents keep
 *  their pre-Talaria volume (it predates us; deleting it isn't ours to do).
 *  History that references the agent by model string (ledger, messages,
 *  tickets) is deliberately kept. */
export async function deleteAgentForever(defId: string): Promise<{ removedVolume: boolean }> {
  const sql = await db()
  const rows = (await sql`
    select slug, department, source, enabled from agent_defs where id = ${defId}
  `) as unknown as Array<{ slug: string; department: string; source: string; enabled: boolean }>
  const def = rows[0]
  if (!def) throw new Error('not found')
  if (def.enabled) throw new Error('retire the agent first — delete is for retired agents only')

  const { removeContainerByName, slotContainer } = await import('./fleet-docker')
  const { renderFleet, FLEET_DIR } = await import('./fleet-render')
  const { rm } = await import('node:fs/promises')
  const { join } = await import('node:path')
  const { execFile } = await import('node:child_process')

  // Containers first (should already be gone after retire; both slots, best-effort).
  await removeContainerByName(slotContainer(def.department, 'a')).catch(() => {})
  await removeContainerByName(slotContainer(def.department, 'b')).catch(() => {})

  // The def row — versions + secrets cascade with it.
  await sql`delete from agent_defs where id = ${defId}`

  // Rendered dir + fleet key line (best-effort cleanup).
  await rm(join(FLEET_DIR(), 'agents', def.slug), { recursive: true, force: true }).catch(() => {})
  try {
    const envPath = FLEET_ENV()
    const content = await readFile(envPath, 'utf8')
    const keyLine = new RegExp(`^(# added by Talaria \\(agent create\\)\\n)?HERMES_KEY_${def.slug.toUpperCase()}=.*\\n?`, 'm')
    const next = content.replace(keyLine, '')
    if (next !== content) {
      const { writeFile } = await import('node:fs/promises')
      await writeFile(envPath, next)
    }
  } catch {
    /* keep going — a stale key line is harmless */
  }

  // State volume: only for created agents (imported volumes are external legacy).
  let removedVolume = false
  if (def.source === 'created') {
    removedVolume = await new Promise<boolean>((res) => {
      execFile('docker', ['volume', 'rm', `talaria-fleet_hermes-${def.department}`], { timeout: 20_000 }, (err) => res(!err))
    })
  }

  await renderFleet() // compose + manifest drop the agent
  return { removedVolume }
}
