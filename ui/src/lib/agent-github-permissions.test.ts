// The repo editor's GitHub permissions arithmetic, pinned: a key is measured
// against the installations that mint the agent's tokens, a key that is not
// held says why (the App does not ask, or an owner has not accepted), the add
// picker offers what rides at once first, and each edit writes the whole next
// set from what it read.
import { describe, expect, it } from 'vitest'
import {
  addOptions,
  fromOf,
  heldSentence,
  heldState,
  impactNote,
  mintingInstallations,
  setRows,
  withPermission,
  withoutPermission,
  type CatalogEntry,
  type HostPermissions,
  type PermissionsInstallation,
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
