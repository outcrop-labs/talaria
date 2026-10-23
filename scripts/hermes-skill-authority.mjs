// Classify Hermes bundled skill paths against scripts/hermes-skill-authority.json.
// Shared by `bun run check` (invariants) and the chassis boot smoke (live image).
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

export function loadAuthority(root) {
  return JSON.parse(readFileSync(join(root, 'scripts/hermes-skill-authority.json'), 'utf8'))
}

/** The replaced entry whose path holds this pack path, or undefined. */
export function replacedEntry(entry, cat) {
  return (cat.replaced || []).find((r) => entry === r.path || entry.startsWith(r.path + '/'))
}

export function isClassified(entry, cat) {
  if (replacedEntry(entry, cat)) return true
  if ((cat.keepExact || []).includes(entry)) return true
  if ((cat.keepPrefixes || []).some((p) => entry.startsWith(p))) return true
  return false
}

export function unclassified(entries, cat) {
  return entries.filter((e) => !isClassified(e, cat))
}

/** SKILL.md path relative to the skills root → catalog pack path. */
export function packFromSkillFile(rel) {
  return rel.replace(/\/SKILL\.md$/i, '').replace(/\\/g, '/')
}

/** The frontmatter `name:` of a SKILL.md body, or '' when it has none. Only the
 *  frontmatter counts: a body that mentions a name does not occupy it. */
export function frontmatterName(text) {
  const front = String(text).match(/^---\r?\n([\s\S]*?)\r?\n---/)
  for (const line of front ? front[1].split(/\r?\n/) : []) {
    const m = line.match(/^name:\s*(.+?)\s*$/)
    if (m) return m[1].replace(/^(["'])(.*)\1$/, '$2')
  }
  return ''
}

/** Every name a scripts/skills/<dir>/SKILL.md declares in its frontmatter. */
export function declaredSignpostNames(root) {
  const declared = new Set()
  const skillsRoot = join(root, 'scripts/skills')
  for (const dir of existsSync(skillsRoot) ? readdirSync(skillsRoot) : []) {
    const file = join(skillsRoot, dir, 'SKILL.md')
    if (!existsSync(file)) continue
    const name = frontmatterName(readFileSync(file, 'utf8'))
    if (name) declared.add(name)
  }
  return declared
}

/** Replaced paths that are not a catalogued skill directory. A category path
 *  (`email`) classifies every sibling Hermes adds under it as replaced, while
 *  no signpost holds the newcomer's name, so Hermes syncs it onto every agent. */
export function replacedPathsNotCatalogued(cat) {
  const catalog = new Set(cat.catalog || [])
  return (cat.replaced || []).map((r) => r.path).filter((p) => !catalog.has(p))
}

/** Live skills under a replaced path that would still reach an agent. Hermes
 *  names a bundled skill by its frontmatter `name:` (the directory name when
 *  there is none) and defers it only to an external skill of that name, so the
 *  name has to be in the entry's names[] and declared by a signpost.
 *  `skills` is [{ pack, name }]. */
export function uncoveredReplacedSkills(skills, cat, declared) {
  const out = []
  for (const { pack, name } of skills) {
    const r = replacedEntry(pack, cat)
    if (!r) continue
    const n = name || pack.split('/').pop()
    if (!(r.names || []).includes(n)) {
      out.push(`${pack} ships a skill named "${n}" that ${r.path}'s names[] does not list`)
    } else if (!declared.has(n)) {
      out.push(`${pack} ships "${n}" and no scripts/skills/*/SKILL.md declares name: ${n}`)
    }
  }
  return out
}
