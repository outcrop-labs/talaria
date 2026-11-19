// The permission-request section's words, pinned: an approval names every
// repo it applies to, says when GitHub still has to accept it, and a fact
// carries nothing a reader could not be shown.
import { describe, expect, it } from 'vitest'
import {
  approveConfirmMessage,
  askedLabel,
  evidenceLine,
  heldLabel,
  isDecidable,
  statusLabel,
  ticketHref,
  type DecidablePermissionRequest,
  type FactPermissionRequest,
} from './git-permission-requests'

const request = (over: Partial<DecidablePermissionRequest> = {}): DecidablePermissionRequest => ({
  id: 'r1',
  content: true,
  status: 'pending',
  host: 'github',
  hostName: 'GitHub',
  repo: 'acme/site',
  permission: 'workflows',
  level: 'write',
  agentId: 'a1',
  agentName: 'Billy',
  agentModel: 'dev-billy',
  why: 'the deploy workflow must change',
  evidence: [],
  held: true,
  installation: '91',
  installationUrl: 'https://github.com/organizations/acme/settings/installations/91',
  inSet: false,
  appliesTo: ['acme/api', 'acme/site'],
  ticket: null,
  decidedBy: null,
  decisionNote: null,
  createdAt: '2026-11-16T12:00:00.000Z',
  updatedAt: '2026-11-16T12:00:00.000Z',
  ...over,
})

describe('approveConfirmMessage', () => {
  it('names the agent, the permission, the host and every repo it applies to', () => {
    const text = approveConfirmMessage(request())
    expect(text).toContain('Give Billy workflows write on GitHub?')
    expect(text).toContain('acme/api, acme/site')
    expect(text).not.toContain('has not accepted')
  })

  it('says GitHub has to accept it first when the installation lacks it, and that it is unknown when unread', () => {
    expect(approveConfirmMessage(request({ held: false }))).toContain('has not accepted it yet')
    expect(approveConfirmMessage(request({ held: null }))).toContain('not known yet')
  })

  it('falls back to the repo it arose on when the agent holds none there', () => {
    expect(approveConfirmMessage(request({ appliesTo: [] }))).toContain('holds there: acme/site.')
  })
})

describe('the chips and lines', () => {
  it('says where a request stands and what GitHub holds', () => {
    expect(statusLabel('pending')).toBe('waiting on you')
    expect(statusLabel('awaiting_github')).toBe('approved, waiting on GitHub')
    expect(heldLabel(true)).toBe('the installation holds it')
    expect(heldLabel(false)).toBe('the installation has not accepted it')
    expect(heldLabel(null)).toContain('not known')
    expect(askedLabel(request())).toBe('workflows write')
  })

  it('labels the agent’s own report as unverified', () => {
    expect(evidenceLine({ source: 'agent', verified: false, repo: 'acme/site', at: '2026-11-16T12:34:56.000Z' })).toBe(
      'reported by the agent (unverified) on acme/site, 2026-11-16 12:34 UTC',
    )
    expect(evidenceLine({ source: 'agent', verified: false })).toBe('reported by the agent (unverified)')
  })

  it('links the ticket on its board', () => {
    expect(
      ticketHref({ id: 't1', ref: 'WEB-1', title: 'Fix CI', boardId: 'b1', status: 'blocked', statusLabel: 'Blocked', blocked: true, restartStatus: 'in_progress' }),
    ).toBe('/boards/b1/t1')
  })

  it('tells a fact from a request to decide', () => {
    const fact: FactPermissionRequest = { id: 'r2', content: false, status: 'pending', host: 'github', hostName: 'GitHub', createdAt: '' }
    expect(isDecidable(fact)).toBe(false)
    expect(isDecidable(request())).toBe(true)
  })
})
