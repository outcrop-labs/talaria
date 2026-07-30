import { createFileRoute } from '@tanstack/react-router'
import { Trash2 } from 'lucide-react'
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
import { Segmented } from '@/components/ui/segmented'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { relativeTime } from '@/lib/fleet'
import { GATEABLE_VIEWS, MANAGE_VIEWS } from '@/lib/nav'
import { useEnabledApps } from '@/lib/apps'
import { RetrievalPanel } from '@/components/admin/retrieval-panel'
import { StoragePanel } from '@/components/admin/storage-panel'
import { Tabs } from '@/components/ui/tabs'
import { Checkbox } from '@/components/ui/checkbox'
import { SectionHeader } from '@/components/ui/section-header'
import { useSavedFlash } from '@/components/ui/save-button'
import { CopyButton } from '@/components/ui/copy-link-button'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'


type AdminTab = 'org' | 'people' | 'agents' | 'retrieval' | 'storage' | 'security'
const ADMIN_TABS: { id: AdminTab; label: string }[] = [
  { id: 'org', label: 'Organization' },
  { id: 'people', label: 'People' },
  { id: 'agents', label: 'Agents' },
  { id: 'retrieval', label: 'Retrieval' },
  { id: 'storage', label: 'Storage' },
  { id: 'security', label: 'Security' },
]

export const Route = createFileRoute('/_app/admin')({
  component: AdminPage,
  // /admin?tab=security deep-links a concern.
  validateSearch: (search: Record<string, unknown>): { tab?: AdminTab } => {
    const t = ADMIN_TABS.find((v) => v.id === search.tab)
    return t ? { tab: t.id } : {}
  },
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
  allowedManageViews: string[]
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

interface PermCatalogEntry {
  id: string
  label: string
  hint: string
  group: string
  memberDefault: boolean
}
interface PermsData {
  catalog: PermCatalogEntry[]
  orgDefaults: Record<string, boolean>
  overrides: Record<string, Record<string, boolean>>
}

function useAdminPermissions() {
  return useQuery({
    queryKey: ['admin-permissions'],
    queryFn: async (): Promise<PermsData | null> => {
      const r = await fetch('/api/admin/permissions', { credentials: 'same-origin' })
      if (!r.ok) return null
      return r.json()
    },
  })
}

// The admin console: people, their roles, and which agents each may use.
function AdminPage() {
  const qc = useQueryClient()
  const { data: me } = useSession()
  const { data: users, isPending: usersPending } = useAdminUsers()
  const { data: perms } = useAdminPermissions()
  const { data: fleet } = useAgents()
  const agentOptions = (fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))
  // Enabled apps join the per-person view checklist as EXPLICIT grants —
  // every app view (work and manage) defaults denied for members; an admin
  // adds each one per person, same storage as core Manage views.
  const { data: enabledApps = [] } = useEnabledApps()
  const appViews = [
    ...enabledApps.filter((a) => a.surfaces.work).map((a) => ({ to: `/x/${a.slug}`, label: a.surfaces.work! })),
    ...enabledApps.filter((a) => a.surfaces.manage).map((a) => ({ to: `/x/${a.slug}/manage`, label: a.surfaces.manage! })),
  ]
  const workViews = GATEABLE_VIEWS
  const manageViews = [...MANAGE_VIEWS.map((v) => ({ ...v, sub: 'manage' })), ...appViews.map((v) => ({ ...v, sub: 'app' }))]
  const [error, setError] = useState<string | null>(null)

  const update = async (userId: string, patch: { role?: 'admin' | 'member'; agentModels?: string[]; canMintKeys?: boolean; deniedViews?: string[]; allowedManageViews?: string[]; assistantElevated?: boolean }) => {
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

  const search = Route.useSearch()
  const nav = Route.useNavigate()
  const tab: AdminTab = search.tab ?? 'org'
  const setTab = (t: AdminTab) => void nav({ search: t === 'org' ? {} : { tab: t } })

  if (me && me.role !== 'admin') {
    return <EmptyState icon="⛨" title="Admins only" hint="Ask an admin if you need access here." />
  }

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-4xl space-y-6">
        <h1 className="mercury-text text-2xl font-semibold">Admin</h1>

        {/* One concern per tab; every panel keeps its own component. */}
        <Tabs items={ADMIN_TABS} value={tab} onChange={setTab} />

        {tab === 'org' && (
          <>
            <OrgPanel />
            <InstanceDomainPanel />
            <SignupDomainsPanel />
            <EmailPanel />
            <GithubPanel />
            <OrgGooglePanel />
          </>
        )}
        {tab === 'agents' && (
          <>
            <JudgePanel />
            <GuardrailsPanel />
            <OutreachPanel />
          </>
        )}
        {tab === 'retrieval' && <RetrievalPanel />}
        {tab === 'storage' && <StoragePanel />}
        {tab === 'security' && (
          <>
            <EncryptionPanel />
            <SettingsPanel />
          </>
        )}
        {tab === 'people' && <InvitesPanel />}
        {tab === 'people' && perms && <MemberDefaultsPanel perms={perms} />}
        {tab === 'people' && (
        <Panel>
          <SectionHeader
            className="mb-4"
            title="People"
            info="Roles, per-person agent access, and which views each member can reach. Empty = all (open by default); pick any to restrict. Admins always have full access."
          />
          {error && (
            <div className="mb-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
              {error}
            </div>
          )}
          {usersPending ? (
            // Person-row skeletons: avatar + name/email bars + the role/keys
            // controls, so the resolved list lands without a jump.
            <div className="divide-y divide-line-subtle">
              {Array.from({ length: 4 }, (_, i) => (
                <div key={i} className="flex items-center gap-3 py-3">
                  <Skeleton className="h-7 w-7 shrink-0 rounded-full" delay={i * 0.12} />
                  <div className="min-w-0 flex-1 space-y-1.5">
                    <Skeleton className="h-2.5 w-36 rounded-full" delay={i * 0.12} />
                    <Skeleton className="h-2 w-48 rounded-full" delay={i * 0.12 + 0.06} />
                  </div>
                  <Skeleton className="h-7 w-28 shrink-0" delay={i * 0.12} />
                  <Skeleton className="h-4 w-12 shrink-0" delay={i * 0.12} />
                  <Skeleton className="h-2.5 w-16 shrink-0 rounded-full" delay={i * 0.12} />
                </div>
              ))}
            </div>
          ) : (
          <ul className="divide-y divide-line-subtle">
            {(users ?? []).map((u) => {
              // One ALLOW list spanning both worlds: work views default in
              // (denials stored), Manage views default out (allows stored).
              const allowedViews = [
                ...workViews.filter((v) => !u.deniedViews.includes(v.to)).map((v) => v.to),
                ...manageViews.filter((v) => u.allowedManageViews.includes(v.to)).map((v) => v.to),
              ]
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
                    {u.role === 'admin' && u.assistantModel && (
                      <Checkbox
                        className="shrink-0"
                        title={`Give ${u.assistantModel} org-wide view/edit: every board, every channel and relay (never DMs), and editor rights on all org-visible knowledge and artifacts. Only while this user is an admin.`}
                        checked={u.assistantElevated}
                        onChange={(checked) => void update(u.id, { assistantElevated: checked })}
                        label="elevated assistant"
                      />
                    )}
                    <span className="w-16 shrink-0 text-right text-xs text-muted">{relativeTime(u.lastSeenAt)}</span>
                  </div>
                  {u.role !== 'admin' && (
                    <>
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
                          options={[
                            ...workViews.map((v) => ({ value: v.to, label: v.label, sub: 'work' })),
                            ...manageViews.map((v) => ({ value: v.to, label: v.label, sub: v.sub })),
                          ]}
                          selected={allowedViews}
                          onChange={(views) =>
                            void update(u.id, {
                              deniedViews: workViews.filter((v) => !views.includes(v.to)).map((v) => v.to),
                              allowedManageViews: manageViews.filter((v) => views.includes(v.to)).map((v) => v.to),
                            })
                          }
                          multiple
                          size="sm"
                          placeholder="Work views"
                          className="min-w-0 flex-1"
                        />
                      </div>
                      {perms && <UserPermChips userId={u.id} perms={perms} />}
                    </>
                  )}
                </li>
              )
            })}
          </ul>
          )}
        </Panel>
        )}
      </div>
    </div>
  )
}

