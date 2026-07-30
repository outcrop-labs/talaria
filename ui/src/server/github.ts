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

/** Repos the connection can reach — the pool agent grants pick from. */
export async function listReachableRepos(): Promise<string[]> {
  const cfg = await getGithubConfig()
  const token = await githubToken()
  if (!token) return []
  if (cfg.mode === 'app') {
    const res = await gh('/installation/repositories?per_page=100', token)
    if (!res.ok) return []
    const j = (await res.json()) as { repositories: Array<{ full_name: string }> }
    return j.repositories.map((r) => r.full_name)
  }
  const res = await gh('/user/repos?per_page=100&sort=pushed', token)
  if (!res.ok) return []
  return ((await res.json()) as Array<{ full_name: string }>).map((r) => r.full_name)
}

// ── Per-agent repo grants — explicit, like MCP assignment ────────────────────

export async function grantedRepos(agentId: string): Promise<string[]> {
  const sql = await db()
  const rows = (await sql`select repo from workbench_repos where agent_id = ${agentId} order by repo`) as unknown as Array<{ repo: string }>
  return rows.map((r) => r.repo)
}

export async function setGrantedRepos(agentId: string, repos: string[]): Promise<void> {
  const sql = await db()
  await sql`delete from workbench_repos where agent_id = ${agentId}`
  for (const repo of repos.slice(0, 100)) {
    await sql`insert into workbench_repos (agent_id, repo) values (${agentId}, ${repo}) on conflict do nothing`
  }
}
