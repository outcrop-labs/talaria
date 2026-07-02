// Agent skills as files — the way Hermes actually consumes them. Every agent
// mounts two skill roots read-only:
//   /opt/skills       ← <stack>/skills            (shared across the fleet)
//   /opt/dept-skills  ← <stack>/agents/<department>/skills   (imported agents)
//                     ← <fleet>/agents/<slug>/skills          (created agents)
// Hermes reads skills per invocation, so edits here are live — no restart.
// Each skill is a directory holding a SKILL.md (plus optional support files).
import { mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { db } from './db/pg'
import { FLEET_DIR, STACK_DIR } from './fleet-render'

/** Owner key: 'shared' or an agent slug. */
export const SHARED = 'shared'
const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/

interface OwnerInfo {
  owner: string
  label: string
  root: string
  /** Imported agents' dept skills live in the legacy stack dir (their live mount). */
  source: 'shared' | 'imported' | 'created'
}

async function owners(): Promise<OwnerInfo[]> {
  const sql = await db()
  const defs = (await sql`
    select slug, department, display_name as "displayName", source
    from agent_defs where enabled order by slug
  `) as unknown as Array<{ slug: string; department: string; displayName: string; source: string }>
  return [
    { owner: SHARED, label: 'Shared (all agents)', root: join(STACK_DIR(), 'skills'), source: 'shared' as const },
    ...defs.map((d) => ({
      owner: d.slug,
      label: d.displayName,
      source: d.source as 'imported' | 'created',
      root:
        d.source === 'created'
          ? join(FLEET_DIR(), 'agents', d.slug, 'skills')
          : join(STACK_DIR(), 'agents', d.department, 'skills'),
    })),
  ]
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
  /** First prose line of SKILL.md. */
  description: string
  files: string[]
}

export interface OwnerSkills {
  owner: string
  label: string
  source: 'shared' | 'imported' | 'created'
  skills: SkillSummary[]
}

function summarize(md: string): string {
  for (const line of md.split('\n')) {
    const t = line.trim()
    if (!t || t.startsWith('#') || t.startsWith('---')) continue
    // Frontmatter-style "description: …" wins if present.
    const fm = /^description:\s*(.+)$/.exec(t)
    return (fm ? fm[1]! : t).slice(0, 160)
  }
  return ''
}

export async function listAllSkills(): Promise<OwnerSkills[]> {
  const out: OwnerSkills[] = []
  for (const o of await owners()) {
    const entries = await readdir(o.root, { withFileTypes: true }).catch(() => [])
    const skills: SkillSummary[] = []
    for (const e of entries) {
      if (!e.isDirectory() || !NAME_RE.test(e.name)) continue
      const dir = join(o.root, e.name)
      const files = (await readdir(dir).catch(() => [])).filter((f) => !f.startsWith('.'))
      const md = await readFile(join(dir, 'SKILL.md'), 'utf8').catch(() => '')
      skills.push({ name: e.name, description: summarize(md), files })
    }
    out.push({ owner: o.owner, label: o.label, source: o.source, skills: skills.sort((a, b) => a.name.localeCompare(b.name)) })
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

export async function writeSkill(owner: string, name: string, content: string): Promise<void> {
  const o = await ownerRoot(owner)
  const dir = safeJoin(o.root, name)
  await mkdir(dir, { recursive: true })
  await writeFile(join(dir, 'SKILL.md'), content)
}

export async function deleteSkill(owner: string, name: string): Promise<void> {
  const o = await ownerRoot(owner)
  const dir = safeJoin(o.root, name)
  // Only remove things that look like a skill dir — refuse anything else.
  const st = await stat(dir)
  if (!st.isDirectory()) throw new Error('not a skill')
  await rm(dir, { recursive: true })
}
