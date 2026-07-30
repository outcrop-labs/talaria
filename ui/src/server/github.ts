// The Workbench's GitHub connection — org-level, either flavor:
//   app  a GitHub App (preferred): short-lived installation tokens, per-repo
//        install scope, instant revocation from GitHub's side
//   pat  a personal access token (quickest start; fine-grained PAT advised)
// Config lives in app_settings with secrets SEALED (secretbox), same contract
// as email. Repo access per agent is a separate explicit grant table —
// connecting GitHub grants nothing to anyone by itself.
import { createSign } from 'node:crypto'
import { getSetting, setSetting } from './audit'
import { seal, open } from './secretbox'
import { db } from './db/pg'

export interface GithubConfig {
  mode: 'app' | 'pat' | null
  pat: { tokenEnc: string | null }
  app: { appId: string; installationId: string; privateKeyEnc: string | null }
}

const KEY = 'github_config'
const DEFAULTS: GithubConfig = {
  mode: null,
  pat: { tokenEnc: null },
  app: { appId: '', installationId: '', privateKeyEnc: null },
}

export const getGithubConfig = async (): Promise<GithubConfig> => ({
  ...DEFAULTS,
  ...(await getSetting<Partial<GithubConfig>>(KEY, {})),
})

export async function setGithubConfig(patch: {
  mode?: 'app' | 'pat' | null
  pat?: { token?: string | null }
  app?: { appId?: string; installationId?: string; privateKey?: string | null }
}): Promise<void> {
  const cur = await getGithubConfig()
  const next: GithubConfig = {
    mode: patch.mode !== undefined ? patch.mode : cur.mode,
    pat: { tokenEnc: patch.pat?.token !== undefined ? (patch.pat.token ? seal(patch.pat.token) : null) : cur.pat.tokenEnc },
    app: {
      appId: patch.app?.appId ?? cur.app.appId,
      installationId: patch.app?.installationId ?? cur.app.installationId,
      privateKeyEnc:
        patch.app?.privateKey !== undefined ? (patch.app.privateKey ? seal(patch.app.privateKey) : null) : cur.app.privateKeyEnc,
    },
  }
  await setSetting(KEY, next)
  // Credentials changed — a cached installation token must never outlive them.
  cachedInstallToken = null
}

const GH = 'https://api.github.com'
const HEADERS = { accept: 'application/vnd.github+json', 'x-github-api-version': '2022-11-28', 'user-agent': 'talaria-workbench' }

async function gh(path: string, token: string, init: RequestInit = {}): Promise<Response> {
  return fetch(`${GH}${path}`, { ...init, headers: { ...HEADERS, authorization: `Bearer ${token}`, ...(init.headers ?? {}) } })
}

/** RS256 app JWT — GitHub Apps authenticate to /app endpoints with this. */
function appJwt(appId: string, privateKeyPem: string): string {
  const now = Math.floor(Date.now() / 1000)
  const b64 = (o: unknown) => Buffer.from(JSON.stringify(o)).toString('base64url')
  const unsigned = `${b64({ alg: 'RS256', typ: 'JWT' })}.${b64({ iat: now - 60, exp: now + 9 * 60, iss: appId })}`
  const sig = createSign('RSA-SHA256').update(unsigned).sign(privateKeyPem).toString('base64url')
  return `${unsigned}.${sig}`
}

let cachedInstallToken: { token: string; expiresAt: number } | null = null

/** A usable token for repo operations, whatever the mode. App tokens are
 *  minted per ~hour and cached; PATs pass through. Null = not configured. */
export async function githubToken(): Promise<string | null> {
  const cfg = await getGithubConfig()
  if (cfg.mode === 'pat') return cfg.pat.tokenEnc ? open(cfg.pat.tokenEnc) : null
  if (cfg.mode !== 'app' || !cfg.app.appId || !cfg.app.installationId || !cfg.app.privateKeyEnc) return null
  if (cachedInstallToken && cachedInstallToken.expiresAt > Date.now() + 60_000) return cachedInstallToken.token
  const jwt = appJwt(cfg.app.appId, open(cfg.app.privateKeyEnc))
  const res = await gh(`/app/installations/${cfg.app.installationId}/access_tokens`, jwt, { method: 'POST' })
  if (!res.ok) throw new Error(`GitHub installation token failed (${res.status})`)
  const j = (await res.json()) as { token: string; expires_at: string }
  cachedInstallToken = { token: j.token, expiresAt: Date.parse(j.expires_at) }
  return j.token
}

export interface GithubStatus {
  mode: 'app' | 'pat' | null
  configured: boolean
  /** Who the connection acts as — PAT login or App name — when verifiable. */
  account: string | null
  error: string | null
  app: { appId: string; installationId: string; keySet: boolean }
  patSet: boolean
}

