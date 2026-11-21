import { afterEach, describe, expect, it, vi } from 'vitest'
import { HttpError } from '@/lib/fetch-json'
import { CHANGED_BODY, changedTitle, grantChanges, normalizedRule, readOf, saveRepoGrants, type GrantsRead, type RepoRule } from './repo-grants'

// The repositories section's write is a compare-and-set: it sends back what
// it read, and a 409 brings what is there now. These pin the request's
// shape, the conflict path the editor shows, and the words that say what
// another editor changed.

const rule = (repo: string, over: Partial<RepoRule> = {}): RepoRule => ({
  host: 'github',
  repo,
  baseBranch: null,
  pushMode: 'branches_only',
  branchPrefix: null,
  ...over,
})
const grant = (repo: string, host = 'github') => ({ host, repo })

/** What the editor read: two repos, one with a prefix. */
const read: GrantsRead = {
  repos: [grant('acme/site'), grant('acme/app')],
  rules: [rule('acme/site', { branchPrefix: 'agent' }), rule('acme/app')],
}

describe('saveRepoGrants', () => {
  const sent: Array<{ url: string; init: RequestInit }> = []
  const reply = (status: number, body: unknown) =>
    vi.fn(async (url: string, init: RequestInit) => {
      sent.push({ url, init })
      return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })
    })

  afterEach(() => {
    sent.length = 0
    vi.unstubAllGlobals()
  })

  it('sends what the editor read as `from`, beside the whole next set', async () => {
    vi.stubGlobal('fetch', reply(200, { granted: [grant('acme/site')] }))
    const out = await saveRepoGrants('agent-1', read, { repos: [grant('acme/site')], rules: [rule('acme/site', { pushMode: 'free' })] })
    expect(out).toEqual({ kind: 'saved', granted: [grant('acme/site')] })
    expect(sent).toHaveLength(1)
    expect(sent[0].url).toBe('/api/workbench/repos/agent-1')
    expect(sent[0].init.method).toBe('PUT')
    expect(JSON.parse(String(sent[0].init.body))).toEqual({
      repos: [grant('acme/site')],
      rules: [rule('acme/site', { pushMode: 'free' })],
      from: read,
    })
  })

  it('reads the GET into the `from` it sends, untouched', () => {
    const body = { hosts: [], available: [], granted: read.repos, rules: read.rules, branches: {} }
    expect(readOf(body)).toEqual(read)
    // A server from before rules rode the GET reads as none.
    expect(readOf({ granted: [] })).toEqual({ repos: [], rules: [] })
  })

  it('turns a 409 into what is there now and the words for what changed, writing nothing', async () => {
    // Another manager removed acme/app and set a prefix on acme/site; an
    // approval granted acme/docs.
    const current: GrantsRead = {
      repos: [grant('acme/docs'), grant('acme/site')],
      rules: [rule('acme/docs'), rule('acme/site', { branchPrefix: 'theirs' })],
    }
    vi.stubGlobal('fetch', reply(409, { error: "this agent's repositories or branch rules changed since you read them — read them again", current }))
    const out = await saveRepoGrants('agent-1', read, { repos: [...read.repos, grant('acme/new')], rules: read.rules })
    expect(out).toEqual({
      kind: 'changed',
      current,
      title: 'These repositories changed since you read them: acme/docs was added, acme/app was removed and the branch rules of acme/site changed',
      body: 'Showing what is set now. Make your change again if it still applies.',
    })
    expect(sent).toHaveLength(1)
  })

  it('throws any other refusal with the server sentence', async () => {
    vi.stubGlobal('fetch', reply(403, { error: "only this agent's managers can change it" }))
    const err = await saveRepoGrants('agent-1', read, { repos: [], rules: [] }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(HttpError)
    expect((err as HttpError).status).toBe(403)
    expect((err as HttpError).message).toBe("only this agent's managers can change it")
  })
})

describe('what changed since the read', () => {
  it('says nothing moved when only the spelling of a rule differs', () => {
    const stored: GrantsRead = {
      repos: [grant('acme/app'), grant('acme/site')],
      rules: [rule('acme/site', { branchPrefix: '/agent/', baseBranch: '' })],
    }
    const typed: GrantsRead = { repos: read.repos, rules: [rule('acme/site', { branchPrefix: 'agent' })] }
    expect(grantChanges(typed, stored)).toEqual([])
    expect(changedTitle(typed, stored)).toBe('These repositories changed since you read them')
    expect(CHANGED_BODY).toBe('Showing what is set now. Make your change again if it still applies.')
    expect(normalizedRule(rule('x', { pushMode: 'yolo' as RepoRule['pushMode'], branchPrefix: ' / ' }))).toEqual(rule('x'))
  })

  it('names each kind once, counts past three, and names hosts when there are several', () => {
    const many: GrantsRead = { repos: ['a', 'b', 'c', 'd', 'e'].map((n) => grant(`acme/${n}`)), rules: [] }
    expect(grantChanges({ repos: [], rules: [] }, many)).toEqual(['acme/a, acme/b, acme/c and 2 more were added'])
    expect(grantChanges(many, { repos: [], rules: [] })).toEqual(['acme/a, acme/b, acme/c and 2 more were removed'])
    expect(grantChanges({ repos: [], rules: [] }, { repos: [grant('acme/a'), grant('acme/b')], rules: [] })).toEqual(['acme/a and acme/b were added'])

    // The same name on another host is another repository, and says so.
    const forge: GrantsRead = { repos: [grant('acme/notes', 'forge')], rules: [] }
    const both: GrantsRead = { repos: [grant('acme/notes', 'forge'), grant('acme/notes')], rules: [rule('acme/notes', { pushMode: 'free' })] }
    const name = (id: string) => (id === 'forge' ? 'Forgejo' : 'GitHub')
    expect(grantChanges(forge, both, name)).toEqual(['acme/notes on GitHub was added'])
    expect(changedTitle(both, forge, name)).toBe('These repositories changed since you read them: acme/notes on GitHub was removed')
    // A rule moved on a repo both reads hold, from the defaults it had.
    const ruled: GrantsRead = { repos: forge.repos, rules: [rule('acme/notes', { host: 'forge', baseBranch: 'develop' })] }
    expect(grantChanges(forge, ruled, name)).toEqual(['the branch rules of acme/notes changed'])
  })
})
