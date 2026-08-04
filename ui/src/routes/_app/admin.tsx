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
import { Modal } from '@/components/ui/modal'
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
import { focusGold } from '@/components/chat/chat-chrome'
import { CopyButton } from '@/components/ui/copy-link-button'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { listQuery, QueryError } from '@/components/ui/query-state'
import { getJson, getList } from '@/lib/fetch-json'
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
    // An admin console that renders an EMPTY People list because /api/admin/users
    // 500'd is telling an owner their org has no members. Non-2xx throws.
    queryKey: ['admin-users'],
    queryFn: (): Promise<AdminUser[]> => getList<AdminUser>('/api/admin/users', 'users'),
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
    // Every permission chip on this page is gated on `perms` being truthy, so a
    // failed read used to make the whole permissions model DISAPPEAR — an admin
    // reads that as "nothing is restricted here". Non-2xx throws.
    queryKey: ['admin-permissions'],
    queryFn: (): Promise<PermsData> => getJson<PermsData>('/api/admin/permissions'),
  })
}

/** The one shape of `/api/admin/settings`, declared ONCE.
 *
 *  Three components read this endpoint under the single `['admin-settings']`
 *  cache key (two here, `MemberAccessPanel` in models.tsx) and each used to
 *  declare its own narrower return type over that one entry — TypeScript
 *  happily believed three different contracts for the same cached object, so
 *  the day one of them started fetching something else nothing would have
 *  complained. One type, one hook. */
interface AdminSettings {
  auditRetentionDays: number
  org: { name: string; about: string }
  memberModels: string[]
}
function useAdminSettings() {
  return useQuery({
    queryKey: ['admin-settings'],
    queryFn: (): Promise<AdminSettings> => getJson<AdminSettings>('/api/admin/settings'),
  })
}

