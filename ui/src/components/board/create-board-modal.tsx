import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Modal } from '@/components/ui/modal'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Button } from '@/components/ui/button'
import { Combobox } from '@/components/ui/combobox'
import { Skeleton } from '@/components/ui/skeleton'
import { UserPicker } from '@/components/app/user-picker'
import { useAgents } from '@/lib/agents'
import { useTeams } from '@/lib/teams'
import { createBoard, setBoardAgents, shareBoard } from '@/lib/boards'

type Invite = { email: string; role: 'editor' | 'viewer' }

// Create a board and configure everything up front: owner (personal/team),
// which agents may work it (restrictive by default — opt into "all agents"),
// and who to invite.
export function CreateBoardModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient()
  const navigate = useNavigate()
  const { data: fleet, isLoading: fleetLoading } = useAgents()
  const { data: teams = [], isLoading: teamsLoading } = useTeams()
  const agentOptions = (fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))

  const [name, setName] = useState('')
  const [teamId, setTeamId] = useState('')
  const [allowAll, setAllowAll] = useState(false)
  const [agents, setAgents] = useState<string[]>([])
  const [invites, setInvites] = useState<Invite[]>([])
  const [role, setRole] = useState<'editor' | 'viewer'>('editor')
  const [busy, setBusy] = useState(false)

  const close = () => {
    setName('')
    setTeamId('')
    setAllowAll(false)
    setAgents([])
    setInvites([])
    onClose()
  }

  const addInvite = (email: string) => {
    const e = email.trim().toLowerCase()
    if (!e || invites.some((i) => i.email === e)) return
    setInvites((prev) => [...prev, { email: e, role }])
  }

  const create = async () => {
    const n = name.trim()
    if (!n) return
    setBusy(true)
    try {
      const { board } = await createBoard(n, teamId || null)
      await setBoardAgents(board.id, allowAll, allowAll ? [] : agents)
      for (const inv of invites) await shareBoard(board.id, inv.email, inv.role).catch(() => {})
      await qc.invalidateQueries({ queryKey: ['boards'] })
      close()
      void navigate({ to: '/boards/$boardId', params: { boardId: board.id } })
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      open={open}
      onClose={close}
      title="New board"
      width="max-w-lg"
      footer={
        <div className="flex items-center justify-between">
          <span className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{allowAll ? 'All agents allowed' : `${agents.length} agents`}</span>
          <div className="flex gap-2">
            <Button variant="ghost" size="sm" onClick={close}>Cancel</Button>
            <Button size="sm" onClick={() => void create()} disabled={busy || !name.trim()}>Create board</Button>
          </div>
        </div>
      }
    >
      <div className="space-y-4">
        <Field label="Name">
          <Input autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && create()} placeholder="e.g. Q3 Launch" className="w-full" />
        </Field>

        <Field label="Owner">
          {teamsLoading ? (
            <Skeleton className="h-11 w-full" />
          ) : (
            <Select value={teamId} onChange={(e) => setTeamId(e.target.value)} className="w-full">
              <option value="">Personal</option>
              {teams.map((t) => (
                <option key={t.id} value={t.id}>{t.name}</option>
              ))}
            </Select>
          )}
        </Field>

        <Field label="Agents">
          <label className="mb-2 flex cursor-pointer items-center gap-2 font-sans text-sm text-fg">
            <input type="checkbox" checked={allowAll} onChange={(e) => setAllowAll(e.target.checked)} className="accent-[color:var(--theme-accent)]" />
            Allow all agents
          </label>
          {!allowAll &&
            (fleetLoading ? (
              <Skeleton className="h-11 w-full" />
            ) : (
              <Combobox options={agentOptions} selected={agents} onChange={setAgents} multiple placeholder="Select agents" />
            ))}
        </Field>

        <Field label="Invite (optional)">
          <div className="flex items-center gap-2">
            <UserPicker className="min-w-0 flex-1" size="sm" onPick={(u) => u.email && addInvite(u.email)} />
            <Select value={role} onChange={(e) => setRole(e.target.value as 'editor' | 'viewer')} size="sm" className="shrink-0">
              <option value="editor">Editor</option>
              <option value="viewer">Viewer</option>
            </Select>
          </div>
          {invites.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {invites.map((i) => (
                <span key={i.email} className="flex items-center gap-1 rounded-full border border-line px-2 py-0.5 font-mono text-[10px] tracking-[0.05em] text-muted">
                  {i.email} · {i.role}
                  <button onClick={() => setInvites((prev) => prev.filter((x) => x.email !== i.email))} className="transition-colors hover:text-danger">✕</button>
                </span>
              ))}
            </div>
          )}
        </Field>
      </div>
    </Modal>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
      {children}
    </div>
  )
}