/** One permission as a toggle chip. Filled = effectively allowed. A dot marks
 *  a value overridden away from the org default; clicking toggles, and
 *  toggling back to the default clears the override. */
function PermChip({
  entry,
  effective,
  overridden,
  onToggle,
}: {
  entry: PermCatalogEntry
  effective: boolean
  overridden: boolean
  onToggle: () => void
}) {
  return (
    <button
      type="button"
      title={`${entry.hint}${overridden ? ' (overridden — click twice to return to default)' : ''}`}
      onClick={onToggle}
      className={cn(
        'flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] transition-colors',
        effective
          ? 'border-accent/60 bg-accent/10 text-fg'
          : 'border-line-subtle text-muted hover:border-line hover:text-fg',
      )}
    >
      {entry.label}
      {overridden && <span className="h-1 w-1 rounded-full bg-[color:var(--theme-warning)]" title="overridden" />}
    </button>
  )
}

const permGroups = (catalog: PermCatalogEntry[]): Array<[string, PermCatalogEntry[]]> => {
  const groups = new Map<string, PermCatalogEntry[]>()
  for (const p of catalog) groups.set(p.group, [...(groups.get(p.group) ?? []), p])
  return [...groups.entries()]
}

/** Per-user fine-grained permissions: chips grouped by concern, showing the
 *  EFFECTIVE state (override → org default → shipped default). */
function UserPermChips({ userId, perms }: { userId: string; perms: PermsData }) {
  const qc = useQueryClient()
  const overrides = perms.overrides[userId] ?? {}
  const orgDefault = (p: PermCatalogEntry) => perms.orgDefaults[p.id] ?? p.memberDefault
  const set = async (perm: string, allowed: boolean | null) => {
    await fetch('/api/admin/permissions', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ userId, perm, allowed }),
    })
    await qc.invalidateQueries({ queryKey: ['admin-permissions'] })
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5 pl-10">
      <span className="text-[11px] uppercase tracking-wide text-muted/80">can</span>
      {perms.catalog.map((p) => {
        const effective = overrides[p.id] ?? orgDefault(p)
        return (
          <PermChip
            key={p.id}
            entry={p}
            effective={effective}
            overridden={overrides[p.id] !== undefined}
            // Toggling back to the org default clears the override row.
            onToggle={() => void set(p.id, !effective === orgDefault(p) ? null : !effective)}
          />
        )
      })}
    </div>
  )
}

/** Org-wide member defaults — what a plain member can do before any per-user
 *  override. Chips toggle; returning to the shipped default clears the row. */
function MemberDefaultsPanel({ perms }: { perms: PermsData }) {
  const qc = useQueryClient()
  const set = async (perm: string, enabled: boolean | null) => {
    await fetch('/api/admin/permissions', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orgDefault: { perm, enabled } }),
    })
    await qc.invalidateQueries({ queryKey: ['admin-permissions'] })
  }
  return (
    <Panel>
      <SectionHeader
        title="Member defaults"
        info="What every plain member may do out of the box. Per-person exceptions live on each row below; admins always hold every permission. A dot marks a default you've changed from Talaria's shipped baseline."
      />
      <div className="space-y-2">
        {permGroups(perms.catalog).map(([group, entries]) => (
          <div key={group} className="flex flex-wrap items-center gap-1.5">
            <span className="w-16 shrink-0 text-[11px] uppercase tracking-wide text-muted/80">{group}</span>
            {entries.map((p) => {
              const effective = perms.orgDefaults[p.id] ?? p.memberDefault
              return (
                <PermChip
                  key={p.id}
                  entry={p}
                  effective={effective}
                  overridden={perms.orgDefaults[p.id] !== undefined && perms.orgDefaults[p.id] !== p.memberDefault}
                  onToggle={() => void set(p.id, !effective === p.memberDefault ? null : !effective)}
                />
              )
            })}
          </div>
        ))}
      </div>
    </Panel>
  )
}

/** The HOSTING domain — where this deployment lives. Verified by a self-
 *  fetch round trip (DNS + routing + TLS must land on this instance), and
 *  then used as the canonical base URL (stable OAuth callbacks, links). */
