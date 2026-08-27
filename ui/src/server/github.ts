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
  app: {
    appId: string
    /** Selected installations — the App can serve several orgs at once. */
    installationIds: string[]
    privateKeyEnc: string | null
  }
  /** Orgs where agents may REQUEST new repos (human approval creates them).
   *  Empty = the feature is off. Requires the App's org Administration
   *  permission to actually create. */
  repoCreationOrgs: string[]
}

const KEY = 'github_config'
const DEFAULTS: GithubConfig = {
  mode: null,
  pat: { tokenEnc: null },
  app: { appId: '', installationIds: [], privateKeyEnc: null },
  repoCreationOrgs: [],
}

export const getGithubConfig = async (): Promise<GithubConfig> => {
  const raw = await getSetting<Partial<GithubConfig> & { app?: { installationId?: string } }>(KEY, {})
  const cfg: GithubConfig = { ...DEFAULTS, ...raw, app: { ...DEFAULTS.app, ...(raw.app ?? {}) } }
  // Legacy single-installation config migrates transparently on read.
  const legacy = (raw.app as { installationId?: string } | undefined)?.installationId
  if (legacy && !cfg.app.installationIds.length) cfg.app.installationIds = [legacy]
  return cfg
}

export async function setGithubConfig(patch: {
  mode?: 'app' | 'pat' | null
  pat?: { token?: string | null }
  app?: { appId?: string; installationIds?: string[]; privateKey?: string | null }
  repoCreationOrgs?: string[]
}): Promise<void> {
  const cur = await getGithubConfig()
  const next: GithubConfig = {
    mode: patch.mode !== undefined ? patch.mode : cur.mode,
    pat: { tokenEnc: patch.pat?.token !== undefined ? (patch.pat.token ? seal(patch.pat.token) : null) : cur.pat.tokenEnc },
    app: {
      appId: patch.app?.appId ?? cur.app.appId,
      installationIds: patch.app?.installationIds ?? cur.app.installationIds,
      privateKeyEnc:
        patch.app?.privateKey !== undefined ? (patch.app.privateKey ? seal(patch.app.privateKey) : null) : cur.app.privateKeyEnc,
    },
    repoCreationOrgs: patch.repoCreationOrgs ?? cur.repoCreationOrgs,
  }
  await setSetting(KEY, next)
  // Credentials changed — cached tokens/routing must never outlive them.
  cachedInstallTokens = new Map()
  repoInstallCache = null
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

let cachedInstallTokens = new Map<string, { token: string; expiresAt: number }>()
/** repo full_name → installation id, learned from listing; refreshed lazily. */
let repoInstallCache: { map: Map<string, string>; at: number } | null = null

/** Mint an installation access token.
 *
 *  When `repo` is given the token is scoped to THAT repository alone. Without a
 *  body GitHub issues a token valid for EVERY repository in the installation —
 *  and this token ends up inside a sandbox running model-authored code (see
 *  cloneUrl), so an unscoped one made a single leaked clone URL equivalent to
 *  push access across the whole org. `repositories` takes bare names, not
 *  owner/name.
 *
 *  Permissions are deliberately NOT narrowed here: the token inherits whatever
 *  the installation was granted. Narrowing further is a real improvement, but
 *  asking for a permission the installation lacks is a hard 422, so it needs to
 *  be driven off the installation's actual grants rather than assumed. */
async function installationToken(cfg: GithubConfig, installationId: string, repo?: string): Promise<string> {
  const cacheKey = repo ? `${installationId}#${repo}` : installationId
  const hit = cachedInstallTokens.get(cacheKey)
  if (hit && hit.expiresAt > Date.now() + 60_000) return hit.token
  const jwt = appJwt(cfg.app.appId, open(cfg.app.privateKeyEnc!))
  const name = repo?.split('/').pop()
  const res = await gh(`/app/installations/${installationId}/access_tokens`, jwt, {
    method: 'POST',
    ...(name ? { body: JSON.stringify({ repositories: [name] }), headers: { 'content-type': 'application/json' } } : {}),
  })
  if (!res.ok) {
    // 422 here usually means the repo isn't in THIS installation — worth
    // saying so, because the caller picked the installation by cache lookup.
    throw new Error(`GitHub installation token failed (${res.status})${repo ? ` for ${repo}` : ''}`)
  }
  const j = (await res.json()) as { token: string; expires_at: string }
  cachedInstallTokens.set(cacheKey, { token: j.token, expiresAt: Date.parse(j.expires_at) })
  return j.token
}

/** A usable token for repo operations. PAT passes through; App mode mints a
 *  per-installation token — pass `repo` so multi-org setups route to the
 *  installation that owns it (defaults to the first installation) AND so the
 *  token is scoped to that one repository.
 *
 *  Callers that genuinely need installation-wide reach (listing repos, probing
 *  the connection) omit `repo` and get the broad token, as before. PAT mode
 *  cannot be scoped at all — that blast radius is the org's own choice, and
 *  the setup guide says so. */
export async function githubToken(repo?: string): Promise<string | null> {
  const cfg = await getGithubConfig()
  if (cfg.mode === 'pat') return cfg.pat.tokenEnc ? open(cfg.pat.tokenEnc) : null
  if (cfg.mode !== 'app' || !cfg.app.appId || !cfg.app.installationIds.length || !cfg.app.privateKeyEnc) return null
  let installationId = cfg.app.installationIds[0]!
  if (repo && cfg.app.installationIds.length > 1) {
    if (!repoInstallCache || Date.now() - repoInstallCache.at > 10 * 60_000) await listReachableRepos()
    installationId = repoInstallCache?.map.get(repo) ?? installationId
  }
  return installationToken(cfg, installationId, repo)
}

export interface GithubStatus {
  mode: 'app' | 'pat' | null
  configured: boolean
  /** Who the connection acts as — PAT login or App name — when verifiable. */
  account: string | null
  error: string | null
  app: { appId: string; installationIds: string[]; keySet: boolean }
  patSet: boolean
  repoCreationOrgs: string[]
}

/** Redacted status for the admin panel — verifies live, never leaks secrets. */
export async function githubStatus(): Promise<GithubStatus> {
  const cfg = await getGithubConfig()
  const base = {
    mode: cfg.mode,
    app: { appId: cfg.app.appId, installationIds: cfg.app.installationIds, keySet: !!cfg.app.privateKeyEnc },
    patSet: !!cfg.pat.tokenEnc,
    repoCreationOrgs: cfg.repoCreationOrgs,
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
      if (!cfg.app.installationIds.length) return { ...base, configured: false, account: j.name, error: 'pick at least one installation' }
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

async function pagedRepos(token: string, firstPath: string): Promise<string[]> {
  const out: string[] = []
  let path = firstPath
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

/** Repos the connection can reach — the pool agent grants pick from. App
 *  mode is the UNION across all selected installations (multi-org), and the
 *  listing doubles as the repo→installation routing table for token minting.
 *  Paginated per source, capped at 5 pages each. */
export async function listReachableRepos(): Promise<string[]> {
  const cfg = await getGithubConfig()
  if (cfg.mode === 'pat') {
    const token = await githubToken()
    return token ? pagedRepos(token, '/user/repos?per_page=100&sort=pushed') : []
  }
  if (cfg.mode !== 'app' || !cfg.app.installationIds.length) return []
  const map = new Map<string, string>()
  const out: string[] = []
  for (const inst of cfg.app.installationIds) {
    const token = await installationToken(cfg, inst).catch(() => null)
    if (!token) continue
    for (const repo of await pagedRepos(token, '/installation/repositories?per_page=100')) {
      if (!map.has(repo)) {
        map.set(repo, inst)
        out.push(repo)
      }
    }
  }
  repoInstallCache = { map, at: Date.now() }
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
  const token = await githubToken(repo)
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
  if (res.status === 409) return { merged: false, reason: 'merge conflict. Resolve on the branch first' }
  return { merged: false, reason: `GitHub merge failed (${res.status})` }
}

/** Create a repo in an org (App must hold org Administration permission).
 *  Token routes to the installation on that org when multi-install. */
export async function createRepo(org: string, name: string, description: string): Promise<{ fullName: string; url: string }> {
  const cfg = await getGithubConfig()
  if (cfg.mode !== 'app') throw new Error('repo creation requires the GitHub App connection')
  // Route by any repo we know in that org, else first installation.
  const known = repoInstallCache?.map ?? new Map<string, string>()
  const inOrg = [...known.entries()].find(([r]) => r.startsWith(`${org}/`))
  const token = inOrg ? await installationToken(cfg, inOrg[1]) : await githubToken()
  if (!token) throw new Error('GitHub is not connected')
  const res = await gh(`/orgs/${org}/repos`, token, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ name, description: description.slice(0, 300), private: true, auto_init: true }),
  })
  if (res.status === 403) throw new Error("the App lacks the org's Administration permission. Grant it in the App settings, then re-approve.")
  if (!res.ok) throw new Error(`GitHub repo creation failed (${res.status}): ${(await res.text()).slice(0, 200)}`)
  const j = (await res.json()) as { full_name: string; html_url: string }
  repoInstallCache = null // pool changed
  return { fullName: j.full_name, url: j.html_url }
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

async function defaultBranch(repo: string): Promise<string> {
  const token = await githubToken(repo)
  if (!token) throw new Error('GitHub is not connected')
  return (await ghJson<{ default_branch: string }>(`/repos/${repo}`, token)).default_branch
}

/** Cut a branch from the default branch's head. Idempotent-ish: an existing
 *  branch of the same name is left alone (the job resumes on it). */
export async function createBranch(repo: string, branch: string, baseOverride?: string): Promise<{ base: string; created: boolean }> {
  const token = await githubToken(repo)
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
  const token = await githubToken(repo)
  if (!token) throw new Error('GitHub is not connected')
  const cmp = await ghJson<{ ahead_by: number }>(`/repos/${repo}/compare/${encodeURIComponent(base)}...${encodeURIComponent(branch)}`, token)
  return cmp.ahead_by
}

export async function createPullRequest(
  repo: string,
  input: { head: string; base: string; title: string; body: string; draft?: boolean },
): Promise<{ url: string; number: number }> {
  const token = await githubToken(repo)
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
/** THE CLONE URL AN AGENT IS GIVEN — and it no longer carries the token.
 *
 *  It used to be `https://x-access-token:<token>@github.com/…`, returned in the
 *  `start_job` tool RESULT, which is the model's context. The job prompt even
 *  said "the token is short-lived — clone now", which is an accurate
 *  description of a credential we had just handed to a language model.
 *
 *  It does not need to be there any more. The sandbox's git is configured with
 *  Talaria's credential helper, so git asks US when it needs one — see
 *  `agentGitCredential` below and `secrets.git-credential.ts`. The agent clones
 *  a plain URL, the push works, and no credential was ever in the transcript.
 *
 *  Still nullable: a deployment with no GitHub configured has no repo to offer,
 *  and every caller already handles that. */
export async function cloneUrl(repo: string): Promise<string | null> {
  return (await githubToken(repo)) ? `https://github.com/${repo}.git` : null
}

/** GIT'S CREDENTIAL, for a repo this agent was actually granted.
 *
 *  The second source behind `/api/secrets/git-credential`. The first is the
 *  workspace credential store; this is Talaria's own GitHub installation token,
 *  which is what a workbench job needs in order to push and which no workspace
 *  credential replaces.
 *
 *  SCOPED BY THE PATH GIT ASKS ABOUT. Git sends `path` when
 *  `credential.useHttpPath` is set — the rendered gitconfig sets it — so the
 *  request names the repository, and an agent gets a token only for a repo on
 *  its own grant list. Without that, this would hand every agent a token for
 *  every repo the installation can reach: a far worse deal than the clone URL
 *  it replaces. */
export async function agentGitCredential(
  agentId: string,
  host: string,
  path: string | undefined,
): Promise<{ username: string; password: string; repo: string } | null> {
  if (host.toLowerCase() !== 'github.com') return null
  const repo = (path ?? '').replace(/^\/+/, '').replace(/\.git$/, '')
  // `owner/name` and nothing else: a path we cannot read as a repo is a request
  // we cannot scope, and an unscoped answer is what this check exists to stop.
  if (!/^[^/]+\/[^/]+$/.test(repo)) return null
  if (!(await grantedRepos(agentId)).includes(repo)) return null
  const token = await githubToken(repo)
  return token ? { username: 'x-access-token', password: token, repo } : null
}