/** Redacted status for the admin panel — verifies live, never leaks secrets. */
export async function githubStatus(): Promise<GithubStatus> {
  const cfg = await getGithubConfig()
  const base = {
    mode: cfg.mode,
    app: { appId: cfg.app.appId, installationId: cfg.app.installationId, keySet: !!cfg.app.privateKeyEnc },
    patSet: !!cfg.pat.tokenEnc,
  }
  try {
    if (cfg.mode === 'pat' && cfg.pat.tokenEnc) {
      const res = await gh('/user', open(cfg.pat.tokenEnc))
      if (!res.ok) return { ...base, configured: true, account: null, error: `token rejected (${res.status})` }
      const j = (await res.json()) as { login: string }
      return { ...base, configured: true, account: j.login, error: null }
    }
    if (cfg.mode === 'app' && cfg.app.appId && cfg.app.privateKeyEnc) {
      const jwt = appJwt(cfg.app.appId, open(cfg.app.privateKeyEnc))
      const res = await gh('/app', jwt)
      if (!res.ok) return { ...base, configured: true, account: null, error: `app credentials rejected (${res.status})` }
      const j = (await res.json()) as { name: string }
      if (!cfg.app.installationId) return { ...base, configured: false, account: j.name, error: 'pick an installation' }
      return { ...base, configured: true, account: j.name, error: null }
    }
  } catch (e) {
    return { ...base, configured: false, account: null, error: (e as Error).message.slice(0, 200) }
  }
  return { ...base, configured: false, account: null, error: null }
}

/** App mode's easy-setup helper: once appId+key verify, list where the app is
 *  installed so the admin picks the installation instead of hunting an id. */
export async function listInstallations(): Promise<Array<{ id: number; account: string }>> {
  const cfg = await getGithubConfig()
  if (!cfg.app.appId || !cfg.app.privateKeyEnc) return []
  const jwt = appJwt(cfg.app.appId, open(cfg.app.privateKeyEnc))
  const res = await gh('/app/installations', jwt)
  if (!res.ok) return []
  const j = (await res.json()) as Array<{ id: number; account: { login: string } }>
  return j.map((i) => ({ id: i.id, account: i.account.login }))
}

/** Repos the connection can reach — the pool agent grants pick from.
 *  Paginated (Link: rel="next"), capped at 5 pages / 500 repos. */
export async function listReachableRepos(): Promise<string[]> {
  const cfg = await getGithubConfig()
  const token = await githubToken()
  if (!token) return []
  const out: string[] = []
  let path = cfg.mode === 'app' ? '/installation/repositories?per_page=100' : '/user/repos?per_page=100&sort=pushed'
  for (let page = 0; page < 5 && path; page++) {
    const res = await gh(path, token)
    if (!res.ok) break
    const j = (await res.json()) as { repositories?: Array<{ full_name: string }> } | Array<{ full_name: string }>
    const repos = Array.isArray(j) ? j : (j.repositories ?? [])
    out.push(...repos.map((r) => r.full_name))
    const next = /<https:\/\/api\.github\.com([^>]+)>;\s*rel="next"/.exec(res.headers.get('link') ?? '')
    path = next?.[1] ?? ''
  }
  return out
}

// ── Per-repo git flow (PR target + optional testing branch) ──────────────────

export interface RepoFlow {
  repo: string
  /** PR/base branch; null = the repo's default branch. */
  baseBranch: string | null
  /** Integration branch features merge into for testing; null = disabled. */
  testingBranch: string | null
}

export async function repoFlow(repo: string): Promise<RepoFlow> {
  const sql = await db()
  const rows = (await sql`
    select repo, base_branch as "baseBranch", testing_branch as "testingBranch"
    from workbench_repo_flow where repo = ${repo}
  `) as unknown as RepoFlow[]
  return rows[0] ?? { repo, baseBranch: null, testingBranch: null }
}

export async function listRepoFlows(): Promise<RepoFlow[]> {
  const sql = await db()
  return (await sql`
    select repo, base_branch as "baseBranch", testing_branch as "testingBranch"
    from workbench_repo_flow order by repo
  `) as unknown as RepoFlow[]
}

export async function setRepoFlow(repo: string, patch: { baseBranch?: string | null; testingBranch?: string | null }): Promise<void> {
  const sql = await db()
  const cur = await repoFlow(repo)
  await sql`
    insert into workbench_repo_flow (repo, base_branch, testing_branch)
    values (${repo}, ${patch.baseBranch !== undefined ? patch.baseBranch : cur.baseBranch}, ${patch.testingBranch !== undefined ? patch.testingBranch : cur.testingBranch})
    on conflict (repo) do update set
      base_branch = excluded.base_branch, testing_branch = excluded.testing_branch, updated_at = now()
  `
}

/** The branch jobs cut from and PRs target — the flow override, else default. */
export async function effectiveBase(repo: string): Promise<string> {
  const flow = await repoFlow(repo)
  return flow.baseBranch ?? (await defaultBranch(repo))
}

