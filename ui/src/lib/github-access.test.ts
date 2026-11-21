// Agent policy › GitHub access, pinned: one word per agent that the overview
// and the editor share (built on keyState, never on what the installation
// holds), the requests that wait, and the editor's permission list (which
// keys show, the levels GitHub takes, the None the last key cannot take).
//
// Fixtures are the wire: the catalog is talaria-github's CATALOG as the
// permissions GET sends it, `carried` is a GitHub mint reply's `permissions`
// block, and the requests are the permission-requests GET's two shapes.
import { describe, expect, it } from 'vitest'
import type { AgentGithubPermissions, CatalogEntry, HostPermissions, LastToken, PermissionsInstallation } from './agent-github-permissions'
import { keyState } from './agent-github-permissions'
import type { HostAgents } from './git-host-agents'
import type { DecidablePermissionRequest, FactPermissionRequest } from './git-permission-requests'
import {
  ACCESS_WORD,
  accessStatus,
  addedKeys,
  addedOf,
  githubAccessHref,
  keyLabel,
  matrixRows,
  overviewRows,
  permsHint,
  requestedKeys,
  requestRows,
  type MatrixRow,
} from './github-access'

const RW = ['read', 'write'] as const
const RWA = ['read', 'write', 'admin'] as const
const repo = (key: string, levels: readonly string[] = RW) => ({ key, scope: 'repository', levels })
const org = (key: string, levels: readonly string[] = RW) => ({ key, scope: 'organization', levels })
const CATALOG: CatalogEntry[] = JSON.parse(
  JSON.stringify([
    ...[
      'actions', 'administration', 'artifact_metadata', 'attestations', 'checks', 'codespaces', 'contents', 'dependabot_secrets',
      'deployments', 'discussions', 'environments', 'issues', 'merge_queues', 'metadata', 'packages', 'pages', 'pull_requests',
      'repository_advisories', 'repository_custom_properties', 'repository_hooks',
    ].map((k) => repo(k)),
    repo('repository_projects', RWA),
    ...['secret_scanning_alerts', 'secrets', 'security_events', 'single_file', 'statuses', 'vulnerability_alerts'].map((k) => repo(k)),
    repo('workflows', ['write']),
    org('custom_properties_for_organizations'),
    org('members'),
    org('organization_administration'),
    org('organization_announcement_banners'),
    org('organization_copilot_seat_management', ['write']),
    org('organization_custom_org_roles'),
    org('organization_custom_properties', RWA),
    org('organization_custom_roles'),
    org('organization_events', ['read']),
    ...[
      'organization_hooks', 'organization_packages', 'organization_personal_access_token_requests', 'organization_personal_access_tokens',
    ].map((k) => org(k)),
    org('organization_plan', ['read']),
    org('organization_projects', RWA),
    ...['organization_secrets', 'organization_self_hosted_runners', 'organization_user_blocking', 'team_discussions'].map((k) => org(k)),
  ]),
)

const NOW = Date.parse('2026-11-20T12:00:00Z')
const MINTED_AT = '2026-11-20T11:48:00Z' // 12 min before NOW
const OLDER_AT = '2026-11-20T09:00:00Z'
const DEFAULT = { contents: 'write', metadata: 'read', pull_requests: 'write' } as const
const CARRIED = JSON.parse('{"contents":"write","metadata":"read","pull_requests":"write"}')

/** Two installations that each accepted workflows and actions. */
const acme: PermissionsInstallation = {
  id: '91',
  account: 'acme',
  appPermissions: { actions: 'write', checks: 'read', contents: 'write', metadata: 'read', pull_requests: 'write', workflows: 'write' },
  held: { actions: 'write', checks: 'read', contents: 'write', metadata: 'read', pull_requests: 'write', workflows: 'write' },
  url: 'https://github.com/organizations/acme/settings/installations/91',
}
const beta: PermissionsInstallation = { ...acme, id: '92', account: 'beta', url: null }

