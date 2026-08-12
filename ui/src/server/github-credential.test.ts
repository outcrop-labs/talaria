// THE CLONE URL STOPPED CARRYING A TOKEN, and these hold it that way.
//
// `start_job` returned `https://x-access-token:<token>@github.com/…` inside a
// tool RESULT — which is the model's context, the transcript, and the prompt of
// every later turn. The job instructions even said "the token is short-lived —
// clone now", an accurate description of a credential handed to a language
// model. The sandbox's git asks Talaria instead now, so the URL needs nothing.
//
// PAT MODE throughout, because that is the config whose token comes straight
// from settings — the App path adds a JWT exchange over the network, and the
// subject here is the SCOPING around the token rather than how it is minted.
import { describe, expect, it, vi } from 'vitest'

const granted: Record<string, string[]> = { 'agent-1': ['outcrop/talaria'], 'agent-2': [] }

const sql = ((strings: TemplateStringsArray, ...vals: unknown[]) => {
  const text = strings.join('?').replace(/\s+/g, ' ').trim().toLowerCase()
  if (text.startsWith('select repo from workbench_repos')) {
    return Promise.resolve((granted[String(vals[0])] ?? []).map((repo) => ({ repo })))
  }
  if (text.includes('from app_settings')) {
    return Promise.resolve([{ value: { mode: 'pat', pat: { tokenEnc: 'sealed-installation-token' } } }])
  }
  return Promise.resolve([])
}) as never

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/secretbox', () => ({ open: (t: string) => t.replace(/^sealed-/, ''), seal: (v: string) => `sealed-${v}` }))

const { agentGitCredential, cloneUrl } = await import('@/server/github')

describe('the clone url an agent is given', () => {
  it('carries no credential at all', async () => {
    const url = await cloneUrl('outcrop/talaria')
    expect(url).toBe('https://github.com/outcrop/talaria.git')
    expect(url).not.toContain('x-access-token')
    expect(url).not.toContain('installation-token')
  })
})

describe('git asking Talaria for a github credential', () => {
  it('answers for a repo on this agent grant list', async () => {
    expect(await agentGitCredential('agent-1', 'github.com', 'outcrop/talaria.git')).toEqual({
      username: 'x-access-token',
      password: 'installation-token',
      repo: 'outcrop/talaria',
    })
  })

  it('refuses a repo the agent was not granted', async () => {
    // WITHOUT THE PATH SCOPE this would hand every agent a token for every repo
    // the installation can reach — a worse deal than the clone URL it replaces.
    expect(await agentGitCredential('agent-1', 'github.com', 'someone/private.git')).toBeNull()
    expect(await agentGitCredential('agent-2', 'github.com', 'outcrop/talaria.git')).toBeNull()
  })

  it('refuses a path it cannot read as a repo, and a host that is not github', async () => {
    for (const path of [undefined, '', '/', 'nested/too/deep.git', 'noslash']) {
      expect(await agentGitCredential('agent-1', 'github.com', path)).toBeNull()
    }
    expect(await agentGitCredential('agent-1', 'evil.example', 'outcrop/talaria.git')).toBeNull()
  })

  it('tolerates the shapes git actually sends', async () => {
    expect((await agentGitCredential('agent-1', 'github.com', '/outcrop/talaria.git'))?.repo).toBe('outcrop/talaria')
    expect((await agentGitCredential('agent-1', 'GitHub.com', 'outcrop/talaria'))?.repo).toBe('outcrop/talaria')
  })
})
