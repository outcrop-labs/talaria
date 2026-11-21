// The repo editor's GitHub permissions arithmetic, pinned: a key is measured
// against the installations that mint the agent's tokens, a key that is not
// held says why (the App does not ask, or an owner has not accepted), the add
// picker offers what rides at once first, and each edit writes the whole next
// set from what it read.
import { describe, expect, it } from 'vitest'
import {
  addOptions,
  checkClean,
  checkLines,
  checkSentences,
  fromOf,
  heldSentence,
  heldState,
  impactNote,
  keyLines,
  keySentence,
  keyState,
  keyTone,
  mintingInstallations,
  reasonSentence,
  setRows,
  since,
  tokenAware,
  withPermission,
  withoutPermission,
  workspaceSecretNotice,
  type CatalogEntry,
  type HostPermissions,
  type LastToken,
  type PermissionsInstallation,
  type TokenCheck,
} from './agent-github-permissions'

const catalog: CatalogEntry[] = [
  { key: 'actions', scope: 'repository', levels: ['read', 'write'] },
  { key: 'administration', scope: 'repository', levels: ['read', 'write'] },
  { key: 'contents', scope: 'repository', levels: ['read', 'write'] },
  { key: 'issues', scope: 'repository', levels: ['read', 'write'] },
  { key: 'metadata', scope: 'repository', levels: ['read', 'write'] },
  { key: 'pull_requests', scope: 'repository', levels: ['read', 'write'] },
  { key: 'secrets', scope: 'repository', levels: ['read', 'write'] },
  { key: 'workflows', scope: 'repository', levels: ['write'] },
  { key: 'members', scope: 'organization', levels: ['read', 'write'] },
  { key: 'organization_secrets', scope: 'organization', levels: ['read', 'write'] },
]

const acme: PermissionsInstallation = {
  id: '91',
  account: 'acme',
  appPermissions: { contents: 'write', issues: 'write', metadata: 'read', pull_requests: 'write', workflows: 'write' },
  held: { contents: 'write', issues: 'write', metadata: 'read', pull_requests: 'write' },
  url: 'https://github.com/organizations/acme/settings/installations/91',
}
const beta: PermissionsInstallation = {
  id: '92',
  account: 'beta',
  appPermissions: acme.appPermissions,
  held: { contents: 'write', issues: 'read', metadata: 'read', pull_requests: 'write', workflows: 'write' },
  url: null,
}

const block = (over: Partial<HostPermissions> = {}): HostPermissions => ({
  host: 'github',
  name: 'GitHub',
  kind: 'github',
  narrowing: 'perRepo',
  hostWide: null,
  permissions: { contents: 'write', metadata: 'read', pull_requests: 'write' },
  isDefault: true,
  appPermissions: acme.appPermissions,
  installations: [acme, beta],
  repos: [{ repo: 'acme/widgets', installation: '91' }],
  ...over,
})

describe('mintingInstallations', () => {
  it("is the installations the agent's repos are minted on", () => {
    expect(mintingInstallations(block()).map((i) => i.id)).toEqual(['91'])
    expect(mintingInstallations(block({ repos: [{ repo: 'acme/a', installation: '91' }, { repo: 'beta/b', installation: '92' }] })).map((i) => i.id)).toEqual(['91', '92'])
  })

  it('is every installation the host uses while the agent holds no repo there', () => {
    expect(mintingInstallations(block({ repos: [] })).map((i) => i.id)).toEqual(['91', '92'])
  })

  it('is none when the agent holds repos no installation on record mints, as the server measures', () => {
    const unminted = block({ repos: [{ repo: 'gamma/c', installation: null }] })
    expect(mintingInstallations(unminted)).toEqual([])
    expect(heldState(unminted, 'workflows', 'write')).toEqual({ kind: 'unknown' })
  })
})

