// Skills client — one SKILL.md library per owner, plus the shared one.
//
// There were THREE clients for this endpoint, inline in three editors, and
// they had drifted in every way a copy can:
//
//   - three query keys for the same row (`['skill', …]`, `['assistant-skill',
//     …]`), so a save in one surface left the others showing stale content;
//   - three invalidation sets, one of which knew about `['skill-library']` and
//     the others did not;
//   - and two of the three sent `credentials: 'same-origin'` while the third
//     did not, which is the kind of difference nobody notices until a
//     deployment changes what the default is.
//
// Keys and mutations live here now so those cannot disagree again.
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getJson, getList, postJson, putJson } from '@/lib/fetch-json'

export interface SkillSummary {
  name: string
  description: string
  /** Present on the fleet/Studio reads; absent on narrower ones. */
  files?: string[]
  /** Talaria-maintained rather than org-authored. */
  platform?: boolean
}

export interface SkillOwner {
  owner: string // 'shared' or an agent slug
  label: string
  /** The agent's model id (absent for the shared root). */
  model?: string
  skills: SkillSummary[]
  canEdit?: boolean
}

export interface SkillDoc {
  content: string
  files: string[]
}

/** Every owner's library. One key, because it is one read. */
export const SKILLS_KEY = ['skills'] as const
/** One skill's document. */
export const skillKey = (owner: string, name: string) => ['skill', owner, name] as const

export function useSkills() {
  return createQuery(() => ({
    queryKey: SKILLS_KEY,
    queryFn: (): Promise<SkillOwner[]> => getList<SkillOwner>('/api/skills', 'owners'),
  }))
}

export function useSkill(owner: () => string, name: () => string | null) {
  return createQuery(() => {
    const n = name()
    return {
      queryKey: skillKey(owner(), n ?? ''),
      enabled: !!n,
      // A 404 is NOT forgiven into a null: this is only reachable from a row in
      // a library that just listed it, so "no such skill" is as much a failure
      // as a 500 — and the route sends its reason as `{ error }`, which
      // getJson lifts into the message the panel shows.
      queryFn: (): Promise<SkillDoc> => getJson<SkillDoc>(`/api/skills/${owner()}/${n}`),
    }
  })
}

/** An owner's skills out of the full read. An owner with no entry is a real
 *  empty ("none yet"); a FAILED read is not, and must not be flattened into
 *  the same `?? []` — callers pass `listQuery`'s rows, which already made that
 *  distinction upstream. */
export const skillsOf = (owners: SkillOwner[], owner: string): SkillSummary[] =>
  owners.find((o) => o.owner === owner)?.skills ?? []

export const canEditSkillsOf = (owners: SkillOwner[], owner: string): boolean =>
  owners.find((o) => o.owner === owner)?.canEdit ?? false

export async function saveSkill(owner: string, name: string, content: string): Promise<void> {
  await putJson<{ ok: true }>(`/api/skills/${owner}/${name}`, { content })
}

export async function deleteSkill(owner: string, name: string): Promise<void> {
  await delJson<{ ok: true }>(`/api/skills/${owner}/${name}`)
}

// ── The marketplace (Hermes' official skills index, scanned before install) ──

export type TrustLevel = 'builtin' | 'trusted' | 'community'

export interface MarketplaceEntry {
  /** Hermes' catalog identifier — the install's only input. */
  id: string
  name: string
  description: string
  /** official | github | skills.sh */
  source: string
  /** As the index states it. */
  trustLevel: TrustLevel | string
  /** GitHub owner/name the skill's files live in. */
  repo: string
  /** The skill directory inside `repo` ("" = the repo root). */
  path: string
  tags: string[]
}

/** One Skills Guard finding, as Hermes reports it. */
export interface ScanFinding {
  patternId: string
  severity: 'critical' | 'high' | 'medium' | 'low' | string
  category: string
  file: string
  line: number
  match: string
  description: string
}

/** Hermes' Skills Guard verdict on one fetched skill, and its install
 *  decision: only `allowed === true` installs (null is Hermes' "ask"). */
export interface ScanReport {
  scanner: string
  verdict: 'safe' | 'caution' | 'dangerous' | string
  trustLevel: TrustLevel | string
  allowed: boolean | null
  reason: string
  summary: string
  findings: ScanFinding[]
  image: string
}

/** The preview: the skill as it would install, with its scan. */
export interface MarketplaceSkill {
  id: string
  name: string
  repo: string
  commit: string
  dir: string
  /** owner/repo/path when an official stub was followed upstream. */
  upstream: string | null
  summary: string
  fileCount: number
  files: string[]
  scan: ScanReport
}

export interface InstallOutcome {
  ok: true
  owner: string
  id: string
  name: string
  status: 'installed' | 'exists'
  scan: ScanReport
}

/** The catalog read, keyed by the live query text. */
export const marketplaceKey = (q: string) => ['skills-marketplace', q] as const

/** Fetch and scan one catalog skill for `owner` (the destination's edit
 *  gate applies — only someone who could install it may preview it). The
 *  api caches both halves, so the install that follows re-reads them. */
export function marketplaceDetail(owner: string, id: string): Promise<MarketplaceSkill> {
  return getJson(`/api/skills/marketplace/detail?owner=${encodeURIComponent(owner)}&id=${encodeURIComponent(id)}`)
}

/** Install into an owner's root. The api scans first and refuses (422) what
 *  Hermes' install policy blocks; `exists` = left untouched. */
export function installMarketplaceSkill(owner: string, id: string): Promise<InstallOutcome> {
  return postJson('/api/skills/marketplace/install', { owner, id })
}

/** Rename the skill's directory without touching content. PUTs write to a
 *  single path, so a record that saves under a new name is two writes: rename
 *  the directory first, then write the content under the new name. */
export async function renameSkill(owner: string, name: string, toName: string): Promise<void> {
  await postJson<{ ok: true }>(`/api/skills/${owner}/${name}`, { op: 'rename', toName })
}

/** What a brand-new skill starts as.
 *
 *  One template, because the three call sites had three — one of which wrote a
 *  bare `description:` line into the body, where the format wants prose under a
 *  heading. A skill created in one surface should not look different from one
 *  created in another. */
export const SKILL_TEMPLATE = (name: string) => `# ${name}

## When to use
Describe the job this skill is for, in the words someone would use to ask for it.

## Steps
1.
`

/** A skill name is a directory name: lowercase, no spaces.
 *
 *  Was done two ways — sanitising on every keystroke in one surface, and only
 *  on submit in the other, so the same typing produced different names. */
export const skillName = (raw: string) =>
  raw
    .trim()
    .toLowerCase()
    .replace(/\s+/g, '-')
    .replace(/[^a-z0-9._-]/g, '-')
    .replace(/^-+|-+$/g, '')
