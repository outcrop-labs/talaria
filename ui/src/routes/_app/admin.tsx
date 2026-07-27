import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { confirm } from '@/components/ui/confirm'
import { Combobox } from '@/components/ui/combobox'
import { Button, buttonClasses } from '@/components/ui/button'
import { Generating } from '@/components/ui/generating'
import { Input } from '@/components/ui/input'
import { submitOnEnter } from '@/components/ui/control'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { Select } from '@/components/ui/select'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { relativeTime } from '@/lib/fleet'
import { GATEABLE_VIEWS } from '@/lib/nav'
import { RetrievalPanel } from '@/components/admin/retrieval-panel'
import { StoragePanel } from '@/components/admin/storage-panel'

export const Route = createFileRoute('/_app/admin')({
  component: AdminPage,
})

interface AdminUser {
  id: string
  email: string | null
  name: string | null
  role: 'admin' | 'member'
  lastSeenAt: string
  createdAt: string
  agentModels: string[]
  canMintKeys: boolean
  deniedViews: string[]
  pinnedAdmin: boolean
  assistantModel: string | null
  assistantElevated: boolean
}

function useAdminUsers() {
  return useQuery({
    queryKey: ['admin-users'],
    queryFn: async (): Promise<AdminUser[] | null> => {
      const r = await fetch('/api/admin/users', { credentials: 'same-origin' })
      if (!r.ok) return null
      return ((await r.json()) as { users: AdminUser[] }).users
    },
  })
}

// The admin console: people, their roles, and which agents each may use.
function AdminPage() {
  const qc = useQueryClient()
  const { data: me } = useSession()
  const { data: users } = useAdminUsers()
  const { data: fleet } = useAgents()
  const agentOptions = (fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))
  const [error, setError] = useState<string | null>(null)

  const update = async (userId: string, patch: { role?: 'admin' | 'member'; agentModels?: string[]; canMintKeys?: boolean; deniedViews?: string[]; assistantElevated?: boolean }) => {
    setError(null)
    const r = await fetch('/api/admin/users', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ userId, ...patch }),
    })
    if (!r.ok) setError(((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'update failed')
    await qc.invalidateQueries({ queryKey: ['admin-users'] })
  }

  if (me && me.role !== 'admin') {
    return <EmptyState icon="⛨" title="Admins only" hint="Ask an admin if you need access here." />
  }

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-4xl space-y-8">
        <h1 className="mercury-text text-2xl font-semibold">Admin</h1>

        <OrgPanel />

        <SettingsPanel />

        <JudgePanel />

        <GuardrailsPanel />

        <OutreachPanel />

        <OrgGooglePanel />

        <RetrievalPanel />

        <StoragePanel />

        <EncryptionPanel />

        <Panel>
          <div className="mb-2 text-sm font-semibold text-fg">People</div>
          <p className="mb-4 text-xs text-muted">
            Roles, per-person agent access, and which views each member can reach. Empty = all (open by
            default); pick any to restrict. Admins always have full access.
          </p>
          {error && (
            <div className="mb-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
              {error}
            </div>
          )}
          <ul className="divide-y divide-line-subtle">
            {(users ?? []).map((u) => {
              // Views shown as an ALLOW list (all selected by default); denying =
              // stored as gateable-minus-allowed.
              const allowedViews = GATEABLE_VIEWS.filter((v) => !u.deniedViews.includes(v.to)).map((v) => v.to)
              return (
                <li key={u.id} className="space-y-2 py-3">
                  <div className="flex items-center gap-3">
                    <Avatar name={u.name ?? u.email} className="h-7 w-7" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm text-fg">{u.name ?? u.email ?? u.id}</span>
                      <span className="block truncate text-xs text-muted">
                        {u.name && u.email ? u.email : `seen ${relativeTime(u.lastSeenAt)}`}
                      </span>
                    </span>
                    <Select
                      value={u.role}
                      size="sm"
                      disabled={u.pinnedAdmin || u.id === me?.id}
                      title={u.pinnedAdmin ? 'Pinned admin via AUTH_ADMIN_EMAILS' : u.id === me?.id ? 'You cannot demote yourself' : undefined}
                      onChange={(e) => void update(u.id, { role: e.target.value as 'admin' | 'member' })}
                      className="w-28 shrink-0"
                    >
                      <option value="member">Member</option>
                      <option value="admin">Admin</option>
                    </Select>
                    <label
                      className="flex shrink-0 items-center gap-1.5 text-xs text-muted"
                      title="May create API keys for the Talaria LLM gateway (Settings → API keys)"
                    >
                      <input
                        type="checkbox"
                        checked={u.role === 'admin' || u.canMintKeys}
                        disabled={u.role === 'admin'}
                        onChange={(e) => void update(u.id, { canMintKeys: e.target.checked })}
                        className="accent-[var(--theme-accent)]"
                      />
                      keys
                    </label>
                    {u.role === 'admin' && u.assistantModel && (
                      <label
                        className="flex shrink-0 items-center gap-1.5 text-xs text-muted"
                        title={`Give ${u.assistantModel} org-wide view/edit: every board, every channel and relay (never DMs), and editor rights on all org-visible knowledge and artifacts. Only while this user is an admin.`}
                      >
                        <input
                          type="checkbox"
                          checked={u.assistantElevated}
                          onChange={(e) => void update(u.id, { assistantElevated: e.target.checked })}
                          className="accent-[var(--theme-accent)]"
                        />
                        elevated assistant
                      </label>
                    )}
                    <span className="w-16 shrink-0 text-right text-xs text-muted">{relativeTime(u.lastSeenAt)}</span>
                  </div>
                  {u.role !== 'admin' && (
                    <div className="flex items-center gap-2 pl-10">
                      <Combobox
                        options={agentOptions}
                        selected={u.agentModels}
                        onChange={(models) => void update(u.id, { agentModels: models })}
                        multiple
                        size="sm"
                        placeholder="All agents"
                        className="min-w-0 flex-1"
                      />
                      <Combobox
                        options={GATEABLE_VIEWS.map((v) => ({ value: v.to, label: v.label }))}
                        selected={allowedViews}
                        onChange={(views) =>
                          void update(u.id, { deniedViews: GATEABLE_VIEWS.filter((v) => !views.includes(v.to)).map((v) => v.to) })
                        }
                        multiple
                        size="sm"
                        placeholder="All views"
                        className="min-w-0 flex-1"
                      />
                    </div>
                  )}
                </li>
              )
            })}
          </ul>
        </Panel>
      </div>
    </div>
  )
}