describe('heldState', () => {
  it('is held when every installation that mints the tokens holds it at the level', () => {
    expect(heldState(block(), 'issues', 'write')).toEqual({ kind: 'held' })
    expect(heldSentence(heldState(block(), 'issues', 'write'))).toBeNull()
  })

  it('waits for an owner when the App asks and the installation has not accepted', () => {
    const s = heldState(block(), 'workflows', 'write')
    expect(s).toEqual({ kind: 'awaitingApproval', installations: [acme] })
    expect(heldSentence(s)).toContain('owner of acme')
  })

  it("says the App doesn't ask when its registration lacks the key or the level", () => {
    expect(heldState(block(), 'administration', 'write')).toEqual({ kind: 'appMissing' })
    expect(heldState(block(), 'metadata', 'write')).toEqual({ kind: 'appMissing' })
    expect(heldSentence({ kind: 'appMissing' })).toContain("doesn't ask")
  })

  it('measures every minting installation: one that lacks it is enough', () => {
    const both = block({ repos: [{ repo: 'acme/a', installation: '91' }, { repo: 'beta/b', installation: '92' }] })
    expect(heldState(both, 'issues', 'write')).toEqual({ kind: 'awaitingApproval', installations: [beta] })
  })

  it('is unknown, never "not held", while no installation was read', () => {
    const unread = block({ installations: [{ ...acme, held: null }] })
    expect(heldState(unread, 'contents', 'write')).toEqual({ kind: 'unknown' })
    expect(heldSentence({ kind: 'unknown' })).toContain('Refresh')
  })

  it("is plainly not held when the App's ask is not on record", () => {
    const noAsk = block({ appPermissions: null, installations: [{ ...acme, appPermissions: null }] })
    expect(heldState(noAsk, 'workflows', 'write')).toEqual({ kind: 'notHeld', installations: [{ ...acme, appPermissions: null }] })
  })
})

describe('setRows', () => {
  it('groups the set Repository then Organization, in catalog order, with the levels GitHub takes', () => {
    const rows = setRows({ pull_requests: 'write', organization_secrets: 'read', contents: 'write', workflows: 'write', future_key: 'read' }, catalog)
    expect(rows.repository.map((r) => r.key)).toEqual(['contents', 'pull_requests', 'workflows', 'future_key'])
    expect(rows.repository.find((r) => r.key === 'workflows')?.levels).toEqual(['write'])
    expect(rows.repository.find((r) => r.key === 'future_key')?.levels).toEqual(['read'])
    expect(rows.organization).toEqual([{ key: 'organization_secrets', level: 'read', levels: ['read', 'write'], scope: 'organization' }])
  })

  it('is empty for a host whose own token decides', () => {
    expect(setRows(null, catalog)).toEqual({ repository: [], organization: [] })
  })
})

describe('addOptions', () => {
  it('offers what the installation holds first, then what the App asks, then the rest', () => {
    const keys = addOptions(block(), catalog).map((o) => o.key)
    expect(keys).toEqual(['issues', 'workflows', 'actions', 'administration', 'secrets', 'members', 'organization_secrets'])
  })

  it('adds a held key at the highest level the installation holds, anything else at the lowest GitHub takes', () => {
    const options = addOptions(block(), catalog)
    expect(options.find((o) => o.key === 'issues')).toMatchObject({ level: 'write', note: 'repository · held' })
    expect(options.find((o) => o.key === 'workflows')).toMatchObject({ level: 'write', note: 'repository · the App asks, not accepted yet' })
    expect(options.find((o) => o.key === 'secrets')).toMatchObject({ level: 'read', note: "repository · the App doesn't ask for it" })
    expect(options.find((o) => o.key === 'organization_secrets')?.scope).toBe('organization')
  })

  it('never offers a key the set already names', () => {
    expect(addOptions(block(), catalog).some((o) => o.key === 'contents')).toBe(false)
  })
})

