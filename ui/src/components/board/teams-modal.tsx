import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Modal } from '@/components/ui/modal'
import { InlineCreate } from '@/components/ui/inline-create'
import { Select } from '@/components/ui/select'
import { Avatar } from '@/components/ui/avatar'
import { SkeletonRows } from '@/components/ui/skeleton'
import { UserPicker } from '@/components/app/user-picker'
import { cn } from '@/lib/cn'
import { addTeamMember, createTeam, removeTeamMember, useTeamMembers, useTeams, type TeamRole } from '@/lib/teams'

// Create teams and manage their members. Team members can access all of a team's
// boards.
export function TeamsModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient()
  const { data: teams = [], isLoading: teamsLoading } = useTeams()
  const [selected, setSelected] = useState<string | null>(null)

  const team = teams.find((t) => t.id === selected) ?? teams[0] ?? null
  const activeId = team?.id ?? null

  const create = async (name: string) => {
    const { team: t } = await createTeam(name)
    await qc.invalidateQueries({ queryKey: ['teams'] })
    setSelected(t.id)
  }

  return (
    <Modal open={open} onClose={onClose} title="Teams" width="max-w-2xl">
      <div className="flex min-h-[24rem] gap-7">
        {/* Team list */}
        <div className="flex w-60 shrink-0 flex-col border-r border-line-subtle pr-6">
          <div className="space-y-1.5">
            {teamsLoading && <SkeletonRows rows={3} className="px-2 py-1.5" />}
            {teams.map((t) => (
              <button
                key={t.id}
                onClick={() => setSelected(t.id)}
                className={cn(
                  'flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
                  t.id === activeId ? 'bg-card text-fg' : 'text-muted',
                )}
              >
                <span className="min-w-0 flex-1 truncate">{t.name}</span>
                <span className="text-xs text-muted">{t.memberCount}</span>
              </button>
            ))}
          </div>
          <InlineCreate label="New team" placeholder="New team" onSubmit={(n) => void create(n)} className="mt-auto pt-3" />
        </div>

        {/* Members of the selected team */}
        <div className="min-w-0 flex-1">
          {team ? (
            <TeamMembers teamId={activeId!} canManage={team.role === 'owner'} />
          ) : teamsLoading ? (
            <SkeletonRows rows={3} avatar className="pt-1" />
          ) : (
            <div className="grid h-40 place-items-center text-sm text-muted">Create a team to get started.</div>
          )}
        </div>
      </div>
    </Modal>
  )
}

function TeamMembers({ teamId, canManage }: { teamId: string; canManage: boolean }) {
  const qc = useQueryClient()
  const { data: members = [], isLoading: membersLoading } = useTeamMembers(teamId)
  const [role, setRole] = useState<TeamRole>('member')
  const [err, setErr] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['team-members', teamId] })

  const add = async (email: string) => {
    setErr(null)
    const res = await addTeamMember(teamId, email, role)
    const data = (await res.json().catch(() => ({}))) as { error?: string }
    if (!res.ok || data.error) return setErr(data.error ?? 'Could not add')
    refresh()
    void qc.invalidateQueries({ queryKey: ['teams'] })
  }

  return (
    <div>
      {canManage && (
        <div className="flex items-center gap-2">
          <UserPicker
            className="min-w-0 flex-1"
            size="sm"
            exclude={members.map((m) => m.userId)}
            onPick={(u) => {
              if (u.email) void add(u.email)
            }}
          />
          <Select value={role} onChange={(e) => setRole(e.target.value as TeamRole)} size="sm" className="shrink-0">
            <option value="member">Member</option>
            <option value="owner">Owner</option>
          </Select>
        </div>
      )}
      {err && <div className="mt-1 text-xs" style={{ color: 'var(--theme-danger)' }}>{err}</div>}
      {membersLoading && <SkeletonRows rows={3} avatar className="mt-4 px-1 py-2" />}
      <ul className="mt-4 space-y-1.5">
        {members.map((m) => (
          <li key={m.userId} className="flex items-center gap-2 rounded-lg px-1 py-2">
            <Avatar name={m.name ?? m.email} className="h-6 w-6" />
            <span className="min-w-0 flex-1 truncate text-sm text-fg">
              {m.name ?? m.email ?? m.userId}
              {m.name && m.email && <span className="ml-1.5 text-xs text-muted">{m.email}</span>}
            </span>
            <span className="text-xs text-muted">{m.role}</span>
            {canManage && m.role !== 'owner' && (
              <button onClick={() => void removeTeamMember(teamId, m.userId).then(refresh)} className="text-xs text-muted hover:text-[color:var(--theme-danger)]">
                Remove
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  )
}
