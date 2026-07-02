import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { Combobox } from '@/components/ui/combobox'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { Select } from '@/components/ui/select'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { relativeTime } from '@/lib/fleet'

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

  const update = async (userId: string, patch: { role?: 'admin' | 'member'; agentModels?: string[] }) => {
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

        <Panel className="p-5">
          <div className="mb-1 text-sm font-semibold text-fg">People</div>
          <p className="mb-3 text-xs text-muted">
            Roles and per-person agent access. An empty agent list means all agents; picking any restricts
            that person to exactly those.
          </p>
          {error && (
            <div className="mb-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
              {error}
            </div>
          )}
          <ul className="divide-y divide-line-subtle">
            {(users ?? []).map((u) => (
              <li key={u.id} className="flex items-center gap-3 py-3">
                <Avatar name={u.name ?? u.email} className="h-7 w-7" />
                <span className="w-56 min-w-0">
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
                <Combobox
                  options={agentOptions}
                  selected={u.agentModels}
                  onChange={(models) => void update(u.id, { agentModels: models })}
                  multiple
                  size="sm"
                  placeholder="All agents"
                  className="min-w-0 flex-1"
                />
                <span className="w-20 shrink-0 text-right text-xs text-muted">{relativeTime(u.lastSeenAt)}</span>
              </li>
            ))}
          </ul>
        </Panel>
      </div>
    </div>
  )
}
