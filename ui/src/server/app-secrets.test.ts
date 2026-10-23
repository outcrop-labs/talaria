// ctx.secret(name) — an app's server reading a workspace secret
// (workspace-secrets.ts `resolveAppSecret`). The one rule: an admin gave the
// secret to this app by listing `talaria-app://<slug>` in its allowed hosts,
// and it is a workspace vault doc that has not expired. Everything else is
// null to the app, with the reason for the operator.
import { beforeEach, describe, expect, it, vi } from 'vitest'

interface Doc {
  id: string
  name: string
  kind: 'vault' | 'relay'
  expiresAt: string | null
  usesRemaining: number | null
  allowedHosts: string[] | null
  ownerUserId: string | null
  entries: Array<{ key: string; label: string; cipher: string }>
}
let docs: Doc[] = []
const audits: Array<Record<string, unknown>> = []

const sql = (strings: TemplateStringsArray, ...v: unknown[]) => {
  const text = strings.join('?').replace(/\s+/g, ' ').trim().toLowerCase()
  if (text.startsWith('select id, name, kind')) {
    return Promise.resolve(docs.filter((d) => d.name.toLowerCase() === v[0]).map(({ entries: _e, ...d }) => d))
  }
  if (text.startsWith('select key, label, value_cipher')) {
    return Promise.resolve(docs.find((d) => d.id === v[0])?.entries ?? [])
  }
  if (text.startsWith('update workspace_secrets set uses_remaining')) {
    const d = docs.find((x) => x.id === v[0])
    if (!d || d.usesRemaining === null || d.usesRemaining <= 0) return Promise.resolve([])
    d.usesRemaining -= 1
    return Promise.resolve([{ id: d.id }])
  }
  if (text.startsWith('update workspace_secrets set last_used_at')) return Promise.resolve([])
  if (text.startsWith("update workspace_secret_entries set value_cipher = ''")) {
    for (const e of docs.find((x) => x.id === v[0])?.entries ?? []) e.cipher = ''
    return Promise.resolve([])
  }
  throw new Error(`unexpected query: ${text}`)
}
vi.mock('./db/pg', () => ({ db: async () => sql }))
vi.mock('./secretbox', () => ({
  seal: (v: string) => `sealed:${v}`,
  open: (c: string) => {
    if (!c.startsWith('sealed:')) throw new Error('cannot open')
    return c.slice('sealed:'.length)
  },
}))
vi.mock('./audit', () => ({ logAudit: async (e: Record<string, unknown>) => void audits.push(e) }))

const { resolveAppSecret, appDestination } = await import('./workspace-secrets')

const doc = (over: Partial<Doc> = {}): Doc => ({
  id: `sec-${docs.length + 1}`,
  name: 'crm',
  kind: 'vault',
  expiresAt: null,
  usesRemaining: null,
  allowedHosts: ['api.crm.example', 'talaria-app://crm'],
  ownerUserId: null,
  entries: [{ key: 'api_key', label: 'CRM API key', cipher: 'sealed:sk-live-1' }],
  ...over,
})

