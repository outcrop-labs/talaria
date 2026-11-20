// Admin → Git hosts' "Agents' tokens", pinned: what an installation accepted
// that no agent's set names is said as unclaimed, never as carried; each
// agent's last token says what GitHub put on it and what it did not; and a
// token minted for an earlier set is never drawn as this set's.
import { describe, expect, it } from 'vitest'
import type { LastToken } from './agent-github-permissions'
import { hostAgentRow, hostAgentRows, lastTokenLine, unclaimedKeys, unclaimedSentence, type HostAgent, type HostAgents } from './git-host-agents'

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
  it('REGRESSION: a key the installation holds that no agent set names is unclaimed, and on no agent as carried', () => {
    const keys = unclaimedKeys(wire)
    expect(keys).toEqual(['actions', 'workflows'])
    for (const row of hostAgentRows(wire, NOW)) {
      expect(row.carries?.map((e) => e.key)).not.toContain('workflows')
      expect(row.set.map((e) => e.key)).not.toContain('workflows')
    }
    expect(unclaimedSentence(keys)).toBe(
      'Accepted by the installation, carried by no agent: actions, workflows. An admin grants them per agent (Fleet → agent → Workbench → GitHub permissions).',
    )
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

describe('hostAgentRow', () => {
  it('says what a custom set names that the installation cannot carry', () => {
    const row = hostAgentRow(
      { ...developer, isDefault: false, set: { ...DEFAULT, workflows: 'write', members: 'read' }, carries: { ...DEFAULT, workflows: 'write' } },
      NOW,
    )
    expect(row.beyond).toEqual([{ key: 'members', level: 'read' }])
    expect(row.isDefault).toBe(false)
  })

  it('claims nothing about carries when the record does not say', () => {
    const row = hostAgentRow({ ...developer, carries: null }, NOW)
    expect(row.carries).toBeNull()
    expect(row.beyond).toEqual([])
  })

  it('lists Developer agents first, then by name', () => {
    const rows = hostAgentRows(
      {
        agents: [
          { ...developer, agentId: 'c', name: 'Zed' },
          { ...developer, agentId: 'b', name: 'Ops', developer: false },
          { ...developer, agentId: 'a', name: 'Ada' },
        ],
      },
      NOW,
    )
    expect(rows.map((r) => r.name)).toEqual(['Ada', 'Zed', 'Ops'])
  })
})
