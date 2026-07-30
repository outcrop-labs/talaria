// Agent skills as files — the way Hermes actually consumes them. Every agent
// mounts two skill roots read-only, both Talaria-owned:
//   /opt/skills       ← <fleet>/skills                (shared across the fleet)
//   /opt/dept-skills  ← <fleet>/agents/<slug>/skills  (the agent's own)
// Hermes reads skills per invocation, so edits here are live — no restart.
// Each skill is a directory holding a SKILL.md (plus optional support files).
import { cp, mkdir, readdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { db } from './db/pg'
import { FLEET_DIR } from './fleet-render'
import { snapshot } from './internal-history'
import { dropSummary, moveSummary, queueSummary, skillHash, storedSummaries } from './skill-summaries'

/** Owner key: 'shared' or an agent slug. */
export const SHARED = 'shared'

/** PLATFORM skills — the canonical set Talaria seeds into the shared root
 *  (scripts/skills/*). Essential plumbing like talaria-toolkit: every agent
 *  depends on them, so editing/renaming/deleting is locked to admins. */
export async function platformSkillNames(): Promise<Set<string>> {
  const entries = await readdir(resolve(process.cwd(), '../scripts/skills'), { withFileTypes: true }).catch(() => [])
  return new Set(entries.filter((e) => e.isDirectory()).map((e) => e.name))
}
const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/

interface OwnerInfo {
  owner: string
  label: string
  root: string
  source: 'shared' | 'imported' | 'created'
  /** The agent's model id (absent for the shared root) — what
   *  user_agent_access grants reference. */
  model?: string
}

async function owners(): Promise<OwnerInfo[]> {
  const sql = await db()
  const defs = (await sql`
    select slug, model, department, display_name as "displayName", source
    from agent_defs where enabled order by slug
  `) as unknown as Array<{ slug: string; model: string; department: string; displayName: string; source: string }>
  return [
    { owner: SHARED, label: 'Shared (all agents)', root: join(FLEET_DIR(), 'skills'), source: 'shared' as const },
    ...defs.map((d) => ({
      owner: d.slug,
      label: d.displayName,
      source: d.source as 'imported' | 'created',
      model: d.model,
      root: join(FLEET_DIR(), 'agents', d.slug, 'skills'),
    })),
  ]
}

/** The agent model behind an owner slug (undefined for 'shared'/unknown). */
export async function ownerModel(owner: string): Promise<string | undefined> {
  return (await owners()).find((o) => o.owner === owner)?.model
}

async function ownerRoot(owner: string): Promise<OwnerInfo> {
  const info = (await owners()).find((o) => o.owner === owner)
  if (!info) throw new Error(`unknown owner "${owner}"`)
  return info
}

/** Resolve a skill dir under its owner root, refusing path escapes. */
function safeJoin(root: string, name: string): string {
  if (!NAME_RE.test(name)) throw new Error('invalid skill name')
  const p = resolve(root, name)
  if (!p.startsWith(resolve(root) + '/')) throw new Error('invalid skill name')
  return p
}

export interface SkillSummary {
  name: string
  /** The Summarizer's line (fallback: first prose line of SKILL.md). */
  description: string
  files: string[]
  /** Canonical seeded platform skill (shared root only) — admin-locked. */
  platform?: boolean
}

export interface OwnerSkills {
  owner: string
  label: string
  source: 'shared' | 'imported' | 'created'
  model?: string
  skills: SkillSummary[]
}

/** Mechanical fallback while the Summarizer hasn't produced a line yet. */
function summarize(md: string): string {
  for (const line of md.split('\n')) {
    const t = line.trim()
    if (!t || t.startsWith('#') || t.startsWith('---')) continue
    // Frontmatter-style "description: " wins if present.
    const fm = /^description:\s*(.+)$/.exec(t)
    return (fm ? fm[1]! : t).slice(0, 160)
  }
  return ''
}

export async function listAllSkills(): Promise<OwnerSkills[]> {
  // Persistent one-liners from the Summarizer, keyed to content hash — a
  // changed skill serves the mechanical first-line summary once while its
  // regeneration runs in the background.
  const stored = await storedSummaries().catch(() => new Map<string, { hash: string; summary: string }>())
  const platform = await platformSkillNames()
  const out: OwnerSkills[] = []
  for (const o of await owners()) {
    const entries = await readdir(o.root, { withFileTypes: true }).catch(() => [])
    const skills: SkillSummary[] = []
    for (const e of entries) {
      if (!e.isDirectory() || !NAME_RE.test(e.name)) continue
      const dir = join(o.root, e.name)
      const files = (await readdir(dir).catch(() => [])).filter((f) => !f.startsWith('.'))
      const md = await readFile(join(dir, 'SKILL.md'), 'utf8').catch(() => '')
      const row = stored.get(`${o.owner}/${e.name}`)
      const fresh = !!row && !!md && row.hash === skillHash(md)
      if (!fresh && md.trim()) queueSummary(o.owner, e.name, md)
      skills.push({
        name: e.name,
        description: fresh ? row.summary : summarize(md),
        files,
        ...(o.owner === SHARED && platform.has(e.name) ? { platform: true } : {}),
      })
    }
    out.push({ owner: o.owner, label: o.label, source: o.source, model: o.model, skills: skills.sort((a, b) => a.name.localeCompare(b.name)) })
  }
  return out
}

export async function readSkill(owner: string, name: string): Promise<{ content: string; files: string[] }> {
  const o = await ownerRoot(owner)
  const dir = safeJoin(o.root, name)
  const files = (await readdir(dir)).filter((f) => !f.startsWith('.'))
  const content = await readFile(join(dir, 'SKILL.md'), 'utf8').catch(() => '')
  return { content, files }
}

export async function writeSkill(owner: string, name: string, content: string, author?: string | null): Promise<void> {
  const o = await ownerRoot(owner)
  const dir = safeJoin(o.root, name)
  await mkdir(dir, { recursive: true })
  await writeFile(join(dir, 'SKILL.md'), content)
  await snapshot('skill', `${owner}/${name}`, content, author ?? null).catch(() => {})
}

export async function deleteSkill(owner: string, name: string): Promise<void> {
  const o = await ownerRoot(owner)
  const dir = safeJoin(o.root, name)
  // Only remove things that look like a skill dir — refuse anything else.
  const st = await stat(dir)
  if (!st.isDirectory()) throw new Error('not a skill')
  await rm(dir, { recursive: true })
  await dropSummary(owner, name).catch(() => {})
}

/** Rename a skill in place (same owner). Refuses to clobber. */
export async function renameSkill(owner: string, name: string, toName: string): Promise<void> {
  const o = await ownerRoot(owner)
  const from = safeJoin(o.root, name)
  const to = safeJoin(o.root, toName)
  if (await stat(to).catch(() => null)) throw new Error(`"${toName}" already exists`)
  await rename(from, to)
  await moveSummary(owner, name, owner, toName).catch(() => {})
}

/** Copy a skill (whole dir, support files included) to another owner —
 *  optionally removing the source (= move, e.g. promote dept → shared). */
export async function copySkill(
  owner: string,
  name: string,
  toOwner: string,
  opts: { toName?: string; removeSource?: boolean } = {},
): Promise<void> {
  const src = await ownerRoot(owner)
  const dst = await ownerRoot(toOwner)
  const toName = opts.toName ?? name
  const from = safeJoin(src.root, name)
  const to = safeJoin(dst.root, toName)
  if (await stat(to).catch(() => null)) throw new Error(`"${toName}" already exists there`)
  await mkdir(dst.root, { recursive: true })
  await cp(from, to, { recursive: true })
  if (opts.removeSource) {
    await rm(from, { recursive: true })
    await moveSummary(owner, name, toOwner, toName).catch(() => {})
  }
}
