// Create a brand-new agent from a template (an existing agent's definition).
// The clone carries the template's model tiers/tools with every identity
// reference re-stamped to the new slug; the soul is a role-scaffold the user
// edits in Talaria. A fresh gateway key is allocated into the stack's .env
// (which the renderer + manifest already read).
import { randomBytes } from 'node:crypto'
import { appendFile, readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { db } from './db/pg'
import { addVersionIfChanged, listVersions, upsertAgentDef, type AgentConfig, type AgentDef } from './agent-defs'
import { STACK_DIR } from './fleet-render'

const SLUG_RE = /^[a-z][a-z0-9]{1,30}$/
const DEPT_RE = /^[a-z][a-z0-9-]{1,40}$/

/** Re-stamp identity: replace the template's slug in every string value of the
 *  raw config (X-Agent-Name headers, hook args like "outline_org_gate.py sam"). */
function restampSlug(value: unknown, from: string, to: string): unknown {
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

/** Ensure a HERMES_KEY_<SLUG> exists in the stack .env; returns whether created. */
async function ensureAgentKey(slug: string): Promise<boolean> {
  const envPath = join(STACK_DIR(), '.env')
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
  templateId: string
  createdBy: string
}): Promise<{ def: AgentDef; keyCreated: boolean }> {
  if (!SLUG_RE.test(input.slug)) throw new Error('slug must be short lowercase alphanumeric (e.g. "remy")')
  if (!DEPT_RE.test(input.department)) throw new Error('department must be lowercase-kebab (e.g. "research")')

  const sql = await db()
  const exists = await sql`select 1 from agent_defs where slug = ${input.slug}`
  if (exists.length) throw new Error(`agent "${input.slug}" already exists`)

  const templateVersion = (await listVersions(input.templateId))[0]
  if (!templateVersion) throw new Error('template agent has no versions')
  const tmplDef = (await sql`select slug from agent_defs where id = ${input.templateId}`) as unknown as Array<{
    slug: string
  }>
  const fromSlug = tmplDef[0]?.slug
  if (!fromSlug) throw new Error('template agent not found')

  const keyCreated = await ensureAgentKey(input.slug)

  const def = await upsertAgentDef({ slug: input.slug, department: input.department, displayName: input.displayName, role: input.role, source: 'created' })
  await sql`update agent_defs set managed = true, updated_at = now() where id = ${def.id}`

  const config = restampSlug(templateVersion.config, fromSlug, input.slug) as AgentConfig
  await addVersionIfChanged(def.id, {
    soul: starterSoul(input.displayName, input.department),
    config,
    note: `created from template ${fromSlug}`,
    createdBy: input.createdBy,
  })
  return { def: { ...def, managed: true }, keyCreated }
}