const token = (over: Partial<LastToken> = {}): LastToken => ({
  at: MINTED_AT,
  ok: true,
  source: 'git_host',
  asked: { ...DEFAULT },
  carried: CARRIED,
  shortfall: {},
  shortfallWhy: null,
  error: null,
  minted: true,
  current: true,
  ...over,
})

const host = (over: Partial<HostPermissions> = {}): HostPermissions => ({
  host: 'github',
  name: 'GitHub',
  kind: 'github',
  narrowing: 'perRepo',
  hostWide: null,
  permissions: { ...DEFAULT },
  isDefault: true,
  appPermissions: acme.appPermissions,
  installations: [acme, beta],
  repos: [{ repo: 'acme/widgets', installation: '91', lastToken: token() }],
  workspaceSecret: null,
  ...over,
})

const perms = (hosts: HostPermissions[]): AgentGithubPermissions => ({
  agentId: 'a1',
  canEdit: true,
  canCheck: true,
  default: { ...DEFAULT },
  catalog: CATALOG,
  hosts,
})

const on = { developer: true, enabled: true }
const word = (p: AgentGithubPermissions, def = on) => accessStatus(def, p).word

describe('the catalog fixture', () => {
  it("is GitHub's 47 keys", () => {
    expect(CATALOG).toHaveLength(47)
  })
})