describe('the next set', () => {
  it('changes one key and keeps the rest', () => {
    const set = { contents: 'write', metadata: 'read' } as const
    expect(withPermission(set, 'contents', 'read')).toEqual({ contents: 'read', metadata: 'read' })
    expect(withPermission(set, 'administration', 'write')).toEqual({ contents: 'write', metadata: 'read', administration: 'write' })
    expect(withoutPermission(set, 'metadata')).toEqual({ contents: 'write' })
    expect(set).toEqual({ contents: 'write', metadata: 'read' })
  })

  it('says it read the default as null, and a set as the set', () => {
    expect(fromOf(block())).toBeNull()
    expect(fromOf(block({ isDefault: false, permissions: { administration: 'write' } }))).toEqual({ administration: 'write' })
  })
})

describe('impactNote', () => {
  it('describes the high-impact keys, and only those', () => {
    for (const key of ['administration', 'secrets', 'workflows', 'organization_secrets', 'organization_administration', 'members']) {
      expect(impactNote(key), key).toBeTruthy()
    }
    expect(impactNote('contents')).toBeNull()
    expect(impactNote('issues')).toBeNull()
  })
})

// ── The agent's token, not the installation's record ─────────────────────────
//
// Fixtures are the wire: `carried` is exactly the `permissions` block of a
// GitHub mint reply (POST /app/installations/{id}/access_tokens → 201) for the
// default ask, and the shortfall is the key the agent's set names that the
// reply does not carry.

const NOW = Date.parse('2026-11-20T12:00:00Z')
const MINTED_AT = '2026-11-20T11:48:00Z' // 12 min before NOW
const CARRIED = JSON.parse('{"contents":"write","metadata":"read","pull_requests":"write"}')
const SHORTFALL = JSON.parse('{"workflows":"write"}')
const WITH_WORKFLOWS = { contents: 'write', metadata: 'read', pull_requests: 'write', workflows: 'write' } as const
// GitHub's 422 for an access-token ask it will not mint, as the mint keeps it
// (the reply's `message`): it names no key.
const GITHUB_422 = JSON.parse(
  '{"message":"The permissions requested are not granted to this installation.","documentation_url":"https://docs.github.com/rest/apps/apps#create-an-installation-access-token-for-an-app","status":"422"}',
).message as string

/** An installation that accepted workflows and actions, as the record has it. */
const accepted: PermissionsInstallation = {
  ...acme,
  appPermissions: { ...acme.appPermissions, actions: 'write' },
  held: { actions: 'write', contents: 'write', issues: 'write', metadata: 'read', pull_requests: 'write', workflows: 'write' },
}

const token = (over: Partial<LastToken> = {}): LastToken => ({
  at: MINTED_AT,
  ok: true,
  source: 'git_host',
  asked: WITH_WORKFLOWS,
  carried: CARRIED,
  shortfall: SHORTFALL,
  shortfallWhy: null,
  error: null,
  minted: true,
  current: true,
  ...over,
})

/** The agent's set names workflows, the installation holds it, and the last
 *  token is whatever `t` says. */
const granted = (t: LastToken | null | undefined, over: Partial<HostPermissions> = {}): HostPermissions =>
  block({
    permissions: { ...WITH_WORKFLOWS },
    isDefault: false,
    appPermissions: accepted.appPermissions,
    installations: [accepted],
    repos: [{ repo: 'acme/widgets', installation: '91', ...(t === undefined ? {} : { lastToken: t }) }],
    ...over,
  })

describe('since', () => {
  it('says how long ago, in the editor\'s words', () => {
    expect(since(MINTED_AT, NOW)).toBe('12 min ago')
    expect(since('2026-11-20T11:59:50Z', NOW)).toBe('just now')
    expect(since('2026-11-20T09:00:00Z', NOW)).toBe('3 h ago')
    expect(since('2026-11-17T12:00:00Z', NOW)).toBe('3 days ago')
    expect(since(null, NOW)).toBe('at an unknown time')
  })
})

