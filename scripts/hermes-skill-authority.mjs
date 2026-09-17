// Classify Hermes bundled skill paths against scripts/hermes-skill-authority.json.
// Shared by `bun run check` (invariants) and the chassis boot smoke (live image).
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

export function loadAuthority(root) {
  return JSON.parse(readFileSync(join(root, 'scripts/hermes-skill-authority.json'), 'utf8'))
}

export function isClassified(entry, cat) {
  const replaced = cat.replaced || []
  if (replaced.some((r) => entry === r.path || entry.startsWith(r.path + '/'))) return true
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