function InstanceDomainPanel() {
  const qc = useQueryClient()
  const { data, isPending } = useQuery({
    queryKey: ['instance-domain'],
    queryFn: async (): Promise<{ domain: string; verified: boolean; verifiedAt: string | null } | null> => {
      const r = await fetch('/api/admin/instance', { credentials: 'same-origin' })
      if (!r.ok) return null
      return ((await r.json()) as { instance: { domain: string; verified: boolean; verifiedAt: string | null } | null }).instance
    },
  })
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['instance-domain'] })

  const post = async (body: Record<string, unknown>) => {
    setBusy(true)
    setError(null)
    // PUT writes the domain config; POST runs the verify action.
    const r = await fetch('/api/admin/instance', {
      method: 'verify' in body ? 'POST' : 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    const j = (await r.json().catch(() => ({}))) as { error?: string; verified?: boolean }
    setBusy(false)
    if (!r.ok || j.error) setError(j.error ?? 'failed')
    await refresh()
  }

  return (
    <Panel className="mt-4">
      <SectionHeader
        title="Instance domain"
        info="Where THIS Talaria deployment is hosted (e.g. talaria.yourcompany.com) — separate from your email sign-up domains below. Verification round-trips through the domain and confirms it reaches this exact instance. Once verified it becomes the canonical base URL: OAuth apps get one stable callback, links use it."
      />
      {isPending ? (
        <SkeletonRows rows={1} />
      ) : data ? (
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-fg">{data.domain}</span>
          {data.verified ? (
            <span className="rounded-full border border-[color:var(--theme-success)]/40 px-1.5 py-0.5 text-[10px] text-[color:var(--theme-success)]">
              ✓ verified — routes to this instance
            </span>
          ) : (
            <span className="rounded-full border border-[color:var(--theme-warning)]/40 px-1.5 py-0.5 text-[10px] text-[color:var(--theme-warning)]">
              unverified
            </span>
          )}
          <span className="flex-1" />
          {!data.verified && (
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void post({ verify: true })}>
              {busy ? 'Checking' : 'Verify'}
            </Button>
          )}
          <button
            type="button"
            title="Clear the hosting domain"
            onClick={() => void post({ domain: null })}
            className="text-muted hover:text-[color:var(--theme-danger)]"
          >
            <Trash2 size={13} />
          </button>
        </div>
      ) : (
        <div className="flex items-center gap-2">
          <Input
            size="sm"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && draft.trim() && void post({ domain: draft.trim() })}
            placeholder="talaria.yourcompany.com — where this instance is hosted"
            className="w-96"
          />
          <Button size="sm" disabled={!draft.trim() || busy} onClick={() => void post({ domain: draft.trim() })}>
            Set domain
          </Button>
        </div>
      )}
      {error && (
        <div className="mt-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
          {error}
        </div>
      )}
    </Panel>
  )
}

/** Transactional email: bring your own SMTP (Google Workspace etc.) or
 *  connect Resend. More providers as requested. */