describe('keyState: absent token fields draw today\'s view', () => {
  it('is the record, exactly heldState, for every key when no repo says lastToken', () => {
    const today = granted(undefined)
    expect(tokenAware(today)).toBe(false)
    for (const [key, level] of Object.entries(today.permissions ?? {})) {
      expect(keyState(today, key, level)).toEqual({ kind: 'record', held: heldState(today, key, level) })
    }
    expect(keyState(block(), 'workflows', 'write')).toEqual({ kind: 'record', held: { kind: 'awaitingApproval', installations: [acme] } })
  })

  it('tones the chips as before: neutral held or unknown, warn otherwise, never green', () => {
    expect(keyTone(keyState(block(), 'contents', 'write'))).toBe('neutral')
    expect(keyTone(keyState(block(), 'workflows', 'write'))).toBe('warn')
    expect(keyTone({ kind: 'record', held: { kind: 'unknown' } })).toBe('neutral')
  })

  it('sends no workspace-secret notice', () => {
    expect(workspaceSecretNotice(block())).toBeNull()
  })
})

describe('keyState: the token decides', () => {
  it('REGRESSION: a key the installation holds but the last token lacks is never on the token', () => {
    const b = granted(token())
    // The record alone would call it held, which the editor drew as fine.
    expect(heldState(b, 'workflows', 'write')).toEqual({ kind: 'held' })
    const s = keyState(b, 'workflows', 'write')
    expect(s.kind).not.toBe('onToken')
    expect(s).toMatchObject({ kind: 'offToken', repo: 'acme/widgets', at: MINTED_AT })
    expect(keyTone(s)).toBe('warn')
    expect(keySentence(s, NOW)).toMatch(/^Not on the token \(acme\/widgets, 12 min ago\): /)
  })

  it('REGRESSION: a token minted for an earlier set is never on the token, even when it carried the key', () => {
    const stale = granted(token({ current: false, carried: { ...WITH_WORKFLOWS }, shortfall: {} }))
    for (const key of ['contents', 'workflows']) {
      const s = keyState(stale, key, 'write')
      expect(s).toEqual({ kind: 'notYet' })
      expect(keyTone(s)).not.toBe('success')
    }
    expect(keySentence({ kind: 'notYet' }, NOW)).toBe('Granted; no token checked against this set yet')
  })

  it('REGRESSION: with no token for this set, a key the installation has not accepted keeps its warning and the link to accept', () => {
    // Granted ahead of GitHub's acceptance: the only token predates the grant.
    const b = granted(token({ current: false, asked: { ...CARRIED }, shortfall: {} }), { installations: [acme], appPermissions: acme.appPermissions })
    const s = keyState(b, 'workflows', 'write')
    expect(s).toEqual({ kind: 'record', held: { kind: 'awaitingApproval', installations: [acme] } })
    expect(keyTone(s)).toBe('warn')
    expect(keySentence(s, NOW)).toBe('Waiting for an owner of acme to accept it on GitHub. No token carries it until then.')
    const line = keyLines(b, catalog, NOW).find((l) => l.keys.some((k) => k.key === 'workflows'))
    expect(line?.installations).toEqual([acme])
    // What the App does not ask for says so too.
    const unasked = granted(token({ current: false }), {
      installations: [{ ...acme, appPermissions: { ...CARRIED } }],
      appPermissions: { ...CARRIED },
    })
    expect(keyState(unasked, 'workflows', 'write')).toEqual({ kind: 'record', held: { kind: 'appMissing' } })
    // A key it holds is still only granted.
    expect(keyState(b, 'contents', 'write')).toEqual({ kind: 'notYet' })
  })

  it('reads a row from before rows said what was asked as no token for this set, never as on the token', () => {
    // The server never calls such a row current (it cannot say which set it
    // was for), even though it carried these very keys.
    const legacy = granted(token({ asked: undefined, current: false, carried: { ...CARRIED }, shortfall: {} }), { permissions: { ...CARRIED }, isDefault: true })
    for (const [key, level] of Object.entries(CARRIED) as Array<[string, 'read' | 'write']>) {
      expect(keyState(legacy, key, level)).toEqual({ kind: 'notYet' })
    }
  })

  it('REGRESSION: a key the last token carried is not green once the installation stops holding it', () => {
    const lost = granted(token(), { installations: [{ ...accepted, held: { ...accepted.held, contents: 'read' } }] })
    const s = keyState(lost, 'contents', 'write')
    expect(s).toMatchObject({ kind: 'record', held: { kind: 'awaitingApproval' } })
    expect(keyTone(s)).toBe('warn')
  })

  it('is on the token when the token minted for this set carries it, with the repo and when', () => {
    const s = keyState(granted(token()), 'contents', 'write')
    expect(s).toEqual({ kind: 'onToken', repo: 'acme/widgets', at: MINTED_AT })
    expect(keySentence(s, NOW)).toBe("On the agent's token (acme/widgets, 12 min ago)")
    expect(keyTone(s)).toBe('success')
    // Carried at read is not carried at write.
    expect(keyState(granted(token({ carried: { ...CARRIED, contents: 'read' } })), 'contents', 'write').kind).toBe('offToken')
  })

  it('falls back to the record while nothing was minted, and never calls it verified', () => {
    const s = keyState(granted(null), 'contents', 'write')
    expect(s).toEqual({ kind: 'record', held: { kind: 'held' } })
    expect(keySentence(s, NOW)).toBe('The installation holds it; no token checked yet')
    expect(keyTone(s)).toBe('neutral')
  })

  it("says the installation hasn't accepted a key it doesn't hold, with the installation to accept it on", () => {
    const b = granted(token(), { installations: [acme], appPermissions: acme.appPermissions })
    const s = keyState(b, 'workflows', 'write')
    expect(s).toMatchObject({ kind: 'offToken', why: 'notAccepted', installations: [acme], acceptedSince: false })
    expect(keySentence(s, NOW)).toContain('the installation on acme has not accepted it on GitHub')
  })

  it('says when the installation accepted the key only after this token was minted', () => {
    const s = keyState(granted(token()), 'workflows', 'write')
    expect(s).toMatchObject({ why: 'notAccepted', acceptedSince: true, installations: [] })
    expect(keySentence(s, NOW)).toContain('the next token can carry it')
  })

  it("quotes GitHub's own words when it refused an organization key, and only for one", () => {
    const b = granted(
      token({ asked: { ...WITH_WORKFLOWS, members: 'read' }, shortfall: { members: 'read' }, shortfallWhy: GITHUB_422 }),
      { permissions: { ...WITH_WORKFLOWS, members: 'read' }, installations: [{ ...accepted, held: { ...accepted.held, members: 'read' } }] },
    )
    const s = keyState(b, 'members', 'read', 'organization')
    expect(s).toMatchObject({ why: 'githubRefused', detail: GITHUB_422 })
    expect(keySentence(s, NOW)).toContain(`GitHub refused it: ${GITHUB_422}`)
    // keyLines takes each key's scope from the catalog.
    expect(keyLines(b, catalog, NOW).find((l) => l.keys.some((k) => k.key === 'members'))?.state).toMatchObject({ why: 'githubRefused' })
  })

  it('never pins GitHub\'s refusal on a repository key: one the installation accepted after the mint can ride on the next token', () => {
    // An organization key and workflows, granted before the installation
    // accepted workflows. GitHub refused the organization key (its 422 names
    // none), workflows was never asked of it, and the installation has
    // accepted workflows since.
    const b = granted(
      token({
        asked: { ...WITH_WORKFLOWS, members: 'read' },
        shortfall: { members: 'read', workflows: 'write' },
        shortfallWhy: GITHUB_422,
      }),
      { permissions: { ...WITH_WORKFLOWS, members: 'read' }, installations: [{ ...accepted, held: { ...accepted.held, members: 'read' } }] },
    )
    const wf = keyState(b, 'workflows', 'write', 'repository')
    expect(wf).toMatchObject({ kind: 'offToken', why: 'notAccepted', acceptedSince: true })
    expect(keySentence(wf, NOW)).not.toContain('GitHub refused')
    expect(keyState(b, 'members', 'read', 'organization')).toMatchObject({ why: 'githubRefused', detail: GITHUB_422 })
  })

  it("splits GitHub's words when it refused the organization keys and left others off", () => {
    const omitted = 'GitHub did not put workflows:write on the token'
    const b = granted(
      token({
        asked: { ...WITH_WORKFLOWS, members: 'read' },
        shortfall: { members: 'read', workflows: 'write' },
        shortfallWhy: `${GITHUB_422}; ${omitted}`,
      }),
      { permissions: { ...WITH_WORKFLOWS, members: 'read' }, installations: [{ ...accepted, held: { ...accepted.held, members: 'read' } }] },
    )
    expect(keyState(b, 'workflows', 'write', 'repository')).toMatchObject({ why: 'githubOmitted', detail: omitted })
    expect(keyState(b, 'members', 'read', 'organization')).toMatchObject({ why: 'githubRefused', detail: GITHUB_422 })
  })

  it('claims no acceptance since the mint while what the installation holds is not on record', () => {
    const b = granted(token(), { installations: [{ ...accepted, held: null }] })
    const s = keyState(b, 'workflows', 'write')
    expect(s).toMatchObject({ kind: 'offToken', why: 'notAccepted', acceptedSince: false, installations: [] })
    expect(keySentence(s, NOW)).toBe('Not on the token (acme/widgets, 12 min ago): the installation had not accepted it when this token was minted')
  })

  it('says GitHub omitted a key it minted the token without', () => {
    const said = keyState(granted(token({ shortfallWhy: 'GitHub did not put workflows:write on the token' })), 'workflows', 'write')
    expect(said).toMatchObject({ why: 'githubOmitted', detail: 'GitHub did not put workflows:write on the token' })
    expect(keySentence(said, NOW)).toBe('Not on the token (acme/widgets, 12 min ago): GitHub omitted it from the token')
    // An older server records no shortfall for it: carried still decides.
    const unsaid = keyState(granted(token({ shortfall: {} })), 'workflows', 'write')
    expect(unsaid).toMatchObject({ kind: 'offToken', why: 'githubOmitted', detail: null })
    expect(keySentence(unsaid, NOW)).toContain('GitHub omitted it from the token')
  })

  it("says no token was minted, in the door's words, when it refused", () => {
    const error = 'GitHub put a key on the token that was not asked for: it carries checks:read, which was not asked for'
    const refused = token({ ok: false, carried: null, shortfall: null, error, current: null })
    for (const key of ['contents', 'workflows']) {
      const s = keyState(granted(refused), key, 'write')
      expect(s).toMatchObject({ kind: 'offToken', why: 'mintFailed', detail: error })
      expect(keySentence(s, NOW)).toContain(error)
    }
  })

  it("ignores a workspace secret's serve: it says nothing about these permissions", () => {
    expect(keyState(granted(token({ source: 'workspace_secret', carried: null, shortfall: null })), 'contents', 'write')).toEqual({ kind: 'notYet' })
  })

  it('measures every repo minted for this set: one token that lacks the key is enough', () => {
    const b = granted(null, {
      repos: [
        { repo: 'acme/widgets', installation: '91', lastToken: token({ carried: { ...WITH_WORKFLOWS }, shortfall: {} }) },
        { repo: 'acme/gears', installation: '91', lastToken: token({ at: '2026-11-20T11:30:00Z' }) },
        { repo: 'acme/older', installation: '91', lastToken: token({ current: false }) },
      ],
    })
    expect(keyState(b, 'workflows', 'write')).toMatchObject({ kind: 'offToken', repo: 'acme/gears' })
    expect(keyState(b, 'contents', 'write')).toEqual({ kind: 'onToken', repo: 'acme/widgets', at: MINTED_AT })
  })
})

