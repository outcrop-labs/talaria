import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { Combobox } from '@/components/ui/combobox'
import { Button, buttonClasses } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { Select } from '@/components/ui/select'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { relativeTime } from '@/lib/fleet'
import { GATEABLE_VIEWS } from '@/lib/nav'
import { RetrievalPanel } from '@/components/admin/retrieval-panel'

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

  const update = async (userId: string, patch: { role?: 'admin' | 'member'; agentModels?: string[]; canMintKeys?: boolean; deniedViews?: string[] }) => {
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

        <SettingsPanel />

        <OrgGooglePanel />

        <RetrievalPanel />

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
    if (!confirm('Disconnect the org Google account? General agents lose Drive/Docs access.')) return
    await fetch('/api/integrations/google/org', { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['org-google'] })
  }

  const msg: Record<string, string> = {
    connected: 'Org Google account connected.',
    denied: 'Connection cancelled.',
    bad_state: 'Connection expired — try again.',
    exchange_failed: 'Google rejected the connection — try again.',
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
        <Input size="sm" value={drive} onChange={(e) => setDrive(e.target.value)} placeholder="0Ae1a…  (Shared Drive or folder ID)" />
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