describe('accessStatus', () => {
  it('says each word, in precedence: the switch, retirement, repositories, then the worst App host', () => {
    // Developer off beats everything, retired beats having no repos.
    expect(word(perms([host()]), { developer: false, enabled: false })).toBe('Developer off')
    expect(word(perms([host()]), { developer: true, enabled: false })).toBe('Retired')
    expect(word(perms([host({ repos: [] })]))).toBe('No repositories')
    expect(word(perms([]))).toBe('No repositories')
    // The App hosts: one token per repo, every key of the set on it.
    expect(word(perms([host()]))).toBe('Carries its set')
    expect(accessStatus(on, perms([host()])).tone).toBe('ok')
    expect(word(perms([host({ repos: [{ repo: 'acme/widgets', installation: '91', lastToken: token({ current: false }) }] })]))).toBe('Not checked')
    expect(word(perms([host({ repos: [{ repo: 'acme/widgets', installation: '91', lastToken: null }] })]))).toBe('Not checked')
    expect(word(perms([host({ workspaceSecret: 'deploy-pat' })]))).toBe('Workspace secret')
    const refused = token({ ok: false, carried: null, error: 'GitHub refused the token: 422', current: null })
    expect(word(perms([host({ repos: [{ repo: 'acme/widgets', installation: '91', lastToken: refused }] })]))).toBe('Refused')
    // Every word the page can say is one of these.
    expect(Object.values(ACCESS_WORD)).toEqual([
      'Developer off',
      'Retired',
      'No repositories',
      'Refused',
      'Missing permissions',
      'Not checked',
      'Workspace secret',
      'Carries its set',
      'Host token',
    ])
  })

  it('takes the worst word across the App hosts: Refused > Missing > Not checked > Workspace secret > Carries', () => {
    const clean = host()
    const secret = host({ host: 'ghe', name: 'Enterprise', workspaceSecret: 'deploy-pat' })
    const unchecked = host({ host: 'ghe2', name: 'Enterprise 2', repos: [{ repo: 'beta/api', installation: '92', lastToken: null }] })
    const missing = host({
      host: 'ghe3',
      name: 'Enterprise 3',
      permissions: { ...DEFAULT, workflows: 'write' },
      isDefault: false,
      repos: [{ repo: 'beta/api', installation: '92', lastToken: token({ asked: { ...DEFAULT, workflows: 'write' }, shortfall: { workflows: 'write' } }) }],
    })
    const refused = host({
      host: 'ghe4',
      name: 'Enterprise 4',
      repos: [{ repo: 'beta/api', installation: '92', lastToken: token({ ok: false, carried: null, error: 'no', current: null }) }],
    })
    expect(word(perms([clean, secret]))).toBe('Workspace secret')
    expect(word(perms([secret, unchecked]))).toBe('Not checked')
    expect(word(perms([unchecked, missing]))).toBe('Missing permissions')
    expect(word(perms([missing, refused, clean]))).toBe('Refused')
  })

  it('REGRESSION: never ok from what the installation holds: a key the record holds reads Not checked', () => {
    // No token for this set at all; the installation holds every key.
    const b = host({ repos: [{ repo: 'acme/widgets', installation: '91', lastToken: null }] })
    expect(keyState(b, 'contents', 'write')).toEqual({ kind: 'record', held: { kind: 'held' } })
    const s = accessStatus(on, perms([b]))
    expect(s.word).toBe('Not checked')
    expect(s.tone).not.toBe('ok')
    // An api that predates lastToken: the record, never ok.
    const old = host({ repos: [{ repo: 'acme/widgets', installation: '91' }] })
    expect(word(perms([old]))).toBe('Not checked')
  })

  it('REGRESSION: two repos, the newest token clean and an older current one lacking a key, is Missing permissions, as the editor says', () => {
    const set = { ...DEFAULT, workflows: 'write' } as const
    const b = host({
      permissions: { ...set },
      isDefault: false,
      repos: [
        { repo: 'acme/widgets', installation: '91', lastToken: token({ at: MINTED_AT, asked: { ...set }, carried: { ...CARRIED, workflows: 'write' } }) },
        { repo: 'beta/api', installation: '92', lastToken: token({ at: OLDER_AT, asked: { ...set }, shortfall: { workflows: 'write' } }) },
      ],
    })
    // The editor's row for workflows.
    expect(keyState(b, 'workflows', 'write')).toMatchObject({ kind: 'offToken', repo: 'beta/api' })
    const s = accessStatus(on, perms([b]))
    expect(s).toMatchObject({ kind: 'missing', word: 'Missing permissions', tone: 'warn', missing: ['workflows: write'] })
  })

  it('a workspace secret beats an older clean token: none of these permissions apply to the next push', () => {
    const b = host({ workspaceSecret: 'deploy-pat', repos: [{ repo: 'acme/widgets', installation: '91', lastToken: token({ at: OLDER_AT }) }] })
    expect(word(perms([b]))).toBe('Workspace secret')
  })

  it('reads Host token for an agent that works only where the host token decides', () => {
    const pat = host({ host: 'forge', name: 'Forge', kind: 'forgejo', narrowing: 'hostWide', hostWide: 'a Forgejo token', permissions: null, isDefault: null, repos: [{ repo: 'ops/infra' }] })
    expect(word(perms([pat]))).toBe('Host token')
    // With an App host beside it, the App host's word.
    expect(word(perms([pat, host()]))).toBe('Carries its set')
  })

  it('ignores an App host where it holds a set but no repo: no token is minted there', () => {
    const idle = host({ host: 'ghe', name: 'Enterprise', repos: [], permissions: { ...DEFAULT, workflows: 'write' }, isDefault: false })
    expect(word(perms([host(), idle]))).toBe('Carries its set')
  })
})

describe('addedOf and addedKeys', () => {
  it("is the server's field when it sends one", () => {
    expect(addedOf(token({ added: { checks: 'read' }, carried: { ...CARRIED, checks: 'read', issues: 'write' } }))).toEqual({ checks: 'read' })
  })

  it('is derived from what was carried beyond the ask, metadata aside, on an api without the field', () => {
    expect(addedOf(token({ carried: { ...CARRIED, checks: 'read' } }))).toEqual({ checks: 'read' })
    expect(addedOf(token({ asked: { contents: 'write', pull_requests: 'write' } }))).toEqual({})
  })

  it('is empty when what was asked or what was carried is not on record', () => {
    expect(addedOf(token({ asked: null, carried: { ...CARRIED, checks: 'read' } }))).toEqual({})
    expect(addedOf(token({ asked: undefined, carried: { ...CARRIED, checks: 'read' } }))).toEqual({})
    expect(addedOf(token({ carried: null }))).toEqual({})
    expect(addedOf(null)).toEqual({})
  })

  it("lists what GitHub added on the host's current tokens, minus every key of the set, cited from the newest", () => {
    const b = host({
      permissions: { ...DEFAULT, checks: 'write' },
      isDefault: false,
      repos: [
        { repo: 'acme/widgets', installation: '91', lastToken: token({ asked: { ...DEFAULT, checks: 'write' }, carried: { ...CARRIED, checks: 'write', actions: 'read' } }) },
        { repo: 'beta/api', installation: '92', lastToken: token({ at: OLDER_AT, added: { actions: 'read', statuses: 'read' } }) },
        // A token for an earlier set says nothing about the next one.
        { repo: 'beta/old', installation: '92', lastToken: token({ current: false, added: { pages: 'write' } }) },
      ],
    })
    expect(addedKeys(b)).toEqual([
      { key: 'actions', level: 'read', repo: 'acme/widgets', at: MINTED_AT },
      { key: 'statuses', level: 'read', repo: 'beta/api', at: OLDER_AT },
    ])
  })
})