function EmailPanel() {
  const qc = useQueryClient()
  interface EmailCfg {
    provider: 'smtp' | 'resend' | null
    from: string
    smtp: { host: string; port: number; secure: boolean; user: string; passSet: boolean }
    resend: { apiKeySet: boolean }
  }
  const { data, isPending } = useQuery({
    queryKey: ['email-config'],
    queryFn: async (): Promise<EmailCfg | null> => {
      const r = await fetch('/api/admin/email', { credentials: 'same-origin' })
      if (!r.ok) return null
      return ((await r.json()) as { config: EmailCfg }).config
    },
  })
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [secret, setSecret] = useState('') // smtp password / resend key draft

  const post = async (body: unknown, okNotice?: string) => {
    setBusy(true)
    setError(null)
    setNotice(null)
    // PUT patches the config; POST sends the test email.
    const r = await fetch('/api/admin/email', {
      method: typeof body === 'object' && body !== null && 'test' in body ? 'POST' : 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    setBusy(false)
    const j = (await r.json().catch(() => ({}))) as { error?: string }
    if (!r.ok || j.error) setError(j.error ?? 'failed')
    else if (okNotice) setNotice(okNotice)
    await qc.invalidateQueries({ queryKey: ['email-config'] })
  }

  if (isPending || !data) {
    return (
      <Panel className="mt-4">
        <Skeleton className="mb-3 h-4 w-24 rounded-full" />
        <SkeletonRows rows={2} />
      </Panel>
    )
  }
  return (
    <Panel className="mt-4">
      <SectionHeader
        title="Email"
        info="Transactional email — invites today, more later. Bring your own SMTP (e.g. Google Workspace: smtp.gmail.com, port 587, an app password) or connect Resend. Secrets are stored encrypted and never shown again."
      />
      <div className="space-y-3">
        <div className="flex items-center gap-3">
          <label className="w-24 shrink-0 text-[11px] uppercase tracking-wide text-muted">Provider</label>
          <Select
            size="sm"
            value={data.provider ?? ''}
            onChange={(e) => void post({ provider: (e.target.value || null) as 'smtp' | 'resend' | null })}
            className="w-44"
          >
            <option value="">Not configured</option>
            <option value="smtp">Your own SMTP</option>
            <option value="resend">Resend</option>
          </Select>
        </div>
        {data.provider && (
          <div className="flex items-center gap-3">
            <label className="w-24 shrink-0 text-[11px] uppercase tracking-wide text-muted">From</label>
            <Input
              size="sm"
              defaultValue={data.from}
              onBlur={(e) => e.target.value !== data.from && void post({ from: e.target.value })}
              placeholder='Talaria <talaria@yourcompany.com>'
              className="w-96"
            />
          </div>
        )}
        {data.provider === 'smtp' && (
          <>
            <div className="flex items-center gap-3">
              <label className="w-24 shrink-0 text-[11px] uppercase tracking-wide text-muted">Host / port</label>
              <Input size="sm" defaultValue={data.smtp.host} onBlur={(e) => void post({ smtp: { host: e.target.value } })} placeholder="smtp.gmail.com" className="w-56" />
              <Input size="sm" defaultValue={String(data.smtp.port)} onBlur={(e) => void post({ smtp: { port: Number(e.target.value) || 587 } })} className="w-20" />
              <Checkbox
                title="TLS from the first byte (port 465). Off = STARTTLS (587)."
                checked={data.smtp.secure}
                onChange={(checked) => void post({ smtp: { secure: checked } })}
                label="TLS"
              />
            </div>
            <div className="flex items-center gap-3">
              <label className="w-24 shrink-0 text-[11px] uppercase tracking-wide text-muted">User</label>
              <Input size="sm" defaultValue={data.smtp.user} onBlur={(e) => void post({ smtp: { user: e.target.value } })} placeholder="talaria@yourcompany.com" className="w-56" />
              <Input
                size="sm"
                type="password"
                value={secret}
                onChange={(e) => setSecret(e.target.value)}
                placeholder={data.smtp.passSet ? 'password saved — type to replace' : 'password / app password'}
                autoComplete="off"
                className="w-56"
              />
              <Button size="sm" variant="outline" disabled={!secret.trim() || busy} onClick={() => { void post({ smtp: { pass: secret } }, 'password saved'); setSecret('') }}>
                Save
              </Button>
            </div>
          </>
        )}
        {data.provider === 'resend' && (
          <div className="flex items-center gap-3">
            <label className="w-24 shrink-0 text-[11px] uppercase tracking-wide text-muted">API key</label>
            <Input
              size="sm"
              type="password"
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              placeholder={data.resend.apiKeySet ? 'key saved — type to replace' : 're_…'}
              autoComplete="off"
              className="w-72"
            />
            <Button size="sm" variant="outline" disabled={!secret.trim() || busy} onClick={() => { void post({ resend: { apiKey: secret } }, 'key saved'); setSecret('') }}>
              Save
            </Button>
          </div>
        )}
        {data.provider && (
          <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
            <Button size="sm" disabled={busy} onClick={() => void post({ test: true }, 'test sent — check your inbox')}>
              {busy ? 'Working' : 'Send me a test'}
            </Button>
            {notice && <span className="text-xs" style={{ color: 'var(--theme-success)' }}>{notice}</span>}
          </div>
        )}
        {error && <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>{error}</div>}
      </div>
    </Panel>
  )
}

/** Invites: the third door in. Create + send, watch state, revoke. */
function InvitesPanel() {
  const qc = useQueryClient()
  interface InviteRow {
    id: string
    email: string
    invitedBy: string | null
    createdAt: string
    expiresAt: string
    acceptedAt: string | null
    revokedAt: string | null
  }
  const { data, isPending } = useQuery({
    queryKey: ['invites'],
    queryFn: async (): Promise<InviteRow[]> => {
      const r = await fetch('/api/admin/invites', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { invites: InviteRow[] }).invites
    },
  })
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['invites'] })

  const invite = async () => {
    if (!draft.trim()) return
    setBusy(true)
    setError(null)
    setNotice(null)
    const r = await fetch('/api/admin/invites', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: draft.trim() }),
    })
    setBusy(false)
    const j = (await r.json().catch(() => ({}))) as { error?: string; emailSent?: boolean; emailError?: string }
    if (!r.ok || j.error) {
      setError(j.error ?? 'failed')
    } else {
      setDraft('')
      setNotice(j.emailSent ? 'invite sent' : `invite created, but the email failed: ${j.emailError ?? 'no email provider configured'}`)
    }
    await refresh()
  }
  const state = (i: InviteRow) =>
    i.acceptedAt ? 'accepted' : i.revokedAt ? 'revoked' : new Date(i.expiresAt) < new Date() ? 'expired' : 'pending'

  return (
    <Panel className="mb-4">
      <SectionHeader
        title="Invites"
        info="Invite by email — they get a join link and are admitted the moment they sign in with Google on that address. Invites expire after two weeks; re-inviting re-issues a fresh link; revoking shuts the door instantly. Needs the Email provider on the Org tab."
      />
      <div className="mb-3 flex items-center gap-2">
        <Input
          size="sm"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void invite()}
          placeholder="teammate@yourcompany.com"
          className="w-72"
        />
        <Button size="sm" disabled={busy || !draft.trim()} onClick={() => void invite()}>
          {busy ? 'Sending' : 'Invite'}
        </Button>
        {notice && <span className="min-w-0 truncate text-xs text-muted">{notice}</span>}
      </div>
      {error && <div className="mb-2 text-xs" style={{ color: 'var(--theme-danger)' }}>{error}</div>}
      {isPending ? (
        <SkeletonRows rows={2} />
      ) : (data ?? []).length === 0 ? (
        <EmptyState variant="inline" title="No invites yet." className="font-sans" />
      ) : (
        <ul className="divide-y divide-line-subtle">
          {(data ?? []).map((i) => {
            const st = state(i)
            return (
              <li key={i.id} className="flex items-center gap-3 py-2 text-sm">
                <span className="min-w-0 flex-1 truncate text-fg">{i.email}</span>
                <span
                  className={cn(
                    'shrink-0 rounded-full border px-1.5 py-0.5 text-[10px]',
                    st === 'accepted' && 'border-[color:var(--theme-success)]/40 text-[color:var(--theme-success)]',
                    st === 'pending' && 'border-line-subtle text-muted',
                    (st === 'revoked' || st === 'expired') && 'border-line-subtle text-muted/60 line-through',
                  )}
                >
                  {st}
                </span>
                <span className="shrink-0 text-xs text-muted">{relativeTime(i.createdAt)}</span>
                {st === 'pending' && (
                  <button
                    type="button"
                    title="Revoke — the link stops working immediately"
                    onClick={async () => {
                      await fetch('/api/admin/invites', {
                        method: 'DELETE',
                        credentials: 'same-origin',
                        headers: { 'content-type': 'application/json' },
                        body: JSON.stringify({ id: i.id }),
                      })
                      await refresh()
                    }}
                    className="shrink-0 text-muted hover:text-[color:var(--theme-danger)]"
                  >
                    <Trash2 size={13} />
                  </button>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </Panel>
  )
}

interface OrgDomainRow {
  id: string
  domain: string
  verified: boolean
  verificationToken: string
  verifiedAt: string | null
}

/** Sign-up domains: prove a domain with a DNS TXT record and anyone signing
 *  in through Google with an email on it joins as a member — no invites. */
function SignupDomainsPanel() {
  const qc = useQueryClient()
  const { data, isPending } = useQuery({
    queryKey: ['org-domains'],
    queryFn: async (): Promise<OrgDomainRow[]> => {
      const r = await fetch('/api/admin/domains', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { domains: OrgDomainRow[] }).domains
    },
  })
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [verifying, setVerifying] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['org-domains'] })

  const add = async () => {
    if (!draft.trim()) return
    setError(null)
    const r = await fetch('/api/admin/domains', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ domain: draft.trim() }),
    })
    if (!r.ok) {
      setError(((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed')
      return
    }
    setDraft('')
    await refresh()
  }
  const verify = async (id: string) => {
    setVerifying(id)
    setError(null)
    const r = await fetch('/api/admin/domains', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ verifyId: id }),
    })
    const j = (await r.json().catch(() => ({}))) as { verified?: boolean; error?: string }
    setVerifying(null)
    if (!j.verified) setError(j.error ?? 'verification failed')
    await refresh()
  }

  return (
    <Panel className="mt-4">
      <SectionHeader
        title="Email sign-up domains"
        info="The domain after the @ in your team's EMAIL addresses — not where Talaria is hosted (talaria.yourcompany.com hosting still means yourcompany.com emails). Add it, publish the TXT record to prove ownership, and anyone signing in with Google on that email domain becomes a member automatically — no invites, no env edits. Email subdomains are separate; add each you use. Password logins stay env-managed."
      />
      {isPending ? (
        <SkeletonRows rows={2} />
      ) : (
        <div className="space-y-2">
          {(data ?? []).length === 0 && (
            <EmptyState
              variant="inline"
              title="No email domains yet"
              hint="add the domain your team's email addresses use to open self-service joins."
              className="font-sans"
            />
          )}
          {(data ?? []).map((d) => (
            <div key={d.id} className="space-y-1.5 rounded-xl border border-line-subtle p-2.5">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-fg">{d.domain}</span>
                {d.verified ? (
                  <span className="rounded-full border border-[color:var(--theme-success)]/40 px-1.5 py-0.5 text-[10px] text-[color:var(--theme-success)]">
                    ✓ verified — self-joins open
                  </span>
                ) : (
                  <span className="rounded-full border border-[color:var(--theme-warning)]/40 px-1.5 py-0.5 text-[10px] text-[color:var(--theme-warning)]">
                    awaiting DNS proof
                  </span>
                )}
                <span className="flex-1" />
                {!d.verified && (
                  <Button size="sm" variant="outline" disabled={verifying === d.id} onClick={() => void verify(d.id)}>
                    {verifying === d.id ? 'Checking' : 'Verify'}
                  </Button>
                )}
                <button
                  type="button"
                  title="Remove — self-joins from this domain stop immediately"
                  onClick={async () => {
                    if (!(await confirm({ title: 'Remove domain', message: `Remove ${d.domain}? New self-joins stop; existing members keep their accounts.`, confirmLabel: 'Remove' }))) return
                    await fetch('/api/admin/domains', {
                      method: 'DELETE',
                      credentials: 'same-origin',
                      headers: { 'content-type': 'application/json' },
                      body: JSON.stringify({ id: d.id }),
                    })
                    await refresh()
                  }}
                  className="text-muted hover:text-[color:var(--theme-danger)]"
                >
                  <Trash2 size={13} />
                </button>
              </div>
              {!d.verified && (
                <div className="flex items-center gap-2">
                  <span className="shrink-0 text-[11px] text-muted">TXT on _talaria-verify.{d.domain}:</span>
                  <code className="min-w-0 flex-1 truncate rounded bg-card px-1.5 py-0.5 text-[11px] text-fg">{d.verificationToken}</code>
                  <CopyButton value={d.verificationToken} label="Copy" className="text-xs" />
                </div>
              )}
            </div>
          ))}
          <div className="flex items-center gap-2 pt-1">
            <Input
              size="sm"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && void add()}
              placeholder="yourcompany.com — your email domain"
              className="w-72"
            />
            <Button size="sm" onClick={() => void add()} disabled={!draft.trim()}>
              Add domain
            </Button>
          </div>
          {error && (
            <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>
              {error}
            </div>
          )}
        </div>
      )}
    </Panel>
  )
}

