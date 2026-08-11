// AN AGENT USES A CREDENTIAL WITHOUT EVER READING IT.
//
// `secret-vault.ts` stops a credential in context reaching a provider. This is
// the other half — the value is substituted at the boundary that spends it — and
// every property below is one an operator would want to be able to state plainly
// to a customer asking where their keys go.
import { describe, expect, it, vi } from 'vitest'

// A tiny in-memory Postgres stand-in: enough shape for the three tables, so the
// subject stays the authorization and lifetime rules rather than SQL.
interface Row {
  [k: string]: unknown
}
const tables = { workspace_secrets: [] as Row[], workspace_secret_entries: [] as Row[], workspace_secret_grants: [] as Row[] }
let seq = 0

const sql = ((strings: TemplateStringsArray, ...vals: unknown[]) => {
  const text = strings.join('?').replace(/\s+/g, ' ').trim().toLowerCase()
  const v = vals
  if (text.startsWith('insert into workspace_secrets ')) {
    const id = `sec-${++seq}`
    tables.workspace_secrets.push({
      id,
      name: v[0],
      title: v[1],
      kind: v[2],
      note: v[3],
      created_by: v[4],
      expires_at: v[5],
      uses_remaining: v[6],
      created_at: new Date().toISOString(),
      last_used_at: null,
    })
    return Promise.resolve([{ id }])
  }
  if (text.startsWith('insert into workspace_secret_entries')) {
    tables.workspace_secret_entries.push({ secret_id: v[0], key: v[1], label: v[2], value_cipher: v[3] })
    return Promise.resolve([])
  }
  if (text.startsWith('insert into workspace_secret_grants (secret_id, agent_model, granted_by) values')) {
    tables.workspace_secret_grants.push({ secret_id: v[0], agent_model: v[1] })
    return Promise.resolve([])
  }
  if (text.startsWith('insert into workspace_secret_grants (secret_id, agent_model, granted_by) select')) {
    const doc = tables.workspace_secrets.find((d) => d.name === v[2])
    if (doc) tables.workspace_secret_grants.push({ secret_id: doc.id, agent_model: v[0] })
    return Promise.resolve([])
  }
  if (text.startsWith('select id, name, title, kind')) {
    return Promise.resolve(
      tables.workspace_secrets
        .filter((d) => d.name === v[0])
        .map((d) => ({
          id: d.id,
          name: d.name,
          title: d.title,
          kind: d.kind,
          note: d.note,
          createdBy: d.created_by,
          createdAt: d.created_at,
          expiresAt: d.expires_at,
          usesRemaining: d.uses_remaining,
          lastUsedAt: d.last_used_at,
        })),
    )
  }
  if (text.startsWith('select key, label from workspace_secret_entries')) {
    return Promise.resolve(tables.workspace_secret_entries.filter((e) => e.secret_id === v[0]).map((e) => ({ key: e.key, label: e.label })))
  }
  if (text.startsWith('select agent_model from workspace_secret_grants')) {
    return Promise.resolve(tables.workspace_secret_grants.filter((g) => g.secret_id === v[0]))
  }
  if (text.startsWith('select s.id, s.name, s.kind')) {
    return Promise.resolve(
      tables.workspace_secrets
        .filter((d) => String(d.name).toLowerCase() === v[1])
        .map((d) => ({
          id: d.id,
          name: d.name,
          kind: d.kind,
          expiresAt: d.expires_at,
          usesRemaining: d.uses_remaining,
          granted: String(tables.workspace_secret_grants.filter((g) => g.secret_id === d.id && g.agent_model === v[0]).length),
        })),
    )
  }
  if (text.startsWith('select key, label, value_cipher')) {
    return Promise.resolve(tables.workspace_secret_entries.filter((e) => e.secret_id === v[0]).map((e) => ({ key: e.key, label: e.label, cipher: e.value_cipher })))
  }
  if (text.startsWith('update workspace_secrets set uses_remaining')) {
    const doc = tables.workspace_secrets.find((d) => d.id === v[0] && Number(d.uses_remaining) > 0)
    if (!doc) return Promise.resolve([])
    doc.uses_remaining = Number(doc.uses_remaining) - 1
    return Promise.resolve([{ id: doc.id }])
  }
  if (text.startsWith('update workspace_secrets set last_used_at')) return Promise.resolve([])
  if (text.startsWith('select s.name, e.key, e.label')) {
    const out: Row[] = []
    for (const g of tables.workspace_secret_grants.filter((x) => x.agent_model === v[0])) {
      const doc = tables.workspace_secrets.find((d) => d.id === g.secret_id)
      if (!doc) continue
      if (doc.expires_at && new Date(String(doc.expires_at)).getTime() <= Date.now()) continue
      if (doc.uses_remaining !== null && Number(doc.uses_remaining) <= 0) continue
      for (const e of tables.workspace_secret_entries.filter((x) => x.secret_id === doc.id)) out.push({ name: doc.name, key: e.key, label: e.label })
    }
    return Promise.resolve(out)
  }
  return Promise.resolve([])
}) as never

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
// The envelope is exercised by its own tests; here it only has to round-trip.
vi.mock('@/server/secretbox', () => ({ seal: (v: string) => `sealed:${v}`, open: (t: string) => t.replace(/^sealed:/, '') }))

