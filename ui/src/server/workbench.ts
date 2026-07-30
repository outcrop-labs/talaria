// Workbench runtime profiles — the role-agnostic sandbox methodology. A
// profile is a chassis overlay (image + env + mounts + the harnesses it
// preinstalls) plus autoAttach fit rules; 'dev' ships seeded, and designer /
// data / marketing workbenches ride the exact same table later. Six pieces
// make every workbench: the image, scoped creds (phase 2), the governed
// workbench MCP (phase 3), effort→model routing (phase 4), MCP pass-through
// (phase 6), and the shared audit spine.
//
// The per-agent control is ONE setting on agent_defs:
//   workbench: 'off' | 'auto' | 'on'   (+ optional explicit profile slug)
// 'auto' attaches when a profile's fit rules match the agent (department /
// role); 'on' forces the explicit profile (else best fit, else 'dev').
import { db } from './db/pg'

export interface WorkbenchProfile {
  slug: string
  name: string
  description: string
  /** Container image override; '' = keep the chassis image. */
  image: string
  env: Record<string, string>
  /** Compose volume strings ("name-or-path:/dest[:ro]"). */
  mounts: string[]
  /** Harness slugs this profile preinstalls (adapter registry, phase 4). */
  harnesses: string[]
  autoAttach: { departments?: string[]; roles?: string[] }
  /** Room for the later phases: creds scoping, toolkit verbs, effort map. */
  config: Record<string, unknown>
  enabled: boolean
}

const ROW = `slug, name, description, image, env, mounts, harnesses, auto_attach as "autoAttach", config, enabled`

/** The shipped default — a coding workbench for dev-leaning agents. Seeded
 *  once; admins tune it from the API afterwards (never re-clobbered). */
const DEV_SEED = {
  slug: 'dev',
  name: 'Coding workbench',
  description:
    'A sandboxed development environment: coding harnesses (opencode, claude code, codex) working repo checkouts under the platform-owned git flow.',
  env: { TALARIA_WORKBENCH: 'dev' },
  harnesses: ['opencode', 'claude-code', 'codex', 'oh-my-pi'],
  autoAttach: { departments: ['engineering'], roles: ['engineer', 'developer'] },
}

let seeded = false
async function ensureSeed(): Promise<void> {
  if (seeded) return
  const sql = await db()
  await sql`
    insert into workbench_profiles (slug, name, description, env, harnesses, auto_attach)
    values (${DEV_SEED.slug}, ${DEV_SEED.name}, ${DEV_SEED.description}, ${sql.json(DEV_SEED.env)},
            ${sql.json(DEV_SEED.harnesses)}, ${sql.json(DEV_SEED.autoAttach)})
    on conflict (slug) do nothing
  `
  seeded = true
}

export async function listProfiles(): Promise<WorkbenchProfile[]> {
  await ensureSeed()
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from workbench_profiles order by slug`)) as unknown as WorkbenchProfile[]
}

export async function updateProfile(
  slug: string,
  patch: Partial<Pick<WorkbenchProfile, 'name' | 'description' | 'image' | 'env' | 'mounts' | 'harnesses' | 'autoAttach' | 'config' | 'enabled'>>,
): Promise<boolean> {
  const sql = await db()
  const rows = await sql`
    update workbench_profiles set
      name = coalesce(${patch.name ?? null}, name),
      description = coalesce(${patch.description ?? null}, description),
      image = coalesce(${patch.image ?? null}, image),
      enabled = coalesce(${patch.enabled ?? null}, enabled),
      env = coalesce(${patch.env !== undefined ? sql.json(patch.env) : null}, env),
      mounts = coalesce(${patch.mounts !== undefined ? sql.json(patch.mounts) : null}, mounts),
      harnesses = coalesce(${patch.harnesses !== undefined ? sql.json(patch.harnesses) : null}, harnesses),
      auto_attach = coalesce(${patch.autoAttach !== undefined ? sql.json(patch.autoAttach) : null}, auto_attach),
      config = coalesce(${patch.config !== undefined ? sql.json(patch.config as Record<string, string>) : null}, config),
      updated_at = now()
    where slug = ${slug}
    returning slug
  `
  return rows.length > 0
}

const fits = (p: WorkbenchProfile, agent: { department: string; role: string | null }): boolean =>
  (p.autoAttach.departments ?? []).some((d) => d.toLowerCase() === agent.department.toLowerCase()) ||
  (!!agent.role && (p.autoAttach.roles ?? []).some((r) => agent.role!.toLowerCase().includes(r.toLowerCase())))

/** The profile an agent actually runs with, honoring THE setting.
 *  off → none · on → explicit pick, else best fit, else 'dev' · auto → fit. */
export async function resolveWorkbench(agent: {
  department: string
  role: string | null
  workbench: 'off' | 'auto' | 'on'
  workbenchProfile: string | null
}): Promise<WorkbenchProfile | null> {
  if (agent.workbench === 'off') return null
  const profiles = (await listProfiles()).filter((p) => p.enabled)
  if (agent.workbench === 'on') {
    return (
      profiles.find((p) => p.slug === agent.workbenchProfile) ??
      profiles.find((p) => fits(p, agent)) ??
      profiles.find((p) => p.slug === 'dev') ??
      null
    )
  }
  // auto: an explicit profile pick wins outright (the admin chose it); with
  // no pick, the autoAttach fit rules decide.
  if (agent.workbenchProfile) {
    const picked = profiles.find((p) => p.slug === agent.workbenchProfile)
    if (picked) return picked
  }
  return profiles.find((p) => fits(p, agent)) ?? null
}

export async function setAgentWorkbench(id: string, workbench: 'off' | 'auto' | 'on', profile?: string | null): Promise<void> {
  const sql = await db()
  await sql`update agent_defs set workbench = ${workbench}, updated_at = now() where id = ${id}`
  if (profile !== undefined) await sql`update agent_defs set workbench_profile = ${profile}, updated_at = now() where id = ${id}`
}

/** Per-agent workbench tuning: the harness pick + effort→model overrides —
 *  the knobs the agent-view dropdowns write. */
export async function setAgentWorkbenchTuning(
  id: string,
  patch: { harness?: string | null; models?: Partial<Record<'light' | 'standard' | 'heavy', string | null>> },
): Promise<void> {
  const sql = await db()
  if (patch.harness !== undefined) await sql`update agent_defs set workbench_harness = ${patch.harness}, updated_at = now() where id = ${id}`
  if (patch.models !== undefined) {
    const [cur] = (await sql`select workbench_models as m from agent_defs where id = ${id}`) as unknown as Array<{ m: Record<string, string> }>
    const next: Record<string, string> = { ...(cur?.m ?? {}) }
    for (const k of ['light', 'standard', 'heavy'] as const) {
      const v = patch.models[k]
      if (v === undefined) continue
      if (v === null || v === '') delete next[k]
      else next[k] = v
    }
    await sql`update agent_defs set workbench_models = ${sql.json(next)}, updated_at = now() where id = ${id}`
  }
}