describe('the workspace-secret notice', () => {
  it('names the host and the secret', () => {
    expect(workspaceSecretNotice(block({ workspaceSecret: 'deploy-pat' }))).toBe(
      "Git at GitHub answers from the workspace secret deploy-pat for this agent, ahead of the App, so these permissions don't apply to it.",
    )
  })
})

describe('Check token', () => {
  const reply = (over: Partial<TokenCheck> = {}): TokenCheck => ({
    ok: true,
    source: 'git_host',
    secret: null,
    installation: '91',
    asked: { contents: 'write', metadata: 'read', pull_requests: 'write' },
    carried: CARRIED,
    shortfall: {},
    shortfallWhy: null,
    minted: true,
    error: null,
    reasons: [],
    at: MINTED_AT,
    ...over,
  })

  it('says what the token carries, and each reason in plain words', () => {
    const lines = checkSentences(
      reply({ reasons: [{ key: 'workflows', level: 'write', why: 'notInSet', detail: null }] }),
      'acme/widgets',
      'GitHub',
      'acme',
    )
    expect(lines).toEqual([
      'The token for acme/widgets carries contents: write, metadata: read, pull requests: write (minted now).',
      "workflows: write is not in this agent's set, though the installation holds it. Add it here to put it on the token.",
    ])
  })

  it('words every reason', () => {
    expect(reasonSentence({ key: 'workflows', level: 'write', why: 'installationLacks' }, 'acme')).toContain('installation on acme has not accepted it')
    expect(reasonSentence({ key: 'members', level: 'read', why: 'githubRefused', detail: 'not on a repository token' })).toBe(
      'members: read: GitHub refused it on this token: not on a repository token',
    )
    expect(reasonSentence({ key: 'workflows', level: 'write', why: 'githubOmitted' })).toBe('workflows: write: GitHub minted the token without it.')
  })

  it('is clean only with no error, no reason a key of the set is off, and the App answering', () => {
    expect(checkClean(reply())).toBe(true)
    // What the installation holds outside the set is not a fault.
    expect(checkClean(reply({ reasons: [{ key: 'workflows', level: 'write', why: 'notInSet' }] }))).toBe(true)
    expect(checkClean(reply({ reasons: [{ key: 'workflows', level: 'write', why: 'installationLacks' }] }))).toBe(false)
    expect(checkClean(reply({ ok: false, error: 'no' }))).toBe(false)
    expect(checkClean(reply({ source: 'workspace_secret', secret: 'deploy-pat' }))).toBe(false)
  })

  it("shows the error verbatim, and is never blank", () => {
    const error = 'the GitHub App on GitHub has no private key on record — connect it in Admin → Git hosts'
    expect(checkSentences(reply({ ok: false, carried: null, error, reasons: null }), 'acme/widgets', 'GitHub')).toEqual([error])
    expect(checkSentences({ ok: false }, 'acme/widgets', 'GitHub')).toEqual(['The check failed and gave no reason.'])
    expect(checkSentences({ ok: true }, 'acme/widgets', 'GitHub')).toHaveLength(1)
  })

  it('says what an installation holding everything holds outside the set once, as information, and keeps the token line green', () => {
    // The installation was given the organization's whole permission set;
    // the agent is on the default, and its token carries all of it.
    const outside = ['actions', 'administration', 'issues', 'workflows'].map((key) => ({ key, level: 'write' as const, why: 'notInSet', detail: null }))
    const lines = checkLines(reply({ reasons: outside }), 'acme/widgets', 'GitHub', 'acme')
    expect(lines).toEqual([
      { text: 'The token for acme/widgets carries contents: write, metadata: read, pull requests: write (minted now).', tone: 'success' },
      {
        text: "The installation also holds actions: write, administration: write, issues: write, workflows: write; none is in this agent's set. Add any of them here to put it on the token.",
        tone: 'neutral',
      },
    ])
    // A real gap stands out from them.
    const gap = checkLines(reply({ reasons: [...outside, { key: 'pull_requests', level: 'write', why: 'githubOmitted', detail: null }] }), 'acme/widgets', 'GitHub')
    expect(gap.map((l) => l.tone)).toEqual(['neutral', 'warn', 'neutral'])
    expect(gap[1]?.text).toBe('pull_requests: write: GitHub minted the token without it.')
  })

  it('says what GitHub added unasked on its own line, warn above read, naming a key the catalog does not know', () => {
    // An api that sends `added`: GitHub put checks: read on the token.
    const read = checkLines(reply({ carried: { ...CARRIED, checks: 'read' }, added: { checks: 'read' } }), 'acme/widgets', 'GitHub', 'acme')
    expect(read).toEqual([
      { text: 'The token for acme/widgets carries checks: read, contents: write, metadata: read, pull requests: write (minted now).', tone: 'success' },
      { text: "GitHub added checks: read, which this agent's set does not name. Talaria kept it and recorded it.", tone: 'neutral' },
    ])
    // Above read is said in the warning tone, and a key outside the catalog
    // is named all the same: the permission list draws catalog rows only.
    const wide = checkLines(
      reply({ carried: { ...CARRIED, administration: 'write', copilot_requests: 'write' }, added: { administration: 'write', copilot_requests: 'write' } }),
      'acme/widgets',
      'GitHub',
    )
    expect(wide[1]).toEqual({
      text: "GitHub added administration: write, copilot_requests: write, which this agent's set does not name. Talaria kept it and recorded it.",
      tone: 'warn',
    })
    // An api without the field: derived from what was carried beyond the ask.
    const derived = checkSentences(reply({ carried: { ...CARRIED, checks: 'read' } }), 'acme/widgets', 'GitHub')
    expect(derived[1]).toBe("GitHub added checks: read, which this agent's set does not name. Talaria kept it and recorded it.")
    // Nothing added, no line; and none for a refusal or a workspace secret.
    expect(checkSentences(reply(), 'acme/widgets', 'GitHub').some((l) => l.includes('GitHub added'))).toBe(false)
    expect(checkSentences(reply({ ok: false, error: 'no', added: { checks: 'read' } }), 'acme/widgets', 'GitHub').some((l) => l.includes('GitHub added'))).toBe(false)
  })

  it('drops the not-in-set reason for a key GitHub added: it is on the token', () => {
    const reasons = [
      { key: 'checks', level: 'read' as const, why: 'notInSet', detail: null },
      { key: 'actions', level: 'write' as const, why: 'notInSet', detail: null },
    ]
    const lines = checkSentences(reply({ carried: { ...CARRIED, checks: 'read' }, added: { checks: 'read' }, reasons }), 'acme/widgets', 'GitHub', 'acme')
    expect(lines).toEqual([
      'The token for acme/widgets carries checks: read, contents: write, metadata: read, pull requests: write (minted now).',
      "GitHub added checks: read, which this agent's set does not name. Talaria kept it and recorded it.",
      "actions: write is not in this agent's set, though the installation holds it. Add it here to put it on the token.",
    ])
    expect(lines.some((l) => l.startsWith('checks: read is not'))).toBe(false)
  })

  it('names the workspace secret that answers instead of the App', () => {
    expect(checkSentences(reply({ source: 'workspace_secret', secret: 'deploy-pat', carried: null }), 'acme/widgets', 'GitHub')).toEqual([
      "Git at GitHub answers from the workspace secret deploy-pat for this agent; these permissions don't apply to it.",
    ])
  })
})

describe('keyLines', () => {
  it('says each key once, keys that share a sentence on one line, in the set order', () => {
    const lines = keyLines(granted(token()), catalog, NOW)
    expect(lines.map((l) => l.keys.map((k) => k.key))).toEqual([['contents', 'metadata', 'pull_requests'], ['workflows']])
    expect(lines[0]?.sentence).toBe("On the agent's token (acme/widgets, 12 min ago)")
    expect(lines[1]?.state.kind).toBe('offToken')
  })

  it('carries the installations to accept on, from the token or the record', () => {
    const offered = keyLines(granted(token(), { installations: [acme], appPermissions: acme.appPermissions }), catalog, NOW)
    expect(offered.find((l) => l.keys.some((k) => k.key === 'workflows'))?.installations).toEqual([acme])
    const recorded = keyLines(granted(null, { installations: [acme], appPermissions: acme.appPermissions }), catalog, NOW)
    expect(recorded.find((l) => l.keys.some((k) => k.key === 'workflows'))?.installations).toEqual([acme])
    expect(recorded.find((l) => l.keys.some((k) => k.key === 'contents'))?.sentence).toBe('The installation holds it; no token checked yet')
  })
})