describe('the overview', () => {
  const defs = [
    { id: 'a1', displayName: 'Builder', model: 'builder', developer: true, enabled: true },
    { id: 'a2', displayName: 'Reviewer', model: 'reviewer', developer: true, enabled: true },
    { id: 'a3', displayName: 'Ops bot', model: 'ops', developer: true, enabled: true },
    { id: 'a4', displayName: 'Scout', model: 'scout', developer: false, enabled: true },
    { id: 'a5', displayName: 'Archivist', model: 'archivist', developer: true, enabled: false },
  ]
  const missingHost = host({
    permissions: { ...DEFAULT, workflows: 'write' },
    isDefault: false,
    repos: [
      { repo: 'acme/widgets', installation: '91', lastToken: token({ asked: { ...DEFAULT, workflows: 'write' }, shortfall: { workflows: 'write' } }) },
      { repo: 'acme/api', installation: '91', lastToken: null },
    ],
  })
  const request: DecidablePermissionRequest = {
    id: 'r1',
    content: true,
    status: 'pending',
    host: 'github',
    hostName: 'GitHub',
    repo: 'acme/widgets',
    permission: 'workflows',
    level: 'write',
    agentId: 'a1',
    agentName: 'Builder',
    agentModel: 'builder',
    why: 'The push of .github/workflows/ci.yml was refused.',
    evidence: [],
    held: true,
    installation: '91',
    installationUrl: acme.url,
    inSet: true,
    appliesTo: ['acme/widgets', 'acme/api'],
    ticket: { id: 't1', ref: 'TICK-12', title: 'CI fix', boardId: 'b1', status: 'blocked', statusLabel: 'Blocked', blocked: true, restartStatus: 'in_progress' },
    decidedBy: null,
    decisionNote: null,
    createdAt: MINTED_AT,
    updatedAt: MINTED_AT,
  }
  const fact: FactPermissionRequest = { id: 'r2', content: false, status: 'pending', host: 'github', hostName: 'GitHub', createdAt: MINTED_AT }
  const agentsOnGithub: HostAgents = JSON.parse(
    JSON.stringify({
      agents: [
        { agentId: 'a1', name: 'Builder', developer: true, isDefault: false, set: {}, repos: ['acme/api', 'acme/widgets'], carries: null },
        { agentId: 'a4', name: 'Scout', developer: false, isDefault: true, set: {}, repos: ['acme/docs'], carries: null },
        { agentId: 'a5', name: 'Archivist', developer: false, isDefault: true, set: {}, repos: ['acme/old', 'acme/older'], carries: null },
      ],
      unclaimed: { actions: 'write', workflows: 'write' },
    }),
  )

  const overview = overviewRows({
    defs,
    perms: {
      a1: { data: perms([missingHost]) },
      a2: { data: perms([host()]) },
      a3: { error: 'Could not reach the api (502)' },
    },
    hosts: [
      { id: 'github', name: 'GitHub', kind: 'github', body: agentsOnGithub },
      // A server without the agents route answers 404: no notice, no counts.
      { id: 'ghe', name: 'Enterprise', kind: 'github', body: null },
      { id: 'forge', name: 'Forge', kind: 'forgejo', body: { agents: [], unclaimed: ['issues'] } },
    ],
    requests: [request, fact],
  })

  it('lists the live Developer agents by name, then the retired ones, each from its own read', () => {
    expect(overview.developer.map((r) => [r.name, r.status?.word ?? null, r.hint])).toEqual([
      ['Builder', 'Missing permissions', 'Custom permissions · 2 repositories'],
      ['Ops bot', null, 'No repositories'],
      ['Reviewer', 'Carries its set', 'Default permissions · 1 repository'],
      ['Archivist', 'Retired', '2 repositories'],
    ])
    expect(overview.developer[0]).toMatchObject({ missing: ['workflows: write'], waiting: 1, href: '/admin/github?agent=a1' })
  })

  it('says a failed per-agent read on its row, which still opens', () => {
    const ops = overview.developer.find((r) => r.id === 'a3')
    expect(ops).toMatchObject({ status: null, error: 'Could not reach the api (502)', href: githubAccessHref('a3') })
  })

  it('lists agents without the Developer switch apart, with repo counts from the agents route', () => {
    expect(overview.others.map((r) => [r.name, r.status?.word, r.hint])).toEqual([['Scout', 'Developer off', '1 repository']])
  })

  it('says the unclaimed keys once per GitHub host, and nothing for a 404 or a host whose own token decides', () => {
    expect(overview.unclaimed).toEqual([
      {
        hostId: 'github',
        hostName: 'GitHub',
        keys: ['actions', 'workflows'],
        sentence: 'Accepted by the installation on GitHub, granted to no agent: actions, workflows. Open an agent to grant them.',
      },
    ])
  })

  it('counts no repos while no host read has landed, and says none when every read says none', () => {
    const none = overviewRows({ defs: [defs[3]!], perms: {}, hosts: [{ id: 'github', name: 'GitHub', kind: 'github', body: undefined }], requests: [] })
    expect(none.others[0]?.hint).toBe('')
    const empty = overviewRows({ defs: [defs[3]!], perms: {}, hosts: [{ id: 'github', name: 'GitHub', kind: 'github', body: { agents: [] } }], requests: [] })
    expect(empty.others[0]?.hint).toBe('No repositories')
  })

  it('hints the hosts when the repos span several', () => {
    const two = perms([host(), host({ host: 'ghe', name: 'Enterprise', repos: [{ repo: 'beta/api', installation: '92', lastToken: null }] })])
    expect(permsHint(two)).toBe('Default permissions · 2 repositories, on 2 hosts')
    expect(permsHint(perms([]))).toBe('No repositories')
  })

  it('lists the requests as compact rows that open the agent, fact-only ones as a sentence, and counts repo requests of listed agents', () => {
    const awaiting: DecidablePermissionRequest = { ...request, id: 'r3', status: 'awaiting_github', held: false, ticket: null }
    const { rows, repoLine } = requestRows(
      [request, awaiting, fact],
      [
        { id: 'q1', agentId: 'a1', org: 'acme', name: 'tools' },
        { id: 'q2', agentId: 'a4', org: 'acme', name: 'docs-site' },
        { id: 'q3', agentId: 'gone', org: 'acme', name: 'stray' },
      ],
      defs,
    )
    expect(rows).toEqual([
      {
        kind: 'decide',
        id: 'r1',
        agentId: 'a1',
        agentName: 'Builder',
        label: 'Builder asks for workflows write',
        hint: 'on GitHub, where it worked on acme/widgets · TICK-12 · the installation holds it',
        word: 'Waiting on you',
        tone: 'warn',
        href: '/admin/github?agent=a1',
      },
      {
        kind: 'decide',
        id: 'r3',
        agentId: 'a1',
        agentName: 'Builder',
        label: 'Builder asks for workflows write',
        hint: 'on GitHub, where it worked on acme/widgets · the installation has not accepted it',
        word: 'Waiting on GitHub',
        tone: 'idle',
        href: '/admin/github?agent=a1',
      },
      {
        kind: 'fact',
        id: 'r2',
        text: 'A GitHub permission request waits on GitHub. It was raised on a board you are not a member of; an admin who is a member decides it.',
      },
    ])
    expect(repoLine).toBe('2 new-repository requests wait in Git hosts')
    expect(requestRows([], [{ id: 'q1', agentId: 'a1', org: 'acme', name: 'tools' }], defs).repoLine).toBe('1 new-repository request waits in Git hosts')
    expect(requestRows([], [], defs)).toEqual({ rows: [], repoLine: null })
  })

  it("names the keys each agent asked for on a host, and never a fact-only request's", () => {
    expect(requestedKeys([request, fact], 'a1', 'github')).toEqual(['workflows'])
    expect(requestedKeys([request], 'a1', 'ghe')).toEqual([])
    expect(requestedKeys([request], 'a2', 'github')).toEqual([])
  })
})

