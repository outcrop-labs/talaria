// The agents route as Agent policy › GitHub access and the Git hosts dialog
// read it, pinned: what an installation accepted that no agent's set names
// is said as granted to no agent, never as carried, with where an admin
// grants it; each agent's last token says what GitHub put on it, what it did
// not, and what it added unasked; and a token minted for an earlier set is
// never drawn as this set's.
import { describe, expect, it } from 'vitest'
import type { LastToken } from './agent-github-permissions'
import { lastTokenLine, unclaimedKeys, unclaimedSentence, type HostAgent, type HostAgents } from './git-host-agents'

const NOW = Date.parse('2026-11-20T12:00:00Z')
// The wire: `carried` is exactly a GitHub mint reply's `permissions` block
// for the default ask, and the shortfall is the key the reply lacks.
const CARRIED = JSON.parse('{"contents":"write","metadata":"read","pull_requests":"write"}')
const SHORTFALL = JSON.parse('{"workflows":"write"}')
const DEFAULT = { contents: 'write', metadata: 'read', pull_requests: 'write' } as const

const token = (over: Partial<LastToken> = {}): LastToken => ({
  at: '2026-11-20T11:48:00Z',
  ok: true,
  source: 'git_host',
  asked: DEFAULT,
  carried: CARRIED,
  shortfall: {},
  shortfallWhy: null,
  error: null,
  minted: true,
  current: true,
  repo: 'acme/widgets',
  ...over,
})

/** A Developer agent on the default, on a host whose installation accepted
 *  workflows and actions: what the route answers. */
const developer: HostAgent = {
  agentId: '0b6f1f0e-1d7e-4c51-9d0a-6f7f2b8a4a11',
  name: 'Builder',
  developer: true,
  isDefault: true,
  set: { ...DEFAULT },
  carries: { ...DEFAULT },
  lastToken: token(),
}
const wire: HostAgents = JSON.parse(JSON.stringify({ agents: [developer], unclaimed: { actions: 'write', workflows: 'write' } }))

describe('unclaimed', () => {
  // The server computes `unclaimed` (the agents route); this pins that the
  // readers say it as granted to no agent, and where an admin grants it.
  it('says a key the installation holds that no agent set names is granted to no agent, and names Agent policy › GitHub access', () => {
    const keys = unclaimedKeys(wire)
    expect(keys).toEqual(['actions', 'workflows'])
    expect(wire.agents[0]?.carries).not.toHaveProperty('workflows')
    expect(unclaimedSentence(keys)).toBe(
      'Accepted by the installation, granted to no agent: actions, workflows. An admin grants them per agent in Agent policy › GitHub access.',
    )
  })

  it("names the host in the overview's form, where several hosts may each have some", () => {
    expect(unclaimedSentence(['actions', 'workflows'], 'GitHub')).toBe(
      'Accepted by the installation on GitHub, granted to no agent: actions, workflows. Open an agent to grant them.',
    )
    expect(unclaimedSentence([], 'GitHub')).toBeNull()
  })

  it('reads a key list too, keeps the server order, and never names metadata', () => {
    expect(unclaimedKeys({ agents: [], unclaimed: ['workflows', 'actions', 'metadata'] })).toEqual(['workflows', 'actions'])
  })

  it('says nothing when everything is claimed, or the server does not say', () => {
    expect(unclaimedKeys({ agents: [], unclaimed: {} })).toEqual([])
    expect(unclaimedKeys({ agents: [] })).toEqual([])
    expect(unclaimedKeys(null)).toEqual([])
    expect(unclaimedSentence([])).toBeNull()
  })
})

describe('lastTokenLine', () => {
  it('says what the token carried, and that it is this set', () => {
    expect(lastTokenLine(token(), NOW)).toEqual({
      text: 'Last token (acme/widgets, 12 min ago): carried contents: write, metadata: read, pull requests: write',
      tone: 'success',
    })
  })

  it('says what a grant asked for that the token lacks', () => {
    const line = lastTokenLine(token({ asked: { ...DEFAULT, workflows: 'write' }, shortfall: SHORTFALL }), NOW)
    expect(line.text).toBe('Last token (acme/widgets, 12 min ago): carried contents: write, metadata: read, pull requests: write; not workflows: write')
    expect(line.tone).toBe('warn')
  })

  it('REGRESSION: a token minted for an earlier set says so, and is never green', () => {
    const line = lastTokenLine(token({ current: false }), NOW)
    expect(line.text).toContain('minted for an earlier set')
    expect(line.tone).not.toBe('success')
  })

  it('says a row from before rows said what was asked is that, not a token for an earlier set', () => {
    const line = lastTokenLine(token({ asked: undefined, current: false }), NOW)
    expect(line.text).toBe(
      'Last token (acme/widgets, 12 min ago, recorded before Talaria noted what it asked for): carried contents: write, metadata: read, pull requests: write',
    )
    expect(line.tone).toBe('neutral')
  })

  it('says what GitHub added unasked, from the server field or derived from carried beyond asked, metadata aside', () => {
    // An api that sends `added`.
    const sent = lastTokenLine(token({ carried: { ...CARRIED, checks: 'read' }, added: { checks: 'read' } }), NOW)
    expect(sent.text).toBe(
      'Last token (acme/widgets, 12 min ago): carried checks: read, contents: write, metadata: read, pull requests: write; GitHub added checks: read',
    )
    // An api that does not: what was carried and never asked for.
    const derived = lastTokenLine(token({ carried: { ...CARRIED, checks: 'read', issues: 'write' } }), NOW)
    expect(derived.text.endsWith('; GitHub added checks: read, issues: write')).toBe(true)
    // metadata: read rides on every token, asked or not; nothing added.
    expect(lastTokenLine(token({ asked: { contents: 'write', pull_requests: 'write' } }), NOW).text).not.toContain('GitHub added')
  })

  it("says a refusal in the door's words, a workspace secret by name, and none at all", () => {
    expect(lastTokenLine(token({ ok: false, carried: null, error: 'GitHub refused the token: 422' }), NOW)).toEqual({
      text: 'Refused (acme/widgets, 12 min ago): GitHub refused the token: 422',
      tone: 'warn',
    })
    expect(lastTokenLine(token({ source: 'workspace_secret', secret: 'deploy-pat', carried: null }), NOW).text).toBe(
      'A workspace secret answered (acme/widgets, 12 min ago): deploy-pat',
    )
    expect(lastTokenLine(null, NOW)).toEqual({ text: 'No token yet', tone: 'neutral' })
  })
})