describe('resolveAppSecret (ctx.secret)', () => {
  beforeEach(() => {
    docs = []
    audits.length = 0
  })

  it('resolves a workspace vault secret whose allowed hosts name this app, and audits the read', async () => {
    docs.push(doc())
    expect(await resolveAppSecret('crm', 'crm', 'u1')).toEqual({ ok: true, value: 'sk-live-1' })
    expect(await resolveAppSecret('crm', 'CRM.api_key', 'u1')).toEqual({ ok: true, value: 'sk-live-1' })
    expect(audits).toHaveLength(2)
    expect(audits[0]).toMatchObject({
      actor: 'app:crm',
      action: 'secrets.spend',
      targetId: 'crm',
      targetLabel: 'CRM API key',
      after: { key: 'api_key', destination: 'talaria-app://crm', userId: 'u1' },
    })
    // The value is never in the audit row.
    expect(JSON.stringify(audits)).not.toContain('sk-live-1')
  })

  it('is null to every other app, and to an unrestricted secret', async () => {
    docs.push(doc({ name: 'crm' }), doc({ name: 'open', allowedHosts: [] }), doc({ name: 'nulls', allowedHosts: null }))
    expect(await resolveAppSecret('billing', 'crm', 'u1')).toEqual({ ok: false, reason: 'not-for-this-app' })
    // An allowlist that names the app's host or a lookalike is not the token.
    docs.push(doc({ name: 'near', allowedHosts: ['crm', 'talaria-app://crm2', 'app://crm', 'talaria-app://crm.evil'] }))
    expect(await resolveAppSecret('crm', 'near', 'u1')).toEqual({ ok: false, reason: 'not-for-this-app' })
    // Unrestricted is what agents' per-agent grants gate; an app has no agent.
    expect(await resolveAppSecret('crm', 'open', 'u1')).toEqual({ ok: false, reason: 'not-for-this-app' })
    expect(await resolveAppSecret('crm', 'nulls', 'u1')).toEqual({ ok: false, reason: 'not-for-this-app' })
    expect(audits).toHaveLength(0)
  })

  it('matches the token case-insensitively and trimmed, as the allowlist is stored', async () => {
    docs.push(doc({ allowedHosts: ['  Talaria-App://CRM '] }))
    expect(await resolveAppSecret('crm', 'crm', 'u1')).toEqual({ ok: true, value: 'sk-live-1' })
    expect(appDestination('crm')).toBe('talaria-app://crm')
  })

  it('refuses a person\'s working secret, a relay and an expired secret', async () => {
    docs.push(
      doc({ name: 'mine', ownerUserId: 'u2' }),
      doc({ name: 'paste', kind: 'relay', usesRemaining: 1 }),
      doc({ name: 'old', expiresAt: new Date(Date.now() - 1000).toISOString() }),
    )
    expect(await resolveAppSecret('crm', 'mine', 'u2')).toEqual({ ok: false, reason: 'not-workspace' })
    expect(await resolveAppSecret('crm', 'paste', 'u1')).toEqual({ ok: false, reason: 'not-vault' })
    expect(await resolveAppSecret('crm', 'old', 'u1')).toEqual({ ok: false, reason: 'expired' })
    // The relay was not spent by the refusal.
    expect(docs[1]!.usesRemaining).toBe(1)
  })

  it('names an entry, or the only one; never guesses among several', async () => {
    docs.push(
      doc({
        name: 'deploy',
        entries: [
          { key: 'pat', label: 'GitHub PAT', cipher: 'sealed:ghp-1' },
          { key: 'registry', label: 'Registry password', cipher: 'sealed:reg-1' },
        ],
      }),
    )
    expect(await resolveAppSecret('crm', 'deploy', 'u1')).toEqual({ ok: false, reason: 'ambiguous' })
    expect(await resolveAppSecret('crm', 'deploy.registry', 'u1')).toEqual({ ok: true, value: 'reg-1' })
    expect(await resolveAppSecret('crm', 'deploy.nope', 'u1')).toEqual({ ok: false, reason: 'unknown' })
    expect(await resolveAppSecret('crm', 'nothing', 'u1')).toEqual({ ok: false, reason: 'unknown' })
  })

  it('refuses a name that is not the handle grammar, without a query', async () => {
    for (const bad of ['', '«secret:crm»', 'crm.api.key', '../crm', 'crm key', ' .crm']) {
      expect(await resolveAppSecret('crm', bad, 'u1')).toEqual({ ok: false, reason: 'bad-name' })
    }
    expect(await resolveAppSecret('crm', 7 as unknown as string, 'u1')).toEqual({ ok: false, reason: 'bad-name' })
  })

  it('spends a use limit, and destroys the value with the last use', async () => {
    docs.push(doc({ usesRemaining: 2 }))
    expect((await resolveAppSecret('crm', 'crm', 'u1')).ok).toBe(true)
    expect((await resolveAppSecret('crm', 'crm', 'u1')).ok).toBe(true)
    expect(docs[0]!.entries[0]!.cipher).toBe('')
    expect(await resolveAppSecret('crm', 'crm', 'u1')).toMatchObject({ ok: false })
    expect(audits.map((a) => (a.after as { exhausted: boolean }).exhausted)).toEqual([false, true])
  })
})