describe('matrixRows', () => {
  const keys = (rows: Record<'repository' | 'organization', MatrixRow[]>) => [...rows.repository, ...rows.organization].map((r) => r.key)
  const row = (rows: Record<'repository' | 'organization', MatrixRow[]>, key: string) => [...rows.repository, ...rows.organization].find((r) => r.key === key)!

  it("lists its set, what GitHub added, what it asked for and what was changed this visit, in the catalog's order", () => {
    const b = host({ repos: [{ repo: 'acme/widgets', installation: '91', lastToken: token({ added: { checks: 'read' }, carried: { ...CARRIED, checks: 'read' } }) }] })
    const rows = matrixRows(b, CATALOG, { view: 'set', requested: ['workflows'], touched: ['members'], nowMs: NOW })
    expect(keys(rows)).toEqual(['checks', 'contents', 'metadata', 'pull_requests', 'workflows', 'members'])
    expect(rows.organization.map((r) => r.key)).toEqual(['members'])
    expect(row(rows, 'workflows').requested).toBe(true)
  })

  it('lists the whole catalog under All, and a key the catalog does not know after it', () => {
    const b = host({ permissions: { ...DEFAULT, copilot_requests: 'write' }, isDefault: false })
    const rows = matrixRows(b, CATALOG, { view: 'all', nowMs: NOW })
    expect(keys(rows)).toHaveLength(48)
    expect(rows.repository.at(-1)?.key).toBe('copilot_requests')
    expect(row(rows, 'copilot_requests').options.map((o) => o.id)).toEqual(['none', 'write'])
  })

  it('searches the whole catalog by key or name whenever the filter is not empty, in either view', () => {
    for (const view of ['set', 'all'] as const) {
      expect(keys(matrixRows(host(), CATALOG, { view, filter: 'work', nowMs: NOW })), view).toEqual(['workflows'])
      expect(keys(matrixRows(host(), CATALOG, { view, filter: 'Pull REQ', nowMs: NOW })), view).toEqual(['pull_requests'])
    }
    expect(keys(matrixRows(host(), CATALOG, { view: 'set', filter: 'organization_custom', nowMs: NOW }))).toEqual([
      'organization_custom_org_roles',
      'organization_custom_properties',
      'organization_custom_roles',
    ])
  })

  it('narrows only its set for a reader (searchAll false): a key outside the set is never offered', () => {
    expect(keys(matrixRows(host(), CATALOG, { view: 'set', filter: 'work', searchAll: false, nowMs: NOW }))).toEqual([])
    expect(keys(matrixRows(host(), CATALOG, { view: 'set', filter: 'Pull REQ', searchAll: false, nowMs: NOW }))).toEqual(['pull_requests'])
    expect(keys(matrixRows(host(), CATALOG, { view: 'set', filter: '', searchAll: false, nowMs: NOW }))).toEqual(['contents', 'metadata', 'pull_requests'])
  })

  it('offers None and only the levels GitHub takes a key at; metadata has no None', () => {
    const rows = matrixRows(host(), CATALOG, { view: 'all', nowMs: NOW })
    const levels = (key: string) => row(rows, key).options.map((o) => o.label)
    expect(levels('workflows')).toEqual(['None', 'Write'])
    expect(levels('organization_events')).toEqual(['None', 'Read'])
    expect(levels('repository_projects')).toEqual(['None', 'Read', 'Write', 'Admin'])
    expect(levels('metadata')).toEqual(['Read', 'Write'])
    expect(levels('contents')).toEqual(['None', 'Read', 'Write'])
  })

  it('disables None on the last key of the set, and nowhere else', () => {
    const one = host({ permissions: { contents: 'write' }, isDefault: false })
    const rows = matrixRows(one, CATALOG, { view: 'all', nowMs: NOW })
    expect(row(rows, 'contents').options.find((o) => o.id === 'none')).toMatchObject({ disabled: true })
    expect(row(rows, 'issues').options.every((o) => !o.disabled)).toBe(true)
    const many = matrixRows(host(), CATALOG, { view: 'set', nowMs: NOW })
    expect(many.repository.flatMap((r) => r.options).some((o) => o.disabled)).toBe(false)
  })

  it("words the installation as a fact, never success, and the token from the agent's own tokens", () => {
    const b = host({
      permissions: { ...DEFAULT, workflows: 'write', members: 'read' },
      isDefault: false,
      repos: [{ repo: 'acme/widgets', installation: '91', lastToken: token({ asked: { ...DEFAULT }, current: false }) }],
    })
    const rows = matrixRows(b, CATALOG, { view: 'all', nowMs: NOW })
    expect(row(rows, 'workflows').held).toEqual({ word: 'Installation holds write', tone: 'neutral' })
    expect(row(rows, 'workflows').token).toEqual({ word: 'Not checked', tone: 'idle', check: true })
    // The App does not ask for members: said in the warning tone, it is in the set.
    expect(row(rows, 'members').held).toEqual({ word: "The GitHub App doesn't ask for it", tone: 'warn' })
    expect(row(rows, 'actions').token).toEqual({ word: 'Not in its set', tone: 'idle' })
    expect(row(rows, 'actions').held.word).toBe('Installation holds write')
    expect(row(rows, 'metadata').token.word).toBe('On every token')
    for (const r of [...rows.repository, ...rows.organization]) {
      expect(['warn', 'neutral']).toContain(r.held.tone)
    }
    const fresh = matrixRows(host(), CATALOG, { view: 'set', nowMs: NOW })
    expect(row(fresh, 'contents').token).toEqual({ word: 'On the token (acme/widgets, 12 min ago)', tone: 'ok' })
  })

  it('says GitHub added a key outside the set, neutral at read and warn above, and that None does not take it off', () => {
    const b = host({ repos: [{ repo: 'acme/widgets', installation: '91', lastToken: token({ added: { checks: 'read', administration: 'write' } }) }] })
    const rows = matrixRows(b, CATALOG, { view: 'set', nowMs: NOW })
    expect(row(rows, 'checks')).toMatchObject({ level: null, token: { word: 'GitHub added it, read (acme/widgets, 12 min ago)', tone: 'idle' } })
    expect(row(rows, 'checks').token.title).toContain('None here does not take it off')
    expect(row(rows, 'administration').token.tone).toBe('warn')
  })

  it('says what is held lower than the set asks, and where to accept what is not accepted', () => {
    const lower = host({
      permissions: { ...DEFAULT, checks: 'write' },
      isDefault: false,
      installations: [{ ...acme, appPermissions: { ...acme.appPermissions, issues: 'write' } }],
      appPermissions: { ...acme.appPermissions, issues: 'write' },
    })
    const rows = matrixRows(lower, CATALOG, { view: 'all', nowMs: NOW })
    expect(row(rows, 'checks').held).toEqual({ word: 'Installation holds read only', tone: 'warn' })
    expect(row(rows, 'issues').held).toEqual({
      word: 'Not accepted on GitHub by acme.',
      tone: 'neutral',
      link: { href: acme.url, label: 'Accept it on GitHub ↗' },
    })
  })

  it('names a key for reading', () => {
    expect(keyLabel('pull_requests')).toBe('Pull requests')
    expect(keyLabel('workflows')).toBe('Workflows')
  })
})