const { createSecretDoc, grantedHandlesFor, grantSecret, resolveHandles } = await import('@/server/workspace-secrets')

const PAT = 'github_pat_11ABCDEFG0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'

describe('using a secret without reading it', () => {
  it('substitutes the value at the boundary for a granted caller', async () => {
    await createSecretDoc({ name: 'deploy', title: 'Deploy', entries: [{ key: 'github_pat', label: 'GitHub token', value: PAT }], grantTo: ['dex-developer'] })
    const out = await resolveHandles('git push https://«secret:deploy.github_pat»@github.com/org/repo', 'dex-developer')
    expect(out.text).toContain(PAT)
    // And the audit line names the doc, the key and the KIND — never the value.
    expect(out.used).toEqual([{ name: 'deploy', key: 'github_pat', label: 'GitHub token' }])
    expect(JSON.stringify(out.used)).not.toContain(PAT)
  })

  it('resolves the doc-only form when there is exactly one entry', async () => {
    // What makes a single secret feel single while sharing the bundle's
    // machinery.
    const out = await resolveHandles('token «secret:deploy»', 'dex-developer')
    expect(out.text).toContain(PAT)
  })

  it('refuses a caller with no grant, and says so to the OPERATOR only', async () => {
    // A caller that learns which names exist has been handed a map of the
    // workspace's credentials, so the model gets an unresolved handle and the
    // reason goes to the log.
    const out = await resolveHandles('«secret:deploy.github_pat»', 'penny-assistant')
    expect(out.text).not.toContain(PAT)
    expect(out.text).toContain('«secret:deploy.github_pat»')
    expect(out.unresolved).toEqual([{ handle: '«secret:deploy.github_pat»', reason: 'not-granted' }])
  })

  it('resolves nothing for a handle nobody minted', async () => {
    const out = await resolveHandles('«secret:not-a-thing»', 'dex-developer')
    expect(out.unresolved[0]?.reason).toBe('unknown')
  })

  it('will not guess which of several credentials was meant', async () => {
    // Spending the wrong one of four is worse than refusing.
    await createSecretDoc({
      name: 'registry',
      title: 'Registry',
      entries: [
        { key: 'user', label: 'Username', value: 'ci' },
        { key: 'password', label: 'Registry password', value: 'hunter2' },
      ],
      grantTo: ['dex-developer'],
    })
    const out = await resolveHandles('«secret:registry»', 'dex-developer')
    expect(out.unresolved[0]?.reason).toBe('ambiguous')
    expect(out.text).not.toContain('hunter2')
  })
})

describe('a relay is spent once', () => {
  it('resolves the first time and never again', async () => {
    // Somebody pastes a credential into chat so an agent can do ONE thing with
    // it. A relay that outlived its errand would be a durable secret nobody
    // remembers creating.
    await createSecretDoc({ name: 'oneshot', title: 'One-shot', kind: 'relay', entries: [{ key: 'k', label: 'API key', value: 'sk-live-once' }], grantTo: ['dex-developer'] })
    expect((await resolveHandles('«secret:oneshot»', 'dex-developer')).text).toContain('sk-live-once')
    const again = await resolveHandles('«secret:oneshot»', 'dex-developer')
    expect(again.text).not.toContain('sk-live-once')
    expect(again.unresolved[0]?.reason).toBe('spent')
  })
})

describe('what an agent is told it has', () => {
  it('names the handles and the kinds, and no values', async () => {
    const told = await grantedHandlesFor('dex-developer')
    expect(told).toContain('«secret:deploy»')
    expect(told).toContain('GitHub token')
    expect(told).toContain('«secret:registry.password»')
    expect(told).not.toContain(PAT)
    expect(told).not.toContain('hunter2')
    // A spent relay is not offered.
    expect(told).not.toContain('«secret:oneshot»')
  })

  it('tells an agent with no grants nothing at all', async () => {
    await grantSecret('deploy', 'other-agent')
    expect(await grantedHandlesFor('nobody-agent')).toBe('')
  })
})