/** Merge head into base via GitHub's merge API (e.g. feature → testing).
 *  Ensures the target exists (created from the effective base if missing). */
export async function mergeInto(repo: string, targetBranch: string, head: string): Promise<{ merged: boolean; reason?: string }> {
  const token = await githubToken()
  if (!token) throw new Error('GitHub is not connected')
  const existing = await gh(`/repos/${repo}/git/ref/heads/${encodeURIComponent(targetBranch)}`, token)
  if (!existing.ok) {
    const base = await effectiveBase(repo)
    const headRef = await ghJson<{ object: { sha: string } }>(`/repos/${repo}/git/ref/heads/${encodeURIComponent(base)}`, token)
    await ghJson(`/repos/${repo}/git/refs`, token, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ ref: `refs/heads/${targetBranch}`, sha: headRef.object.sha }),
    })
  }
  const res = await gh(`/repos/${repo}/merges`, token, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ base: targetBranch, head, commit_message: `Merge ${head} into ${targetBranch} (Talaria workbench, for testing)` }),
  })
  if (res.status === 201) return { merged: true }
  if (res.status === 204) return { merged: true, reason: 'already up to date' }
  if (res.status === 409) return { merged: false, reason: 'merge conflict — resolve on the branch first' }
  return { merged: false, reason: `GitHub merge failed (${res.status})` }
}

// ── Per-agent repo grants — explicit, like MCP assignment ────────────────────

export async function grantedRepos(agentId: string): Promise<string[]> {
  const sql = await db()
  const rows = (await sql`select repo from workbench_repos where agent_id = ${agentId} order by repo`) as unknown as Array<{ repo: string }>
  return rows.map((r) => r.repo)
}

export async function setGrantedRepos(agentId: string, repos: string[]): Promise<void> {
  const sql = await db()
  const keep = repos.slice(0, 100)
  // Atomic replace — a crash can never leave the agent grantless by accident.
  await sql.begin(async (tx) => {
    await tx`delete from workbench_repos where agent_id = ${agentId}`
    if (keep.length) {
      await tx`insert into workbench_repos (agent_id, repo) select ${agentId}, unnest(${keep}::text[]) on conflict do nothing`
    }
  })
}

// ── Repo operations (the platform-owned git flow) ────────────────────────────

async function ghJson<T>(path: string, token: string, init: RequestInit = {}): Promise<T> {
  const res = await gh(path, token, init)
  if (!res.ok) throw new Error(`GitHub ${init.method ?? 'GET'} ${path} → ${res.status}: ${(await res.text()).slice(0, 200)}`)
  return (await res.json()) as T
}

export async function defaultBranch(repo: string): Promise<string> {
  const token = await githubToken()
  if (!token) throw new Error('GitHub is not connected')
  return (await ghJson<{ default_branch: string }>(`/repos/${repo}`, token)).default_branch
}

/** Cut a branch from the default branch's head. Idempotent-ish: an existing
 *  branch of the same name is left alone (the job resumes on it). */
export async function createBranch(repo: string, branch: string, baseOverride?: string): Promise<{ base: string; created: boolean }> {
  const token = await githubToken()
  if (!token) throw new Error('GitHub is not connected')
  const base = baseOverride ?? (await effectiveBase(repo))
  const existing = await gh(`/repos/${repo}/git/ref/heads/${encodeURIComponent(branch)}`, token)
  if (existing.ok) return { base, created: false }
  const head = await ghJson<{ object: { sha: string } }>(`/repos/${repo}/git/ref/heads/${encodeURIComponent(base)}`, token)
  await ghJson(`/repos/${repo}/git/refs`, token, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ ref: `refs/heads/${branch}`, sha: head.object.sha }),
  })
  return { base, created: true }
}

export async function branchAhead(repo: string, base: string, branch: string): Promise<number> {
  const token = await githubToken()
  if (!token) throw new Error('GitHub is not connected')
  const cmp = await ghJson<{ ahead_by: number }>(`/repos/${repo}/compare/${encodeURIComponent(base)}...${encodeURIComponent(branch)}`, token)
  return cmp.ahead_by
}

export async function createPullRequest(
  repo: string,
  input: { head: string; base: string; title: string; body: string; draft?: boolean },
): Promise<{ url: string; number: number }> {
  const token = await githubToken()
  if (!token) throw new Error('GitHub is not connected')
  const pr = await ghJson<{ html_url: string; number: number }>(`/repos/${repo}/pulls`, token, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ ...input, draft: input.draft ?? false }),
  })
  return { url: pr.html_url, number: pr.number }
}

/** An authenticated clone URL for the sandbox harness — app tokens expire in
 *  ~an hour by design; PATs are the org's own choice of blast radius. */
export async function cloneUrl(repo: string): Promise<string | null> {
  const token = await githubToken()
  return token ? `https://x-access-token:${token}@github.com/${repo}.git` : null
}