// App-wide settings (grows over time). Audit retention is the first.
// The business every agent works for. Woven automatically into agent design
// (muse-generated souls anchor to this team) and every rendered SOUL.md — so
// no agent introduces itself as belonging to the underlying platform.
function OrgPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['admin-settings'],
    queryFn: async (): Promise<{ auditRetentionDays: number; org: { name: string; about: string } }> => {
      const r = await fetch('/api/admin/settings')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const [name, setName] = useState<string | null>(null)
  const [about, setAbout] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  const nameVal = name ?? data?.org.name ?? ''
  const aboutVal = about ?? data?.org.about ?? ''
  const dirty = nameVal !== (data?.org.name ?? '') || aboutVal !== (data?.org.about ?? '')

  const save = async () => {
    await fetch('/api/admin/settings', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ org: { name: nameVal, about: aboutVal } }),
    })
    setName(null)
    setAbout(null)
    setSaved(true)
    setTimeout(() => setSaved(false), 1500)
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }

  return (
    <Panel>
      <div className="mb-1 text-sm font-semibold text-fg">Organization</div>
      <p className="mb-3 text-xs text-muted">
        The business your agents work for. Baked into every agent's identity automatically. Generated souls anchor to
        this team, and saving here rolls running agents (a fresh container comes up and traffic cuts over only once
        it's healthy), so the fleet speaks the new identity without interrupting anyone's conversation.
      </p>
      <div className="space-y-3">
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Business name</label>
          <Input size="sm" value={nameVal} onChange={(e) => setName(e.target.value)} placeholder="e.g. Outcrop Labs" className="max-w-xs" />
        </div>
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">What the business does</label>
          <Textarea
            rows={2}
            value={aboutVal}
            onChange={(e) => setAbout(e.target.value)}
            placeholder="One or two sentences agents can anchor their mission to."
            className="w-full text-sm"
          />
        </div>
        <div className="flex items-center gap-2">
          {saved && <span className="text-xs text-[color:var(--theme-success)]">Saved</span>}
          <span className="ml-auto" />
          <Button size="sm" onClick={() => void save()} disabled={!dirty}>
            Save
          </Button>
        </div>
      </div>
    </Panel>
  )
}

function SettingsPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['admin-settings'],
    queryFn: async (): Promise<{ auditRetentionDays: number }> => {
      const r = await fetch('/api/admin/settings')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const [days, setDays] = useState('')
  const value = days !== '' ? days : String(data?.auditRetentionDays ?? '')
  const save = async () => {
    const n = Number(value)
    if (!Number.isFinite(n)) return
    await fetch('/api/admin/settings', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ auditRetentionDays: n }),
    })
    setDays('')
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }
  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Settings</div>
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="text-sm text-fg">Audit retention</div>
          <div className="text-xs text-muted">How many days to keep the audit log. 0 = keep forever.</div>
        </div>
        <Input
          size="sm"
          type="number"
          value={value}
          onChange={(e) => setDays(e.target.value)}
          onKeyDown={submitOnEnter(() => days !== '' && Number(days) !== data?.auditRetentionDays && void save())}
          className="w-24 shrink-0"
        />
        <span className="shrink-0 text-xs text-muted">days</span>
        <Button size="sm" className="shrink-0" onClick={() => void save()} disabled={days === '' || Number(days) === data?.auditRetentionDays}>
          Save
        </Button>
      </div>
    </Panel>
  )
}

// The automated QA judge — an advisory reliability gate. When a ticket hits
// quality_review, a judge model reviews the agent's work and posts a verdict.
function EncryptionPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['admin-encryption'],
    queryFn: async (): Promise<{
      keyVersion: number | null
      rotatedAt: string | null
      secretCount: number
      rootSource: string
      algorithm: string
    }> => {
      const r = await fetch('/api/admin/encryption')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)
  const [newRoot, setNewRoot] = useState('')

  const rotate = async () => {
    if (!(await confirm({ title: 'Rotate encryption key', message: 'Rotate the encryption key? Every stored secret is re-encrypted under a fresh key in one pass. Existing secrets keep working.', confirmLabel: 'Rotate' }))) return
    setBusy(true)
    setMsg(null)
    try {
      const r = await fetch('/api/admin/encryption', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(newRoot.trim() ? { newRootSecret: newRoot.trim() } : {}),
      })
      const j = await r.json()
      if (!r.ok) return setMsg(j.error ?? 'rotation failed')
      setMsg(`Re-encrypted ${j.reencrypted} secret${j.reencrypted === 1 ? '' : 's'} · now key v${j.version}`)
      setNewRoot('')
      await qc.invalidateQueries({ queryKey: ['admin-encryption'] })
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Encryption</div>
      <p className="mb-4 text-xs text-muted">
        Every stored secret — provider API keys, agent secrets, Google tokens — is encrypted at rest with{' '}
        <strong>{data?.algorithm ?? 'AES-256-GCM'}</strong>. A random data key encrypts the secrets; that key is itself
        stored wrapped by the root secret, so the key that unlocks everything is never in a config file. Rotating
        re-encrypts <strong>every</strong> secret under a fresh key in one pass, no per-secret steps.
      </p>
      <div className="flex flex-wrap items-center gap-x-6 gap-y-1 text-xs text-muted">
        <span>Key version: <strong className="text-fg">v{data?.keyVersion ?? '—'}</strong></span>
        <span>Secrets protected: <strong className="text-fg">{data?.secretCount ?? '—'}</strong></span>
        <span>Root of trust: <strong className="text-fg">{data?.rootSource ?? '—'}</strong></span>
        {data?.rotatedAt && <span>Rotated: {new Date(data.rotatedAt).toLocaleString()}</span>}
      </div>
      <div className="mt-4 flex flex-wrap items-end gap-3">
        <div className="flex-1 min-w-[16rem]">
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">New root secret (optional)</label>
          <Input
            type="password"
            value={newRoot}
            onChange={(e) => setNewRoot(e.target.value)}
            placeholder="leave blank to keep the current root"
            autoComplete="off"
          />
        </div>
        <Button size="sm" onClick={() => void rotate()} disabled={busy}>
          {busy ? 'Rotating' : 'Rotate keys'}
        </Button>
      </div>
      <div className="mt-3">
        {busy && (
          <Generating
            label="Rotating the data key: re-encrypting provider keys, agent secrets, and OAuth tokens in one pass"
            lines={2}
          />
        )}
      </div>
      {newRoot.trim() && (
        <p className="mt-2 text-[11px] text-muted">
          After rotating with a new root secret, update <code>TALARIA_SECRET_KEY</code> (or the key-file) to match before the next restart.
        </p>
      )}
      {msg && <p className="mt-2 text-xs text-[color:var(--theme-success)]">{msg}</p>}
    </Panel>
  )
}

function JudgePanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['judge-config'],
    queryFn: async (): Promise<{ config: { enabled: boolean; model: string | null }; models: string[] }> => {
      const r = await fetch('/api/admin/judge')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const [saved, setSaved] = useState(false)
  const enabled = data?.config.enabled ?? false
  const model = data?.config.model ?? ''

  const save = async (patch: { enabled?: boolean; model?: string | null }) => {
    const body = { enabled: patch.enabled ?? enabled, model: patch.model !== undefined ? patch.model : model }
    const r = await fetch('/api/admin/judge', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    if (r.ok) {
      await qc.invalidateQueries({ queryKey: ['judge-config'] })
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
    }
  }

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">QA judge</div>
      <p className="mb-4 text-xs text-muted">
        A reliability gate: when an agent hands a ticket to <strong>quality review</strong>, a judge model reviews the
        reported work and posts a verdict (pass / revise / escalate) with specific issues. <strong>Advisory</strong>:
        the human reviewer still decides. Pick a strong model for the sharpest review.
      </p>
      <div className="flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-2 text-sm text-fg">
          <input type="checkbox" checked={enabled} onChange={(e) => void save({ enabled: e.target.checked })} />
          Run the judge on quality review
        </label>
        <div className="flex items-center gap-2">
          <span className="text-[11px] uppercase tracking-wide text-muted">Model</span>
          <Select size="sm" value={model} onChange={(e) => void save({ model: e.target.value || null })} className="w-64" disabled={!enabled}>
            <option value="">Default (self-hosted)</option>
            {(data?.models ?? []).map((m) => <option key={m} value={m}>{m}</option>)}
            {model && !(data?.models ?? []).includes(model) && <option value={model}>{model}</option>}
          </Select>
        </div>
        {saved && <span className="text-xs text-[color:var(--theme-success)]">Saved</span>}
      </div>
      <p className="mt-3 text-[11px] text-muted">Per-board override lives on each board (advisory / off); default follows this toggle.</p>
    </Panel>
  )
}

interface GuardData {
  config: { mode: string; checks: Record<string, boolean>; minConfidence: number; policedHosts: string[]; coach: boolean }
  stats: { total: number; byCheck: Record<string, number> }
  findings: Array<{ id: string; caller: string; model: string; check: string; severity: string; confidence: number; message: string; snippet: string; createdAt: string }>
  rules: Array<{ id: string; label: string; severity: string; defaultOn: boolean }>
}