// The admin console: people, their roles, and which agents each may use.
function AdminPage() {
  const qc = useQueryClient()
  const { data: me } = useSession()
  const usersQuery = useAdminUsers()
  const { data: users, isPending: usersPending } = usersQuery
  const permsQuery = useAdminPermissions()
  const { data: perms } = permsQuery
  const { data: fleet } = useAgents()
  const agentOptions = (fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))
  // Enabled apps join the per-person view checklist as EXPLICIT grants —
  // every app view (work and manage) defaults denied for members; an admin
  // adds each one per person, same storage as core Manage views.
  // This one is not cosmetic. `allowedManageViews` is written back WHOLESALE
  // from `manageViews` below, so with the app list defaulted to `[]` a failed
  // /api/apps meant that toggling any unrelated view silently REVOKED every
  // app manage grant the person had — a permissions write derived from a read
  // that never happened.
  const appsList = listQuery(useEnabledApps(), { title: 'Could not load installed apps', variant: 'inline' })
  const enabledApps = appsList.rows
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
        <h1 className="text-2xl font-semibold tracking-tight text-fg">Admin</h1>

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
        {tab === 'people' &&
          (perms ? (
            <MemberDefaultsPanel perms={perms} />
          ) : permsQuery.isError ? (
            // Silence here reads as "no permission model configured". Say so.
            <Panel>
              <QueryError
                variant="compact"
                error={permsQuery.error}
                title="Could not load member permissions"
                onRetry={() => void permsQuery.refetch()}
              />
            </Panel>
          ) : null)}
        {tab === 'people' && (
        <Panel>
          <SectionHeader
            className="mb-4"
            title="People"
            info="Roles, per-person agent access, and which views each member can reach. Empty = all (open by default); pick any to restrict. Admins always have full access."
          />
          {error && (
            <div className="mb-2 text-xs text-danger">
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
          ) : !users ? (
            // The most dangerous empty render on the whole surface: an admin
            // seeing no rows concludes the org is empty (or that a person they
            // just off-boarded is gone) when the read simply failed.
            <QueryError
              variant="compact"
              error={usersQuery.error}
              title="Could not load your people"
              onRetry={() => void usersQuery.refetch()}
            />
          ) : (
          <ul className="divide-y divide-line-subtle">
            {users.map((u) => {
              // One ALLOW list spanning both worlds: work views default in
              // (denials stored), Manage views default out (allows stored).
              const allowedViews = [
                ...workViews.filter((v) => !u.deniedViews.includes(v.to)).map((v) => v.to),
                ...manageViews.filter((v) => u.allowedManageViews.includes(v.to)).map((v) => v.to),
              ]
              return (
                <li key={u.id} className="-mx-2 space-y-2 rounded-md px-2 py-3 transition-colors hover:bg-hover">
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
                    <span className="w-16 shrink-0 text-right font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(u.lastSeenAt)}</span>
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
                          // Editing writes the whole allow-list back. While the
                          // app views are missing, that write would drop them.
                          disabled={appsList.failed}
                          placeholder="Work views"
                          className="min-w-0 flex-1"
                        />
                      </div>
                      {appsList.notice && <div className="pl-10">{appsList.notice}</div>}
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
        'flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
        focusGold,
        effective
          ? 'border-accent/60 bg-accent/10 text-fg'
          : 'border-line-subtle text-muted hover:border-line hover:text-fg',
      )}
    >
      {entry.label}
      {overridden && <span className="h-1 w-1 rounded-full bg-warning" title="overridden" />}
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
      <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">can</span>
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
            <span className="w-16 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{group}</span>
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
  // `null` is a real answer here — no hosting domain configured yet, and the
  // form below invites you to set one. A FAILED read used to produce the same
  // null, so a blip showed a configured instance its "set a domain" form and a
  // Save would have rewritten live OAuth callbacks. Now only the server's own
  // `instance: null` gets there; a non-2xx becomes an error.
  const query = useQuery({
    queryKey: ['instance-domain'],
    queryFn: async (): Promise<{ domain: string; verified: boolean; verifiedAt: string | null } | null> =>
      (await getJson<{ instance: { domain: string; verified: boolean; verifiedAt: string | null } | null }>('/api/admin/instance')).instance,
  })
  const { data, isPending } = query
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
      ) : data === undefined ? (
        <QueryError
          variant="inline"
          error={query.error}
          title="Could not load the instance domain"
          onRetry={() => void query.refetch()}
        />
      ) : data ? (
        <div className="flex items-center gap-2">
          <span className="font-mono text-[13px] text-fg">{data.domain}</span>
          {data.verified ? (
            <span className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-success">
              <span aria-hidden className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" />
              verified — routes to this instance
            </span>
          ) : (
            <span className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-warning">
              <span aria-hidden className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning" />
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
            className="text-muted transition-colors hover:text-danger"
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
        <div className="mt-2 text-xs text-danger">
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
  const query = useQuery({
    queryKey: ['email-config'],
    queryFn: async (): Promise<EmailCfg> => (await getJson<{ config: EmailCfg }>('/api/admin/email')).config,
  })
  const { data, isPending } = query
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

  if (isPending) {
    return (
      <Panel className="mt-4">
        <Skeleton className="mb-3 h-4 w-24 rounded-full" />
        <SkeletonRows rows={2} />
      </Panel>
    )
  }
  // `isPending || !data` used to send a FAILED read back to the skeleton, so a
  // broken /api/admin/email shimmered for ever with nothing to click.
  if (!data) {
    return (
      <Panel className="mt-4">
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load your email settings"
          onRetry={() => void query.refetch()}
        />
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
          <label className="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Provider</label>
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
            <label className="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">From</label>
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
              <label className="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Host / port</label>
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
              <label className="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">User</label>
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
            <label className="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">API key</label>
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
            {notice && <span className="text-xs text-success">{notice}</span>}
          </div>
        )}
        {error && <div className="text-xs text-danger">{error}</div>}
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
  // "No invites yet." is a claim about who has been let in. A failed read must
  // not make it — an admin would re-invite people who already hold live links,
  // or believe a revoked invite is gone when it is still open.
  const query = useQuery({
    queryKey: ['invites'],
    queryFn: (): Promise<InviteRow[]> => getList<InviteRow>('/api/admin/invites', 'invites'),
  })
  const { data, isPending } = query
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
      {error && <div className="mb-2 text-xs text-danger">{error}</div>}
      {isPending ? (
        <SkeletonRows rows={2} />
      ) : !data ? (
        <QueryError variant="inline" error={query.error} title="Could not load invites" onRetry={() => void query.refetch()} />
      ) : data.length === 0 ? (
        <EmptyState variant="inline" title="No invites yet." className="font-sans" />
      ) : (
        <ul className="divide-y divide-line-subtle">
          {data.map((i) => {
            const st = state(i)
            return (
              <li key={i.id} className="-mx-2 flex items-center gap-3 rounded-md px-2 py-2 text-sm transition-colors hover:bg-hover">
                <span className="min-w-0 flex-1 truncate text-fg">{i.email}</span>
                <span
                  className={cn(
                    'shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em]',
                    st === 'accepted' && 'border-success/40 text-success',
                    st === 'pending' && 'border-line text-muted',
                    (st === 'revoked' || st === 'expired') && 'border-line-subtle text-muted/60 line-through',
                  )}
                >
                  {st}
                </span>
                <span className="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(i.createdAt)}</span>
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
                    className="shrink-0 text-muted transition-colors hover:text-danger"
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
  // An empty list here says "nobody can self-join". Only the server may say
  // that: a failed read used to hide a VERIFIED domain, and the panel then
  // invited the admin to add one that already exists.
  const query = useQuery({
    queryKey: ['org-domains'],
    queryFn: (): Promise<OrgDomainRow[]> => getList<OrgDomainRow>('/api/admin/domains', 'domains'),
  })
  const { data, isPending } = query
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
      ) : !data ? (
        <QueryError
          variant="inline"
          error={query.error}
          title="Could not load your sign-up domains"
          onRetry={() => void query.refetch()}
        />
      ) : (
        <div className="space-y-2">
          {data.length === 0 && (
            <EmptyState
              variant="inline"
              title="No email domains yet"
              hint="add the domain your team's email addresses use to open self-service joins."
              className="font-sans"
            />
          )}
          {/* No `?? []` fallback: `!data` already forked to the error above, so
              reaching here means the read landed. */}
          {data.map((d) => (
            <div key={d.id} className="space-y-1.5 rounded-md border border-line p-2.5">
              <div className="flex items-center gap-2">
                <span className="font-mono text-[13px] text-fg">{d.domain}</span>
                {d.verified ? (
                  <span className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-success">
                    <span aria-hidden className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" />
                    verified — self-joins open
                  </span>
                ) : (
                  <span className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-warning">
                    <span aria-hidden className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning" />
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
                  className="text-muted transition-colors hover:text-danger"
                >
                  <Trash2 size={13} />
                </button>
              </div>
              {!d.verified && (
                <div className="flex items-center gap-2">
                  <span className="shrink-0 text-[11px] text-muted">TXT on _talaria-verify.{d.domain}:</span>
                  <code className="min-w-0 flex-1 truncate rounded bg-raised px-1.5 py-0.5 font-mono text-[11px] text-fg">{d.verificationToken}</code>
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
            <div className="text-xs text-danger">
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
  const query = useAdminSettings()
  const { data, isPending } = query
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
      <SectionHeader className="mb-1" title="Organization" />
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
      ) : !data ? (
        // A failed read used to render the identity form BLANK. Typing one word
        // into it made it dirty, and Save then wrote an empty business name and
        // description over the real ones — and rolled the whole fleet onto them.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load your organization"
          onRetry={() => void query.refetch()}
        />
      ) : (
      <div className="space-y-3">
        <div>
          <label className="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Business name</label>
          <Input size="sm" value={nameVal} onChange={(e) => setName(e.target.value)} placeholder="e.g. Outcrop Labs" className="max-w-xs" />
        </div>
        <div>
          <label className="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">What the business does</label>
          <Textarea
            rows={2}
            value={aboutVal}
            onChange={(e) => setAbout(e.target.value)}
            placeholder="One or two sentences agents can anchor their mission to."
            className="w-full text-sm"
          />
        </div>
        <div className="flex items-center gap-2">
          {saved && <span className="text-xs text-success">Saved</span>}
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
  const query = useAdminSettings()
  const { data, isPending } = query
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
      <SectionHeader className="mb-2" title="Settings" />
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
      ) : !data ? (
        // An empty retention box reads as "0 = keep forever". Saving from there
        // would silently change the org's audit policy.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load your settings"
          onRetry={() => void query.refetch()}
        />
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
        <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">days</span>
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
  type EncryptionData = {
    keyVersion: number | null
    rotatedAt: string | null
    secretCount: number
    rootSource: string
    algorithm: string
  }
  const query = useQuery({
    queryKey: ['admin-encryption'],
    queryFn: (): Promise<EncryptionData> => getJson<EncryptionData>('/api/admin/encryption'),
  })
  const { data, isPending } = query
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
      ) : !data ? (
        // "Secrets protected: —" over a failed read looks like a fact about the
        // key store rather than a fact about the request.
        <QueryError
          variant="inline"
          error={query.error}
          title="Could not load encryption status"
          onRetry={() => void query.refetch()}
        />
      ) : (
        <div className="flex flex-wrap items-end gap-x-8 gap-y-2">
          <span>
            <span className="block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Key version</span>
            <span className="font-sans text-2xl font-semibold text-fg">v{data.keyVersion ?? '—'}</span>
          </span>
          <span>
            <span className="block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Secrets protected</span>
            <span className="font-sans text-2xl font-semibold text-fg">{data.secretCount}</span>
          </span>
          <span>
            <span className="block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Root of trust</span>
            <span className="font-sans text-2xl font-semibold text-fg">{data.rootSource}</span>
          </span>
          {data.rotatedAt && (
            <span className="pb-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
              rotated {new Date(data.rotatedAt).toLocaleString()}
            </span>
          )}
        </div>
      )}
      {/* §8 DANGER ZONE: orange hairline panel, mono label + IRREVERSIBLE
          meta, muted body, orange-outline action — never an orange fill. */}
      <div className="mt-4 rounded-md border border-danger/40 p-4">
        <div className="mb-2 flex items-center gap-1.5">
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">Danger zone</span>
          <span className="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Irreversible</span>
        </div>
        <p className="mb-3 text-xs text-muted">
          Rotating re-encrypts every stored secret under a fresh key in one pass. The previous key is retired for good.
        </p>
        <div className="flex flex-wrap items-end gap-3">
          <div className="flex-1 min-w-[16rem]">
            <label className="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">New root secret (optional)</label>
            <Input
              type="password"
              value={newRoot}
              onChange={(e) => setNewRoot(e.target.value)}
              placeholder="leave blank to keep the current root"
              autoComplete="off"
            />
          </div>
          <Button size="sm" variant="danger" onClick={() => void rotate()} disabled={busy}>
            {busy ? 'Rotating' : 'Rotate keys'}
          </Button>
        </div>
        {busy && (
          <div className="mt-3">
            <Generating
              label="Rotating the data key: re-encrypting provider keys, agent secrets, and OAuth tokens in one pass"
              lines={2}
            />
          </div>
        )}
        {newRoot.trim() && (
          <p className="mt-2 text-[11px] text-muted">
            After rotating with a new root secret, update <code className="font-mono">TALARIA_SECRET_KEY</code> (or the key-file) to match before the next restart.
          </p>
        )}
      </div>
      {msg && <p className="mt-2 text-xs text-success">{msg}</p>}
    </Panel>
  )
}

function JudgePanel() {
  const qc = useQueryClient()
  type JudgeData = { config: { enabled: boolean; model: string | null; mode?: 'advisory' | 'enforcing' }; models: string[] }
  const query = useQuery({
    queryKey: ['judge-config'],
    queryFn: (): Promise<JudgeData> => getJson<JudgeData>('/api/admin/judge'),
  })
  const { data, isPending } = query
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
      ) : !data ? (
        // The controls seed from the response, so a failed read renders the
        // judge as OFF — and the first click would then save it off for real.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load the judge configuration"
          onRetry={() => void query.refetch()}
        />
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
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model</span>
          <Select size="sm" value={model} onChange={(e) => void save({ model: e.target.value || null })} className="w-64" disabled={!enabled}>
            <option value="">Default (self-hosted)</option>
            {(data?.models ?? []).map((m) => <option key={m} value={m}>{m}</option>)}
            {model && !(data?.models ?? []).includes(model) && <option value={model}>{model}</option>}
          </Select>
        </div>
        {saved && <span className="text-xs text-success">Saved</span>}
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
  const query = useQuery({
    queryKey: ['guardrails'],
    queryFn: (): Promise<GuardData> => getJson<GuardData>('/api/admin/guardrails'),
    refetchInterval: 30_000,
  })
  const { data, isPending } = query
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
      ) : !data ? (
        // "0 findings" and mode=observe are both defaults this component
        // invents when `data` is missing — over a failed read that reads as a
        // guard that is quietly running and finding nothing.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load the guardrail configuration"
          onRetry={() => void query.refetch()}
        />
      ) : (
      <>
      <div className="flex flex-wrap items-center gap-4">
        <div className="flex items-center gap-2">
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Mode</span>
          <Select size="sm" value={cfg?.mode ?? 'observe'} onChange={(e) => void save({ mode: e.target.value })} className="w-40">
            <option value="off">Off</option>
            <option value="observe">Observe</option>
            <option value="annotate">Annotate</option>
            <option value="strict">Strict</option>
          </Select>
        </div>
        <label className="flex items-center gap-2 text-xs text-muted" title="Findings below this confidence are dropped">
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Min confidence</span>
          <input
            type="range" min={0} max={1} step={0.05}
            value={cfg?.minConfidence ?? 0.5}
            disabled={cfg?.mode === 'off'}
            onChange={(e) => void save({ minConfidence: Number(e.target.value) })}
            className="w-28 accent-accent"
          />
          <span className="w-8 font-mono text-[11px] text-fg">{((cfg?.minConfidence ?? 0.5) * 100).toFixed(0)}%</span>
        </label>
        <Checkbox
          className="gap-2"
          title="Repeated findings become templated behavioral notes in the agent's rendered soul (counts + advice only, never the flagged content). Applies on the next fleet render."
          checked={cfg?.coach ?? false}
          disabled={cfg?.mode === 'off'}
          onChange={(checked) => void save({ coach: checked })}
          label="Coach agents from findings"
        />
        <div className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{data?.stats.total ?? 0} findings</div>
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
          <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Recent findings</div>
          <div className="max-h-48 divide-y divide-line-subtle overflow-y-auto rounded-md border border-line">
            {data.findings.slice(0, 20).map((f) => (
              <div key={f.id} className="flex items-start gap-2 px-2 py-1.5 text-xs transition-colors hover:bg-hover">
                <span className={cn('shrink-0 rounded px-1 font-mono text-[10px] uppercase tracking-[0.05em]', f.severity === 'high' ? 'text-danger' : 'text-warning')}>{f.check.replace(/_/g, ' ')}</span>
                <span className="min-w-0 flex-1 truncate text-muted" title={f.snippet}>{f.snippet || f.message}</span>
                <span className="shrink-0 font-mono text-[10px] text-muted">{f.model}</span>
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
  const query = useQuery({
    queryKey: ['admin-outreach'],
    queryFn: (): Promise<OutreachData> => getJson<OutreachData>('/api/admin/outreach'),
    refetchInterval: 30_000,
  })
  const { data, isPending } = query
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
      ) : !data ? (
        // Same shape as the guard panel: an unchecked toggle and an empty agent
        // list are this component's defaults, not the org's actual policy.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load outreach settings"
          onRetry={() => void query.refetch()}
        />
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
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Every</span>
          <Input
            size="sm" type="number" min={15} max={1440} className="w-20"
            value={cfg?.intervalMinutes ?? 240}
            disabled={!cfg?.enabled}
            onChange={(e) => void save({ intervalMinutes: Math.max(15, Math.min(1440, Number(e.target.value) || 240)) })}
          />
          <span>min</span>
        </label>
        <label className="flex items-center gap-2 text-xs text-muted" title="Agent-initiated direct messages allowed per person per day">
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">DM cap</span>
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
          <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Proactive agents</div>
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
                    {a.personal && <span className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">personal</span>}
                  </>
                }
              />
            ))}
          </div>
        </div>
      )}
      {data?.events && data.events.length > 0 && (
        <div className="mt-4">
          <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Recent outreach</div>
          <div className="max-h-48 divide-y divide-line-subtle overflow-y-auto rounded-md border border-line">
            {data.events.map((ev, i) => (
              <div key={i} className="flex items-start gap-2 px-2 py-1.5 text-xs transition-colors hover:bg-hover">
                <span className="shrink-0 rounded px-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{ev.kind}</span>
                <span className="min-w-0 flex-1 truncate text-muted" title={ev.note ?? ''}>{ev.note ?? '—'}</span>
                <span className="shrink-0 font-mono text-[10px] text-muted">{ev.agentModel}</span>
                <span className="shrink-0 font-mono text-[10px] text-muted">{relativeTime(ev.createdAt)}</span>
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
  const query = useQuery({
    queryKey: ['org-google'],
    queryFn: (): Promise<OrgGoogle> => getJson<OrgGoogle>('/api/integrations/google/org'),
  })
  const { data, isPending } = query
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
        <div className="flex items-center gap-3 rounded-md border border-line p-4">
          <Skeleton className="h-9 w-9 shrink-0" />
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-3 w-36 rounded-full" />
            <Skeleton className="h-2.5 w-52 rounded-full" delay={0.12} />
          </div>
          <Skeleton className="h-7 w-24 shrink-0" delay={0.24} />
        </div>
      ) : !data ? (
        // Without this branch a failed status read fell through to the row
        // below, which renders "Not connected" and a Connect button — and
        // reconnecting a connection that is already live re-consents the org.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load the org Google account"
          onRetry={() => void query.refetch()}
        />
      ) : !data.available ? (
        <div className="text-xs text-muted">Google integration isn’t configured on this server.</div>
      ) : (
        <div className="flex items-center gap-3 rounded-md border border-line p-4">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-line bg-raised text-lg">🏢</div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5 text-sm font-medium text-fg">
              Shared Drive & Docs
              {data?.connected && <span aria-hidden className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" />}
            </div>
            <div className="truncate font-mono text-[11px] text-muted">
              {data?.connected ? `Connected${data.email ? ` as ${data.email}` : ''}` : 'Not connected'}
            </div>
          </div>
          {data?.connected ? (
            <Button variant="danger-outline" size="sm" onClick={() => void disconnect()}>Disconnect</Button>
          ) : (
            <a href="/api/integrations/google/org/connect" className={buttonClasses({ size: 'sm' })}>Connect</a>
          )}
        </div>
      )}
      {data?.connected && <OrgGoogleTargets targets={data.targets} />}
      {flash && (
        <div className={cn('mt-3 text-xs', flash === 'connected' ? 'text-success' : 'text-danger')}>
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
    <div className="mt-4 space-y-3 rounded-md border border-line p-4">
      <div className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Where agents build</div>
      <div>
        <label className="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Shared Drive / folder ID</label>
        <Input size="sm" value={drive} onChange={(e) => setDrive(e.target.value)} placeholder="Shared Drive or folder ID" />
        <div className="mt-1 text-[11px] text-muted">Files agents create land here (team-owned). Blank = the account’s My Drive. The org account must be a member of the Shared Drive.</div>
      </div>
      <div>
        <label className="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Calendar ID</label>
        <Input size="sm" value={cal} onChange={(e) => setCal(e.target.value)} placeholder="team@group.calendar.google.com" />
        <div className="mt-1 text-[11px] text-muted">Org events land here. Blank = the account’s primary calendar.</div>
      </div>
      <div>
        <label className="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Send mail as</label>
        <Input size="sm" value={sendAs} onChange={(e) => setSendAs(e.target.value)} placeholder="support@yourdomain.com" />
        <div className="mt-1 text-[11px] text-muted">A verified send-as alias on the org account for outgoing mail. Blank = the account’s own address.</div>
      </div>
      <div className="flex items-center gap-3">
        <Button size="sm" onClick={() => void save()} disabled={!dirty}>Save</Button>
        {saved && <span className="text-xs text-success">Saved</span>}
      </div>
    </div>
  )
}


/** The Workbench's GitHub connection — one calm panel: status, the minimal
 *  connect controls, repo flow. The full field-by-field walkthrough lives in
 *  a setup-guide modal so the panel itself stays readable. */
function GithubPanel() {
  const qc = useQueryClient()
  interface GhStatus {
    mode: 'app' | 'pat' | null
    configured: boolean
    account: string | null
    error: string | null
    app: { appId: string; installationIds: string[]; keySet: boolean }
    patSet: boolean
    repoCreationOrgs?: string[]
  }
  const statusQuery = useQuery({
    queryKey: ['github-status'],
    queryFn: async (): Promise<GhStatus> => (await getJson<{ status: GhStatus }>('/api/workbench/github')).status,
  })
  const { data: status, isPending } = statusQuery
  const [mode, setMode] = useState<'app' | 'pat' | ''>('')
  const [pat, setPat] = useState('')
  const [appId, setAppId] = useState('')
  const [privateKey, setPrivateKey] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [guideOpen, setGuideOpen] = useState(false)
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

  // An empty install list makes the panel say "install the app on GitHub first"
  // — advice that is wrong, and acted on, if the list simply failed to load.
  const installsQuery = useQuery({
    queryKey: ['github-installations'],
    enabled: status?.mode === 'app' && !!status.app.keySet,
    queryFn: (): Promise<Array<{ id: number; account: string }>> =>
      getList<{ id: number; account: string }>('/api/workbench/github?installations=1', 'installations'),
  })
  const installations = installsQuery.data ?? []
  // A failed BACKGROUND refetch keeps the chips it already has — only a failure
  // with nothing behind it turns the row into an error.
  const installsFailed = installsQuery.isError && installsQuery.data === undefined

  if (isPending) {
    return (
      <Panel className="mt-4">
        <Skeleton className="mb-3 h-4 w-24 rounded-full" />
        <SkeletonRows rows={2} />
      </Panel>
    )
  }
  // "Not connected — pick a method" below is a verdict on the stored GitHub
  // credentials. A failed read never delivered one, and following that advice
  // means re-pasting an App key that was already there.
  if (!status) {
    return (
      <Panel className="mt-4">
        <QueryError
          variant="compact"
          error={statusQuery.error}
          title="Could not load the GitHub connection"
          onRetry={() => void statusQuery.refetch()}
        />
      </Panel>
    )
  }
  return (
    <Panel className="mt-4">
      <SectionHeader
        title="GitHub · Workbench"
        info="Lets granted agents work real repositories through their sandboxed workbench. Connect once; which agent may touch which repo stays an explicit per-agent grant."
        action={
          <Button size="sm" variant="outline" onClick={() => setGuideOpen(true)}>
            Setup guide
          </Button>
        }
      />
      <div className="space-y-4">
        {/* Status */}
        {status?.configured ? (
          <div className="flex items-center gap-2 text-sm">
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" />
            <span className="text-fg">Connected{status.account ? ` as ${status.account}` : ''}</span>
            <span className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">via {status.mode === 'app' ? 'GitHub App' : 'access token'}</span>
            <button type="button" onClick={() => void disconnect()} className="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger">
              Disconnect
            </button>
          </div>
        ) : (
          <div className="flex items-center gap-2 text-sm">
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-line" />
            <span className="text-muted">Not connected — pick a method and follow the setup guide.</span>
          </div>
        )}
        {status?.error && <div className="text-xs text-warning">{status.error}</div>}

        {/* Connect controls — minimal; the guide holds the walkthrough */}
        <div className="space-y-2 rounded-md border border-line bg-raised/40 px-4 py-3">
          <div className="flex items-center gap-3">
            <label className="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Method</label>
            <Segmented
              options={[
                { id: 'app' as const, label: 'GitHub App' },
                { id: 'pat' as const, label: 'Access token' },
              ]}
              value={effMode === 'pat' ? 'pat' : 'app'}
              onChange={(m) => setMode(m)}
            />
            {effMode === 'app' && <span className="text-xs text-muted">recommended — short-lived tokens, per-repo installs</span>}
          </div>

          {effMode !== 'pat' ? (
            <>
              <div className="flex items-center gap-3">
                <label className="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">App ID</label>
                <Input size="sm" value={appId} onChange={(e) => setAppId(e.target.value)} placeholder={status?.app.appId || 'e.g. 1234567'} className="w-40" />
                <label className="ml-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Key</label>
                <Input size="sm" type="password" value={privateKey} onChange={(e) => setPrivateKey(e.target.value)} placeholder={status?.app.keySet ? 'set — paste .pem to replace' : 'paste the whole .pem'} className="min-w-0 flex-1" />
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
              <div className="flex items-start gap-3">
                <label className="w-20 shrink-0 pt-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Installed on</label>
                <div className="flex flex-wrap items-center gap-1.5">
                  {installations.map((i) => {
                    const selected = (status?.app.installationIds ?? []).includes(String(i.id))
                    return (
                      <button
                        key={i.id}
                        type="button"
                        disabled={!status?.app.keySet}
                        onClick={() =>
                          void save({
                            mode: 'app',
                            app: {
                              installationIds: selected
                                ? (status?.app.installationIds ?? []).filter((x) => x !== String(i.id))
                                : [...(status?.app.installationIds ?? []), String(i.id)],
                            },
                          })
                        }
                        className={cn(
                          'rounded-md border px-2.5 py-0.5 font-mono text-[11px] transition-colors',
                          focusGold,
                          selected ? 'border-[var(--theme-accent-border)] bg-accent/10 text-fg' : 'border-line-subtle text-muted hover:border-line hover:text-fg',
                        )}
                      >
                        {i.account} (#{i.id})
                      </button>
                    )
                  })}
                  {status?.app.keySet && installsFailed && (
                    <QueryError
                      variant="inline"
                      error={installsQuery.error}
                      title="Could not list installations"
                      onRetry={() => void installsQuery.refetch()}
                    />
                  )}
                  {status?.app.keySet && !installsFailed && installations.length === 0 && (
                    <span className="text-xs text-muted">install the app on GitHub first — the guide's step 3</span>
                  )}
                  {!status?.app.keySet && <span className="text-xs text-muted">save the App ID + key first</span>}
                  {(status?.app.installationIds?.length ?? 0) > 1 && <span className="text-xs text-muted">repos pool across all selected orgs</span>}
                </div>
              </div>
            </>
          ) : (
            <div className="flex items-center gap-3">
              <label className="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Token</label>
              <Input size="sm" type="password" value={pat} onChange={(e) => setPat(e.target.value)} placeholder={status?.patSet ? '••••••••  set — paste to replace' : 'github_pat_…'} className="min-w-0 flex-1" />
              <Button size="sm" disabled={!pat.trim() || busy} onClick={() => void save({ mode: 'pat', pat: { token: pat.trim() } }).then(() => setPat(''))}>
                Connect
              </Button>
            </div>
          )}
          {error && <div className="text-xs text-danger">{error}</div>}
        </div>

        {status?.configured && (
          <>
            <RepoCreationSection status={status} save={save} />
            <RepoFlowSection />
          </>
        )}
      </div>

      <GithubGuideModal open={guideOpen} onClose={() => setGuideOpen(false)} mode={effMode === 'pat' ? 'pat' : 'app'} />
    </Panel>
  )
}

/** The full field-by-field setup walkthrough — out of the panel, into a calm
 *  scrollable modal. Mirrors whichever method is selected. */
function GithubGuideModal({ open, onClose, mode }: { open: boolean; onClose: () => void; mode: 'app' | 'pat' }) {
  const [tab, setTab] = useState<'app' | 'pat'>(mode)
  useEffect(() => setTab(mode), [mode, open])
  return (
    <Modal open={open} onClose={onClose} width="max-w-2xl" title="Connect GitHub — setup guide">
      <div className="space-y-4">
        <Segmented
          options={[
            { id: 'app' as const, label: 'GitHub App (recommended)' },
            { id: 'pat' as const, label: 'Personal access token' },
          ]}
          value={tab}
          onChange={setTab}
        />
        <div className="max-h-[60vh] space-y-4 overflow-y-auto pr-1">
          {tab === 'app' ? (
            <>
              <GhStep n={1} title="Create the App">
                <p className="text-xs text-muted">
                  Open{' '}
                  <a href="https://github.com/settings/apps/new" target="_blank" rel="noreferrer" className="text-accent hover:underline">
                    github.com/settings/apps/new
                  </a>{' '}
                  — or, if the repos belong to an organization, prefer <span className="text-fg">Org settings → Developer settings → GitHub Apps → New GitHub App</span> so the org owns it outright. GitHub's form is long; here is every field that matters (leave anything unlisted at its default):
                </p>
                <GhFields
                  rows={[
                    ['GitHub App name', 'Anything unique — e.g. "Talaria Workbench — Yourco".'],
                    ['Description', 'Optional.'],
                    ['Homepage URL', "Required by GitHub but unused here — your company site or this Talaria instance's address both work."],
                    ['Callback URL', 'Leave blank — Talaria never signs users in through this App.'],
                    ['Request user authorization / Device flow', 'Leave unchecked.'],
                    ['Setup URL / Redirect on update', 'Leave blank / unchecked.'],
                    ['Webhook → Active', 'UNCHECK it. Talaria calls GitHub directly, so no webhook — unchecking also removes the required-URL field.'],
                    ['Repository permissions', 'Contents: Read and write · Pull requests: Read and write. (Metadata: Read-only is set automatically.)'],
                    ['Organization permissions', 'None needed. (Optional: Administration — Read and write, only if you later want agents to request NEW repos with human approval.)'],
                    ['Subscribe to events', 'None.'],
                    ['Where can this app be installed?', 'If the repos live in an ORGANIZATION and the app is created under your personal account, choose "Any account" — "Only on this account" locks installs to the owner. (Already created it? App settings → Advanced → "Make public" flips this; public only means installable elsewhere, nothing is exposed.) Org-owned apps can stay "Only on this account".'],
                  ]}
                />
              </GhStep>
              <GhStep n={2} title="Collect the credentials">
                <p className="text-xs text-muted">
                  After "Create GitHub App": the <span className="text-fg">App ID</span> is at the top of the app's settings page (About section). Then scroll to{' '}
                  <span className="text-fg">Private keys → Generate a private key</span> — it downloads a <span className="text-fg">.pem</span> file; open it in any text editor and paste the whole thing into the panel, BEGIN/END lines included. Both are stored encrypted.
                </p>
              </GhStep>
              <GhStep n={3} title="Install it on your repos">
                <p className="text-xs text-muted">
                  On the app's settings page choose <span className="text-fg">Install App</span> in the sidebar → pick the account or organization (org installs by non-owners become a request an org owner approves) →{' '}
                  <span className="text-fg">"Only select repositories"</span> → choose the repos agents will work (you can add more any time). Then pick that installation in the panel's "Installed on" selector.
                </p>
              </GhStep>
            </>
          ) : (
            <>
              <GhStep n={1} title="Create a fine-grained token">
                <p className="text-xs text-muted">
                  Open{' '}
                  <a href="https://github.com/settings/personal-access-tokens/new" target="_blank" rel="noreferrer" className="text-accent hover:underline">
                    github.com/settings/personal-access-tokens/new
                  </a>{' '}
                  and fill the form like this:
                </p>
                <GhFields
                  rows={[
                    ['Token name', 'Anything recognizable — e.g. "Talaria Workbench".'],
                    ['Resource owner', 'The account or organization that owns the repos your agents will work.'],
                    ['Expiration', 'Your policy — you re-paste a fresh token when it rotates.'],
                    ['Repository access', '"Only select repositories" → pick the repos agents may touch. You can add more later.'],
                    ['Permissions → Repository', 'Contents: Read and write · Pull requests: Read and write. (Metadata: Read-only is added automatically.)'],
                    ['Everything else', 'Leave at its default.'],
                  ]}
                />
              </GhStep>
              <GhStep n={2} title="Paste it in the panel">
                <p className="text-xs text-muted">Stored encrypted; never shown again. The status line confirms who it acts as.</p>
              </GhStep>
            </>
          )}
        </div>
        <div className="flex justify-end">
          <Button size="sm" variant="outline" onClick={onClose}>
            Done
          </Button>
        </div>
      </div>
    </Modal>
  )
}


/** One numbered setup step — a small circle, a title, then its content. */
function GhStep({ n, title, children }: { n: number; title: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-3">
      <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-[var(--theme-accent-border)] font-mono text-[10px] text-accent">{n}</span>
      <div className="min-w-0 flex-1 space-y-2">
        <div className="text-sm font-medium text-fg">{title}</div>
        {children}
      </div>
    </div>
  )
}

/** Field-by-field form guidance: GitHub's field name → exactly what to enter. */
function GhFields({ rows }: { rows: Array<[string, string]> }) {
  return (
    <div className="divide-y divide-line-subtle rounded-md border border-line bg-raised/40">
      {rows.map(([label, value]) => (
        <div key={label} className="flex items-baseline gap-3 px-3 py-1.5">
          <span className="w-52 shrink-0 text-xs text-muted">{label}</span>
          <span className="min-w-0 flex-1 text-xs text-fg">{value}</span>
        </div>
      ))}
    </div>
  )
}

/** Agent repo creation: which orgs allow it, and the pending request queue
 *  (agents propose, THIS is where humans ratify — approval creates the repo
 *  and grants it to the requester). */
function RepoCreationSection({
  status,
  save,
}: {
  status: { repoCreationOrgs?: string[] }
  save: (body: unknown) => Promise<void>
}) {
  const qc = useQueryClient()
  const cfgOrgs = status.repoCreationOrgs ?? []
  const [orgs, setOrgs] = useState(cfgOrgs.join(', '))
  interface RepoReq {
    id: string
    agentModel: string
    org: string
    name: string
    why: string
    createdAt: string
  }
  // This is the humans-ratify queue. An empty render means "nothing waiting on
  // you" — a failed poll must never say that, or an agent's repo request sits
  // unseen for ever.
  const requestsQuery = useQuery({
    queryKey: ['repo-requests'],
    queryFn: (): Promise<RepoReq[]> => getList<RepoReq>('/api/workbench/repo-requests', 'requests'),
    refetchInterval: 60_000,
  })
  const requests = requestsQuery.data ?? []
  const requestsFailed = requestsQuery.isError && requestsQuery.data === undefined
  const [decideError, setDecideError] = useState<string | null>(null)
  const decide = async (id: string, action: 'approve' | 'reject') => {
    setDecideError(null)
    const r = await fetch('/api/workbench/repo-requests', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id, action }),
    })
    if (!r.ok) setDecideError(((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed')
    await qc.invalidateQueries({ queryKey: ['repo-requests'] })
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
  return (
    <div className="space-y-2 border-t border-line-subtle pt-3">
      <div className="flex items-baseline gap-2">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent repo creation</span>
        <span className="text-xs text-muted">agents may REQUEST new repos in these orgs; you approve each one</span>
      </div>
      <div className="flex items-center gap-2">
        <Input
          size="sm"
          value={orgs}
          onChange={(e) => setOrgs(e.target.value)}
          onBlur={() => {
            const list = orgs.split(',').map((o) => o.trim()).filter(Boolean)
            if (list.join(',') !== cfgOrgs.join(',')) void save({ repoCreationOrgs: list })
          }}
          placeholder="approved orgs, comma-separated (empty = off)"
          className="max-w-md"
        />
        <span className="text-xs text-muted">needs the App's org Administration permission to create</span>
      </div>
      {requestsFailed && (
        <QueryError
          variant="inline"
          error={requestsQuery.error}
          title="Could not load pending repo requests"
          onRetry={() => void requestsQuery.refetch()}
        />
      )}
      {requests.map((r) => (
        <div key={r.id} className="flex items-center gap-2 rounded-md border border-warning/30 px-3 py-2 text-sm">
          <span className="min-w-0 flex-1 truncate">
            <span className="font-mono text-[13px] text-fg">{r.org}/{r.name}</span>
            <span className="text-muted"> — {r.agentModel}: {r.why}</span>
          </span>
          <Button size="sm" onClick={() => void decide(r.id, 'approve')}>Create + grant</Button>
          <Button size="sm" variant="ghost" onClick={() => void decide(r.id, 'reject')}>Reject</Button>
        </div>
      ))}
      {decideError && <div className="text-xs text-danger">{decideError}</div>}
    </div>
  )
}

/** Per-repo git flow — every reachable repo is a row; blank fields mean the
 *  defaults, so there is no separate "add" ceremony to learn. */
function RepoFlowSection() {
  const qc = useQueryClient()
  interface FlowData {
    flows: Array<{ repo: string; baseBranch: string | null; testingBranch: string | null }>
    repos: string[]
  }
  const query = useQuery({
    queryKey: ['workbench-flow'],
    queryFn: (): Promise<FlowData> => getJson<FlowData>('/api/workbench/flow'),
  })
  const { data } = query
  const { saved, flash } = useSavedFlash()

  const save = async (repo: string, patch: { baseBranch?: string | null; testingBranch?: string | null }) => {
    const cur = data?.flows.find((f) => f.repo === repo)
    await fetch('/api/workbench/flow', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        repo,
        baseBranch: patch.baseBranch !== undefined ? patch.baseBranch : (cur?.baseBranch ?? null),
        testingBranch: patch.testingBranch !== undefined ? patch.testingBranch : (cur?.testingBranch ?? null),
      }),
    })
    flash()
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
  // A failed read used to REMOVE the whole section, so per-repo base and
  // testing branches looked unconfigured — and re-entering them would have
  // overwritten the real flow.
  if (!data)
    return !query.isError ? null : (
      <div className="border-t border-line-subtle pt-3">
        <QueryError
          variant="inline"
          error={query.error}
          title="Could not load repository flow"
          onRetry={() => void query.refetch()}
        />
      </div>
    )
  // Every reachable repo is a row; configs for repos the connection lost
  // access to still show (flagged) so they can be understood and cleared.
  const rows = [...new Set([...data.repos, ...data.flows.map((f) => f.repo)])].sort()
  return (
    <div className="space-y-2 border-t border-line-subtle pt-3">
      <div className="flex items-baseline gap-2">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Repository flow</span>
        <span className="text-xs text-muted">how workbench branches and PRs move, per repo</span>
        {saved && <span className="ml-auto text-xs text-success">Saved</span>}
      </div>
      <div className="overflow-x-auto">
        <div className="min-w-[36rem]">
          <div className="grid grid-cols-[minmax(0,1fr)_11rem_11rem] gap-2 pb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            <span>Repository</span>
            <span>PRs land on</span>
            <span>Testing branch</span>
          </div>
          <div className="space-y-1.5">
            {rows.map((repo) => {
              const f = data.flows.find((x) => x.repo === repo)
              const unreachable = !data.repos.includes(repo)
              return (
                <div key={repo} className="grid grid-cols-[minmax(0,1fr)_11rem_11rem] items-center gap-2">
                  <span className="min-w-0 truncate font-mono text-xs text-fg">
                    {repo}
                    {unreachable && <span className="ml-2 font-sans text-xs text-warning">no longer reachable</span>}
                  </span>
                  <Input
                    size="sm"
                    defaultValue={f?.baseBranch ?? ''}
                    placeholder="default branch"
                    onBlur={(e) => e.target.value.trim() !== (f?.baseBranch ?? '') && void save(repo, { baseBranch: e.target.value.trim() || null })}
                  />
                  <Input
                    size="sm"
                    defaultValue={f?.testingBranch ?? ''}
                    placeholder="none"
                    onBlur={(e) => e.target.value.trim() !== (f?.testingBranch ?? '') && void save(repo, { testingBranch: e.target.value.trim() || null })}
                  />
                </div>
              )
            })}
            {rows.length === 0 && <p className="text-xs text-muted">No repositories reachable yet — finish the install on GitHub.</p>}
          </div>
        </div>
      </div>
      <p className="text-[11px] text-muted">
        <span className="text-fg">PRs land on</span> — the branch workbench jobs cut from and pull requests target; blank uses the repo's default branch.{' '}
        <span className="text-fg">Testing branch</span> — optional integration branch a feature can be merged into (from the ticket or by the agent) before its PR ships; blank disables it. Testing merges never replace review — the PR still lands normally.
      </p>
    </div>
  )
}