// App-wide settings (grows over time). Audit retention is the first.
// The business every agent works for. Woven automatically into agent design
// (muse-generated souls anchor to this team) and every rendered SOUL.md — so
// no agent introduces itself as belonging to the underlying platform.
function OrgPanel() {
  const qc = useQueryClient()
  const { data, isPending } = useQuery({
    queryKey: ['admin-settings'],
    queryFn: async (): Promise<{ auditRetentionDays: number; org: { name: string; about: string } }> => {
      const r = await fetch('/api/admin/settings')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const [name, setName] = useState<string | null>(null)
  const [about, setAbout] = useState<string | null>(null)
  const { saved, flash } = useSavedFlash()
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
    flash()
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
      {isPending ? (
        // Hold the form's footprint until settings land, so the fields never
        // render blank and then fill in.
        <div className="space-y-3">
          <div>
            <Skeleton className="mb-1.5 h-2.5 w-24 rounded-full" />
            <Skeleton className="h-9 w-full max-w-xs" />
          </div>
          <div>
            <Skeleton className="mb-1.5 h-2.5 w-40 rounded-full" delay={0.12} />
            <Skeleton className="h-14 w-full" delay={0.12} />
          </div>
        </div>
      ) : (
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
      )}
    </Panel>
  )
}

function SettingsPanel() {
  const qc = useQueryClient()
  const { data, isPending } = useQuery({
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
      {isPending ? (
        // Hold the retention row so the input never renders blank, then fills.
        <div className="flex items-center gap-3">
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-3 w-28 rounded-full" />
            <Skeleton className="h-2.5 w-64 rounded-full" delay={0.12} />
          </div>
          <Skeleton className="h-8 w-24 shrink-0" />
          <Skeleton className="h-7 w-16 shrink-0" delay={0.12} />
        </div>
      ) : (
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
      )}
    </Panel>
  )
}

// The automated QA judge — an advisory reliability gate. When a ticket hits
// quality_review, a judge model reviews the agent's work and posts a verdict.
function EncryptionPanel() {
  const qc = useQueryClient()
  const { data, isPending } = useQuery({
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
      <SectionHeader
        className="mb-4"
        title="Encryption"
        info="Every stored secret is encrypted at rest (AES-256-GCM). A random data key encrypts the secrets; that key is stored wrapped by the root secret, so the key that unlocks everything is never in a config file. Rotating re-encrypts every secret under a fresh key in one pass."
      />
      {isPending ? (
        // Stat pills shimmer instead of rendering "v—/—" and reflowing.
        <div className="flex flex-wrap items-center gap-x-6 gap-y-1">
          {Array.from({ length: 4 }, (_, i) => (
            <Skeleton key={i} className="h-3 w-24 rounded-full" delay={i * 0.12} />
          ))}
        </div>
      ) : (
      <div className="flex flex-wrap items-center gap-x-6 gap-y-1 text-xs text-muted">
        <span>Key version: <strong className="text-fg">v{data?.keyVersion ?? '—'}</strong></span>
        <span>Secrets protected: <strong className="text-fg">{data?.secretCount ?? '—'}</strong></span>
        <span>Root of trust: <strong className="text-fg">{data?.rootSource ?? '—'}</strong></span>
        {data?.rotatedAt && <span>Rotated: {new Date(data.rotatedAt).toLocaleString()}</span>}
      </div>
      )}
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
  const { data, isPending } = useQuery({
    queryKey: ['judge-config'],
    queryFn: async (): Promise<{ config: { enabled: boolean; model: string | null; mode?: 'advisory' | 'enforcing' }; models: string[] }> => {
      const r = await fetch('/api/admin/judge')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const { saved, flash } = useSavedFlash()
  const enabled = data?.config.enabled ?? false
  const model = data?.config.model ?? ''
  const mode = data?.config.mode ?? 'enforcing'

  const save = async (patch: { enabled?: boolean; model?: string | null; mode?: 'advisory' | 'enforcing' }) => {
    const body = { enabled: patch.enabled ?? enabled, model: patch.model !== undefined ? patch.model : model, mode: patch.mode ?? mode }
    const r = await fetch('/api/admin/judge', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    if (r.ok) {
      await qc.invalidateQueries({ queryKey: ['judge-config'] })
      flash()
    }
  }

  return (
    <Panel>
      <SectionHeader
        className="mb-4"
        title="QA judge"
        info="When an agent hands a ticket to quality review, a judge model reviews the reported work and posts a verdict (pass / revise / escalate) with specific issues. Enforcing: bad submissions never sit in QA — revise verdicts bounce straight back to the agent with the issues (capped, then a human takes over). Advisory: verdicts only, the human decides. Pick a strong model for the sharpest review."
      />
      {isPending ? (
        // The checkbox and model select seed from the query — hold the whole
        // control row so nothing renders unchecked and then flips.
        <div className="flex flex-wrap items-center gap-3">
          <Skeleton className="h-4 w-4" />
          <Skeleton className="h-3 w-52 rounded-full" delay={0.12} />
          <Skeleton className="h-8 w-64" delay={0.24} />
        </div>
      ) : (
      <div className="flex flex-wrap items-center gap-3">
        <Checkbox
          className="gap-2 text-sm text-fg"
          checked={enabled}
          onChange={(checked) => void save({ enabled: checked })}
          label="Run the judge on quality review"
        />
        <Segmented
          options={[
            { id: 'enforcing' as const, label: 'Enforcing' },
            { id: 'advisory' as const, label: 'Advisory' },
          ]}
          value={mode}
          onChange={(m) => void save({ mode: m })}
        />
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
      )}
      <p className="mt-3 text-[11px] text-muted">Per-board override lives on each board (enforcing / advisory / off); boards on "inherit" follow this stance.</p>
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
  const { data, isPending } = useQuery({
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
      <SectionHeader
        className="mb-4"
        title="Confab guard"
        info="A structural check on every model output at the gateway: fabricated tool claims, invented links, outage stories, leaked secrets and PII. Observe records findings; annotate flags the reply; strict also redacts. Flagged content never re-enters an agent\u2019s context."
      />
      {isPending ? (
        // Everything here seeds from the query (mode, slider, coach toggle,
        // rules) — hold the full control area so no false defaults flash.
        <>
          <div className="flex flex-wrap items-center gap-4">
            <Skeleton className="h-8 w-40" />
            <Skeleton className="h-4 w-40 rounded-full" delay={0.12} />
            <Skeleton className="h-4 w-44 rounded-full" delay={0.24} />
          </div>
          <div className="mt-3 space-y-1.5">
            {Array.from({ length: 6 }, (_, i) => (
              <div key={i} className="flex items-center gap-2 py-0.5">
                <Skeleton className="h-4 w-4" delay={i * 0.1} />
                <Skeleton className="h-3 w-48 rounded-full" delay={i * 0.1} />
              </div>
            ))}
          </div>
          <div className="mt-4">
            <SkeletonRows rows={3} />
          </div>
        </>
      ) : (
      <>
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
        <Checkbox
          className="gap-2"
          title="Repeated findings become templated behavioral notes in the agent's rendered soul (counts + advice only, never the flagged content). Applies on the next fleet render."
          checked={cfg?.coach ?? false}
          disabled={cfg?.mode === 'off'}
          onChange={(checked) => void save({ coach: checked })}
          label="Coach agents from findings"
        />
        <div className="text-xs text-muted">{data?.stats.total ?? 0} findings</div>
      </div>
      <div className="mt-3 space-y-1.5">
        {rules.map((c) => (
          <Checkbox
            key={c.id}
            className="gap-2 text-sm text-fg"
            checked={cfg?.checks[c.id] ?? c.defaultOn}
            disabled={cfg?.mode === 'off'}
            onChange={(checked) => cfg && void save({ checks: { ...cfg.checks, [c.id]: checked } })}
            label={
              <>
                {c.label}
                {data?.stats.byCheck[c.id] ? <span className="text-[11px] text-muted">· {data.stats.byCheck[c.id]}</span> : null}
              </>
            }
          />
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
      </>
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
  const { data, isPending } = useQuery({
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
      <SectionHeader
        className="mb-4"
        title="Proactive outreach"
        info="Opted-in agents get a periodic check-in: a look at their stale or blocked work, and the chance to act through their normal tools — a ticket comment, a channel post, or a direct message to your inbox. Everything stays attributed and board-policy-gated; DMs are capped per person per day. Off by default."
      />
      {isPending ? (
        // Controls, agent chips, and the recent list all seed from the query —
        // hold their footprint so the toggle never flashes unchecked.
        <>
          <div className="flex flex-wrap items-center gap-4">
            <Skeleton className="h-4 w-44 rounded-full" />
            <Skeleton className="h-8 w-32" delay={0.12} />
            <Skeleton className="h-8 w-28" delay={0.24} />
          </div>
          <div className="mt-3">
            <Skeleton className="mb-2 h-2.5 w-28 rounded-full" />
            <div className="flex flex-wrap gap-x-4 gap-y-1.5">
              {Array.from({ length: 4 }, (_, i) => (
                <Skeleton key={i} className="h-4 w-28 rounded-full" delay={i * 0.1} />
              ))}
            </div>
          </div>
          <div className="mt-4">
            <SkeletonRows rows={3} />
          </div>
        </>
      ) : (
      <>
      <div className="flex flex-wrap items-center gap-4">
        <Checkbox
          className="gap-2"
          checked={cfg?.enabled ?? false}
          onChange={(checked) => void save({ enabled: checked })}
          label="Enable periodic check-ins"
        />
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
              <Checkbox
                key={a.model}
                className="gap-2 text-sm text-fg"
                checked={a.proactive}
                onChange={(checked) => {
                  const next = agents.filter((x) => (x.model === a.model ? checked : x.proactive)).map((x) => x.model)
                  void save({}, next)
                }}
                label={
                  <>
                    {a.displayName}
                    {a.personal && <span className="text-[10px] uppercase text-muted">personal</span>}
                  </>
                }
              />
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
      </>
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
  const { data, isPending } = useQuery({
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
      <SectionHeader
        className="mb-4"
        title="Organization Google account"
        info="A single shared Google account the fleet builds in. General agents (no personal owner) create Docs, Sheets, and Drive files here; a personal assistant instead acts as its owner\u2019s own connected Google."
      />
      {isPending ? (
        // Never show "Not connected" + Connect while the status is in flight —
        // hold the connection row's shape until it resolves.
        <div className="flex items-center gap-3 rounded-xl border border-line-subtle p-4">
          <Skeleton className="h-9 w-9 shrink-0" />
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-3 w-36 rounded-full" />
            <Skeleton className="h-2.5 w-52 rounded-full" delay={0.12} />
          </div>
          <Skeleton className="h-7 w-24 shrink-0" delay={0.24} />
        </div>
      ) : data && !data.available ? (
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
  const { saved, flash } = useSavedFlash()
  const dirty = drive !== (targets.driveFolderId ?? '') || cal !== (targets.calendarId ?? '') || sendAs !== (targets.sendAs ?? '')

  const save = async () => {
    const r = await fetch('/api/integrations/google/org', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ driveFolderId: drive, calendarId: cal, sendAs }),
    })
    if (r.ok) {
      await qc.invalidateQueries({ queryKey: ['org-google'] })
      flash()
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


/** The Workbench's GitHub connection — App or PAT, whichever the org prefers,
 *  with the setup steps inline. Secrets seal on save and never render back. */
function GithubPanel() {
  const qc = useQueryClient()
  interface GhStatus {
    mode: 'app' | 'pat' | null
    configured: boolean
    account: string | null
    error: string | null
    app: { appId: string; installationId: string; keySet: boolean }
    patSet: boolean
  }
  const { data: status, isPending } = useQuery({
    queryKey: ['github-status'],
    queryFn: async (): Promise<GhStatus | null> => {
      const r = await fetch('/api/workbench/github', { credentials: 'same-origin' })
      if (!r.ok) return null
      return ((await r.json()) as { status: GhStatus }).status
    },
  })
  const [mode, setMode] = useState<'app' | 'pat' | ''>('')
  const [pat, setPat] = useState('')
  const [appId, setAppId] = useState('')
  const [privateKey, setPrivateKey] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const effMode = mode || status?.mode || ''

  const save = async (body: unknown) => {
    setBusy(true)
    setError(null)
    const r = await fetch('/api/workbench/github', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    setBusy(false)
    if (!r.ok) setError(((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed')
    await qc.invalidateQueries({ queryKey: ['github-status'] })
    await qc.invalidateQueries({ queryKey: ['github-installations'] })
  }
  const disconnect = async () => {
    if (!(await confirm({ title: 'Disconnect GitHub', message: 'Agents lose workbench repo access until reconnected.', confirmLabel: 'Disconnect', danger: true }))) return
    await fetch('/api/workbench/github', { method: 'DELETE', credentials: 'same-origin' })
    await qc.invalidateQueries({ queryKey: ['github-status'] })
  }

  const { data: installations = [] } = useQuery({
    queryKey: ['github-installations'],
    enabled: status?.mode === 'app' && !!status.app.keySet,
    queryFn: async (): Promise<Array<{ id: number; account: string }>> => {
      const r = await fetch('/api/workbench/github?installations=1', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { installations: Array<{ id: number; account: string }> }).installations
    },
  })

  if (isPending) {
    return (
      <Panel className="mt-4">
        <Skeleton className="mb-3 h-4 w-24 rounded-full" />
        <SkeletonRows rows={2} />
      </Panel>
    )
  }
  return (
    <Panel className="mt-4">
      <SectionHeader
        title="GitHub · Workbench"
        info="Lets dev-leaning agents work real repositories through their sandboxed workbench. Connect once here; which agent may touch which repo stays an explicit per-agent grant on the agent's manage view."
      />
      <div className="space-y-3">
        {status?.configured && (
          <div className="flex items-center gap-2 text-sm">
            <span className="h-2 w-2 rounded-full bg-[color:var(--theme-success)]" />
            <span className="text-fg">Connected{status.account ? ` as ${status.account}` : ''}</span>
            <span className="text-xs text-muted">({status.mode === 'app' ? 'GitHub App' : 'personal access token'})</span>
            <button type="button" onClick={() => void disconnect()} className="ml-auto text-xs text-muted transition-colors hover:text-[color:var(--theme-danger)]">
              Disconnect
            </button>
          </div>
        )}
        {status?.error && <div className="text-xs text-[color:var(--theme-warning)]">{status.error}</div>}

        <div className="flex items-center gap-3">
          <label className="w-24 shrink-0 text-[11px] uppercase tracking-wide text-muted">Connect via</label>
          <Select size="sm" value={effMode} onChange={(e) => setMode(e.target.value as 'app' | 'pat' | '')}>
            <option value="">choose…</option>
            <option value="app">GitHub App (recommended)</option>
            <option value="pat">Personal access token</option>
          </Select>
        </div>

        {effMode === 'pat' && (
          <div className="space-y-2 rounded-xl border border-line-subtle bg-card/40 px-4 py-3 text-sm">
            <p className="text-xs text-muted">
              1 · Create a <span className="text-fg">fine-grained token</span> at{' '}
              <a href="https://github.com/settings/personal-access-tokens/new" target="_blank" rel="noreferrer" className="text-accent hover:underline">
                github.com/settings/personal-access-tokens
              </a>{' '}
              scoped to the repos your agents will work — permissions: <span className="text-fg">Contents (read/write) · Pull requests (read/write) · Metadata (read)</span>.
              <br />2 · Paste it here. It's stored encrypted and never shown again.
            </p>
            <div className="flex items-center gap-2">
              <Input size="sm" type="password" value={pat} onChange={(e) => setPat(e.target.value)} placeholder={status?.patSet ? '••••••••  (set — paste to replace)' : 'github_pat_…'} className="max-w-sm" />
              <Button size="sm" disabled={!pat.trim() || busy} onClick={() => void save({ mode: 'pat', pat: { token: pat.trim() } }).then(() => setPat(''))}>
                Connect
              </Button>
            </div>
          </div>
        )}

        {effMode === 'app' && (
          <div className="space-y-2 rounded-xl border border-line-subtle bg-card/40 px-4 py-3 text-sm">
            <p className="text-xs text-muted">
              1 · Create a GitHub App at{' '}
              <a href="https://github.com/settings/apps/new" target="_blank" rel="noreferrer" className="text-accent hover:underline">
                github.com/settings/apps/new
              </a>{' '}
              (use your org's settings if the repos live there) — repository permissions: <span className="text-fg">Contents (read/write) · Pull requests (read/write) · Metadata (read)</span>; no webhook needed.
              <br />2 · Copy the <span className="text-fg">App ID</span>, then generate a <span className="text-fg">private key</span> and paste both here.
              <br />3 · Install the app on the repos your agents will work, then pick the installation below.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <Input size="sm" value={appId} onChange={(e) => setAppId(e.target.value)} placeholder={status?.app.appId ? `App ID ${status.app.appId}` : 'App ID'} className="w-32" />
              <Textarea rows={2} value={privateKey} onChange={(e) => setPrivateKey(e.target.value)} placeholder={status?.app.keySet ? 'private key set — paste to replace' : '-----BEGIN RSA PRIVATE KEY-----'} className="max-h-20 min-w-64 flex-1 font-mono text-xs" />
              <Button
                size="sm"
                disabled={busy || (!appId.trim() && !privateKey.trim())}
                onClick={() =>
                  void save({ mode: 'app', app: { ...(appId.trim() ? { appId: appId.trim() } : {}), ...(privateKey.trim() ? { privateKey: privateKey.trim() } : {}) } }).then(() => {
                    setAppId('')
                    setPrivateKey('')
                  })
                }
              >
                Save
              </Button>
            </div>
            {status?.app.keySet && (
              <div className="flex items-center gap-2">
                <label className="text-[11px] uppercase tracking-wide text-muted">Installation</label>
                <Select size="sm" value={status.app.installationId} onChange={(e) => void save({ mode: 'app', app: { installationId: e.target.value } })}>
                  <option value="">pick where it's installed…</option>
                  {installations.map((i) => (
                    <option key={i.id} value={String(i.id)}>
                      {i.account} (#{i.id})
                    </option>
                  ))}
                </Select>
                {installations.length === 0 && <span className="text-xs text-muted">none found yet — install the app on GitHub first</span>}
              </div>
            )}
          </div>
        )}
        {error && <div className="text-xs text-[color:var(--theme-danger)]">{error}</div>}
        {status?.configured && <RepoFlowSection />}
      </div>
    </Panel>
  )
}

/** Per-repo git flow: which branch PRs target, and the optional testing
 *  branch features merge into for integration testing. */
function RepoFlowSection() {
  const qc = useQueryClient()
  interface FlowData {
    flows: Array<{ repo: string; baseBranch: string | null; testingBranch: string | null }>
    repos: string[]
  }
  const { data } = useQuery({
    queryKey: ['workbench-flow'],
    queryFn: async (): Promise<FlowData | null> => {
      const r = await fetch('/api/workbench/flow', { credentials: 'same-origin' })
      if (!r.ok) return null
      return (await r.json()) as FlowData
    },
  })
  const [repo, setRepo] = useState('')
  const [base, setBase] = useState('')
  const [testing, setTesting] = useState('')

  const save = async (body: { repo: string; baseBranch?: string | null; testingBranch?: string | null }) => {
    await fetch('/api/workbench/flow', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
  if (!data) return null
  const configured = new Set(data.flows.map((f) => f.repo))
  return (
    <div className="space-y-2 border-t border-line-subtle pt-3">
      <div className="text-[11px] uppercase tracking-wide text-muted">Repository flow</div>
      <p className="text-xs text-muted">
        Where workbench PRs land per repo (blank = the repo's default branch), and an optional testing branch features can be merged into
        before the PR ships. Unlisted repos use their defaults.
      </p>
      {data.flows.map((f) => (
        <div key={f.repo} className="flex flex-wrap items-center gap-2 text-sm">
          <span className="w-56 truncate text-fg">{f.repo}</span>
          <Input
            size="sm"
            defaultValue={f.baseBranch ?? ''}
            placeholder="PRs → default"
            className="w-36"
            onBlur={(e) => e.target.value.trim() !== (f.baseBranch ?? '') && void save({ repo: f.repo, baseBranch: e.target.value.trim() || null, testingBranch: f.testingBranch })}
          />
          <Input
            size="sm"
            defaultValue={f.testingBranch ?? ''}
            placeholder="testing branch (off)"
            className="w-40"
            onBlur={(e) => e.target.value.trim() !== (f.testingBranch ?? '') && void save({ repo: f.repo, baseBranch: f.baseBranch, testingBranch: e.target.value.trim() || null })}
          />
        </div>
      ))}
      <div className="flex flex-wrap items-center gap-2">
        <Select size="sm" value={repo} onChange={(e) => setRepo(e.target.value)} className="w-56">
          <option value="">configure a repo…</option>
          {data.repos.filter((r) => !configured.has(r)).map((r) => (
            <option key={r} value={r}>{r}</option>
          ))}
        </Select>
        <Input size="sm" value={base} onChange={(e) => setBase(e.target.value)} placeholder="PR base (blank = default)" className="w-44" />
        <Input size="sm" value={testing} onChange={(e) => setTesting(e.target.value)} placeholder="testing branch" className="w-40" />
        <Button
          size="sm"
          variant="outline"
          disabled={!repo}
          onClick={() => void save({ repo, baseBranch: base.trim() || null, testingBranch: testing.trim() || null }).then(() => { setRepo(''); setBase(''); setTesting('') })}
        >
          Add
        </Button>
      </div>
    </div>
  )
}