// Confab guardrail — a structural check on model output at the gateway. Observe
// mode records findings out-of-band; annotate/strict act on them.
function GuardrailsPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['guardrails'],
    queryFn: async (): Promise<GuardData> => {
      const r = await fetch('/api/admin/guardrails')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
    refetchInterval: 30_000,
  })
  const cfg = data?.config
  const save = async (patch: Partial<GuardData['config']>) => {
    if (!cfg) return
    const body = { ...cfg, ...patch }
    await fetch('/api/admin/guardrails', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    await qc.invalidateQueries({ queryKey: ['guardrails'] })
  }
  const rules = data?.rules ?? []

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Confab guard</div>
      <p className="mb-4 text-xs text-muted">
        A cheap structural check on every model’s output at the gateway: catches claims of work no tool did, invented
        links/ids, fabricated outages, and leaked secrets. No extra model call, no added context. <strong>Observe</strong> records
        findings here; <strong>annotate</strong> also flags the reply where it appears (a caveat in chat/channels, appended on
        API responses); <strong>strict</strong> also redacts leaked secrets and personal data from the saved reply. Flagged
        content is never fed back into an agent’s context — coaching (below) delivers only counts and fixed advice, at
        render time.
      </p>
      <div className="flex flex-wrap items-center gap-4">
        <div className="flex items-center gap-2">
          <span className="text-[11px] uppercase tracking-wide text-muted">Mode</span>
          <Select size="sm" value={cfg?.mode ?? 'observe'} onChange={(e) => void save({ mode: e.target.value })} className="w-40">
            <option value="off">Off</option>
            <option value="observe">Observe</option>
            <option value="annotate">Annotate</option>
            <option value="strict">Strict</option>
          </Select>
        </div>
        <label className="flex items-center gap-2 text-xs text-muted" title="Findings below this confidence are dropped">
          <span className="uppercase tracking-wide">Min confidence</span>
          <input
            type="range" min={0} max={1} step={0.05}
            value={cfg?.minConfidence ?? 0.5}
            disabled={cfg?.mode === 'off'}
            onChange={(e) => void save({ minConfidence: Number(e.target.value) })}
            className="w-28"
          />
          <span className="w-8 text-fg">{((cfg?.minConfidence ?? 0.5) * 100).toFixed(0)}%</span>
        </label>
        <label
          className="flex items-center gap-2 text-xs text-muted"
          title="Repeated findings become templated behavioral notes in the agent's rendered soul (counts + advice only, never the flagged content). Applies on the next fleet render."
        >
          <input
            type="checkbox"
            checked={cfg?.coach ?? false}
            disabled={cfg?.mode === 'off'}
            onChange={(e) => void save({ coach: e.target.checked })}
          />
          Coach agents from findings
        </label>
        <div className="text-xs text-muted">{data?.stats.total ?? 0} findings</div>
      </div>
      <div className="mt-3 space-y-1.5">
        {rules.map((c) => (
          <label key={c.id} className="flex items-center gap-2 text-sm text-fg">
            <input
              type="checkbox"
              checked={cfg?.checks[c.id] ?? c.defaultOn}
              disabled={cfg?.mode === 'off'}
              onChange={(e) => cfg && void save({ checks: { ...cfg.checks, [c.id]: e.target.checked } })}
            />
            {c.label}
            {data?.stats.byCheck[c.id] ? <span className="text-[11px] text-muted">· {data.stats.byCheck[c.id]}</span> : null}
          </label>
        ))}
      </div>
      {data?.findings && data.findings.length > 0 && (
        <div className="mt-4">
          <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Recent findings</div>
          <div className="max-h-48 divide-y divide-line-subtle overflow-y-auto rounded-lg border border-line-subtle">
            {data.findings.slice(0, 20).map((f) => (
              <div key={f.id} className="flex items-start gap-2 px-2 py-1.5 text-xs">
                <span className="shrink-0 rounded px-1 text-[10px] uppercase" style={{ color: f.severity === 'high' ? 'var(--theme-danger)' : 'var(--theme-warning)' }}>{f.check.replace(/_/g, ' ')}</span>
                <span className="min-w-0 flex-1 truncate text-muted" title={f.snippet}>{f.snippet || f.message}</span>
                <span className="shrink-0 text-[10px] text-muted">{f.model}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </Panel>
  )
}

interface OutreachData {
  config: { enabled: boolean; intervalMinutes: number; dailyDmCap: number }
  agents: Array<{ model: string; displayName: string; proactive: boolean; personal: boolean }>
  events: Array<{ agentModel: string; kind: string; note: string | null; createdAt: string }>
}

// Proactive outreach (#59): opt-in periodic check-ins where agents surface
// things through their own governed tools, plus agent-initiated DM caps.
function OutreachPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['admin-outreach'],
    queryFn: async (): Promise<OutreachData> => {
      const r = await fetch('/api/admin/outreach')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
    refetchInterval: 30_000,
  })
  const cfg = data?.config
  const agents = data?.agents ?? []
  const save = async (patch: Partial<OutreachData['config']>, proactiveAgents?: string[]) => {
    if (!cfg) return
    const body = {
      ...cfg,
      ...patch,
      proactiveAgents: proactiveAgents ?? agents.filter((a) => a.proactive).map((a) => a.model),
    }
    await fetch('/api/admin/outreach', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    await qc.invalidateQueries({ queryKey: ['admin-outreach'] })
  }

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Proactive outreach</div>
      <p className="mb-4 text-xs text-muted">
        Opted-in agents get a periodic check-in: a look at their own stale or blocked work, and the chance to act
        through their normal tools — a ticket comment, a channel post, or a direct message that lands in your chat
        and inbox. Everything stays attributed and board-policy-gated; direct messages are capped per person per
        day. Off by default.
      </p>
      <div className="flex flex-wrap items-center gap-4">
        <label className="flex items-center gap-2 text-xs text-muted">
          <input type="checkbox" checked={cfg?.enabled ?? false} onChange={(e) => void save({ enabled: e.target.checked })} />
          Enable periodic check-ins
        </label>
        <label className="flex items-center gap-2 text-xs text-muted" title="Minimum minutes between one agent's check-ins">
          <span className="uppercase tracking-wide">Every</span>
          <Input
            size="sm" type="number" min={15} max={1440} className="w-20"
            value={cfg?.intervalMinutes ?? 240}
            disabled={!cfg?.enabled}
            onChange={(e) => void save({ intervalMinutes: Math.max(15, Math.min(1440, Number(e.target.value) || 240)) })}
          />
          <span>min</span>
        </label>
        <label className="flex items-center gap-2 text-xs text-muted" title="Agent-initiated direct messages allowed per person per day">
          <span className="uppercase tracking-wide">DM cap</span>
          <Input
            size="sm" type="number" min={1} max={20} className="w-16"
            value={cfg?.dailyDmCap ?? 3}
            onChange={(e) => void save({ dailyDmCap: Math.max(1, Math.min(20, Number(e.target.value) || 3)) })}
          />
          <span>/day</span>
        </label>
      </div>
      {agents.length > 0 && (
        <div className="mt-3">
          <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Proactive agents</div>
          <div className="flex flex-wrap gap-x-4 gap-y-1.5">
            {agents.map((a) => (
              <label key={a.model} className="flex items-center gap-2 text-sm text-fg">
                <input
                  type="checkbox"
                  checked={a.proactive}
                  onChange={(e) => {
                    const next = agents.filter((x) => (x.model === a.model ? e.target.checked : x.proactive)).map((x) => x.model)
                    void save({}, next)
                  }}
                />
                {a.displayName}
                {a.personal && <span className="text-[10px] uppercase text-muted">personal</span>}
              </label>
            ))}
          </div>
        </div>
      )}
      {data?.events && data.events.length > 0 && (
        <div className="mt-4">
          <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Recent outreach</div>
          <div className="max-h-48 divide-y divide-line-subtle overflow-y-auto rounded-lg border border-line-subtle">
            {data.events.map((ev, i) => (
              <div key={i} className="flex items-start gap-2 px-2 py-1.5 text-xs">
                <span className="shrink-0 rounded px-1 text-[10px] uppercase text-muted">{ev.kind}</span>
                <span className="min-w-0 flex-1 truncate text-muted" title={ev.note ?? ''}>{ev.note ?? '—'}</span>
                <span className="shrink-0 text-[10px] text-muted">{ev.agentModel}</span>
                <span className="shrink-0 text-[10px] text-muted">{relativeTime(ev.createdAt)}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </Panel>
  )
}

interface OrgGoogle {
  available: boolean
  connected: boolean
  email: string | null
  connectedAt: string | null
  targets: { driveFolderId: string | null; calendarId: string | null; sendAs: string | null }
}

// The shared org Google account general fleet agents act as for Drive/Docs.
function OrgGooglePanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['org-google'],
    queryFn: async (): Promise<OrgGoogle> => {
      const r = await fetch('/api/integrations/google/org')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const [flash, setFlash] = useState<string | null>(null)
  useEffect(() => {
    const p = new URLSearchParams(window.location.search).get('googleOrg')
    if (!p) return
    setFlash(p)
    window.history.replaceState({}, '', window.location.pathname)
    if (p === 'connected') void qc.invalidateQueries({ queryKey: ['org-google'] })
    const t = setTimeout(() => setFlash(null), 4000)
    return () => clearTimeout(t)
  }, [qc])

  const disconnect = async () => {
    if (!(await confirm({ title: 'Disconnect Google', message: 'Disconnect the org Google account? General agents lose Drive/Docs access.', confirmLabel: 'Disconnect', danger: true }))) return
    await fetch('/api/integrations/google/org', { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['org-google'] })
  }

  const msg: Record<string, string> = {
    connected: 'Org Google account connected.',
    denied: 'Connection cancelled.',
    bad_state: 'Connection expired. Try again.',
    exchange_failed: 'Google rejected the connection. Try again.',
    forbidden: 'Admins only.',
    disabled: 'Google integration is not configured.',
  }

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Organization Google account</div>
      <p className="mb-4 text-xs text-muted">
        A single shared Google account the fleet builds in. <strong>General agents</strong> (no personal owner)
        create Docs, Sheets, and Drive files here; a user's <strong>personal assistant</strong> instead acts as
        that user’s own connected Google.
      </p>
      {data && !data.available ? (
        <div className="text-xs text-muted">Google integration isn’t configured on this server.</div>
      ) : (
        <div className="flex items-center gap-3 rounded-xl border border-line-subtle p-4">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-line-subtle text-lg">🏢</div>
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-fg">Shared Drive & Docs</div>
            <div className="truncate text-xs text-muted">
              {data?.connected ? `Connected${data.email ? ` as ${data.email}` : ''}` : 'Not connected'}
            </div>
          </div>
          {data?.connected ? (
            <Button variant="ghost" size="sm" onClick={() => void disconnect()}>Disconnect</Button>
          ) : (
            <a href="/api/integrations/google/org/connect" className={buttonClasses({ size: 'sm' })}>Connect</a>
          )}
        </div>
      )}
      {data?.connected && <OrgGoogleTargets targets={data.targets} />}
      {flash && (
        <div className="mt-3 text-xs" style={{ color: flash === 'connected' ? 'var(--theme-success)' : 'var(--theme-danger)' }}>
          {msg[flash] ?? flash}
        </div>
      )}
    </Panel>
  )
}

// Where the org account's agents build: a Shared Drive so files are team-owned,
// a specific calendar, and an optional send-as alias for outgoing mail.
function OrgGoogleTargets({ targets }: { targets: OrgGoogle['targets'] }) {
  const qc = useQueryClient()
  const [drive, setDrive] = useState(targets.driveFolderId ?? '')
  const [cal, setCal] = useState(targets.calendarId ?? '')
  const [sendAs, setSendAs] = useState(targets.sendAs ?? '')
  const [saved, setSaved] = useState(false)
  const dirty = drive !== (targets.driveFolderId ?? '') || cal !== (targets.calendarId ?? '') || sendAs !== (targets.sendAs ?? '')

  const save = async () => {
    const r = await fetch('/api/integrations/google/org', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ driveFolderId: drive, calendarId: cal, sendAs }),
    })
    if (r.ok) {
      await qc.invalidateQueries({ queryKey: ['org-google'] })
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
    }
  }

  return (
    <div className="mt-4 space-y-3 rounded-xl border border-line-subtle p-4">
      <div className="text-xs font-medium text-fg">Where agents build</div>
      <div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Shared Drive / folder ID</label>
        <Input size="sm" value={drive} onChange={(e) => setDrive(e.target.value)} placeholder="Shared Drive or folder ID" />
        <div className="mt-1 text-[11px] text-muted">Files agents create land here (team-owned). Blank = the account’s My Drive. The org account must be a member of the Shared Drive.</div>
      </div>
      <div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Calendar ID</label>
        <Input size="sm" value={cal} onChange={(e) => setCal(e.target.value)} placeholder="team@group.calendar.google.com" />
        <div className="mt-1 text-[11px] text-muted">Org events land here. Blank = the account’s primary calendar.</div>
      </div>
      <div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Send mail as</label>
        <Input size="sm" value={sendAs} onChange={(e) => setSendAs(e.target.value)} placeholder="support@yourdomain.com" />
        <div className="mt-1 text-[11px] text-muted">A verified send-as alias on the org account for outgoing mail. Blank = the account’s own address.</div>
      </div>
      <div className="flex items-center gap-3">
        <Button size="sm" onClick={() => void save()} disabled={!dirty}>Save</Button>
        {saved && <span className="text-xs text-[color:var(--theme-success)]">Saved</span>}
      </div>
    </div>
  )
}
