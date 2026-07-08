import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Avatar } from '@/components/ui/avatar'
import { Combobox } from '@/components/ui/combobox'
import { UserPicker } from '@/components/app/user-picker'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import {
  archiveBoard,
  deleteBoard,
  renameBoard,
  setBoardAgents,
  setBoardJudgeMode,
  shareBoard,
  unshareBoard,
  useBoardAgents,
  useBoardMembers,
  type Board,
} from '@/lib/boards'

type Tab = 'general' | 'people' | 'agents'

// One place for everything about a board: rename, sharing, agent policy, and the
// danger zone (archive / delete). Keeps the board header uncluttered.
export function BoardSettingsModal({
  board,
  open,
  onClose,
  onArchived,
  onDeleted,
}: {
  board: Board
  open: boolean
  onClose: () => void
  onArchived: () => void
  onDeleted: () => void
}) {
  const [tab, setTab] = useState<Tab>('general')
  const isOwner = board.role === 'owner'

  return (
    <Modal open={open} onClose={onClose} title="Board settings" width="max-w-xl">
      <div className="mb-4 flex gap-1 rounded-lg border border-line p-0.5">
        {(['general', 'people', 'agents'] as const).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={cn(
              'flex-1 rounded-md px-3 py-1.5 text-sm capitalize transition-colors',
              tab === t ? 'bg-card text-fg' : 'text-muted hover:text-fg',
            )}
          >
            {t}
          </button>
        ))}
      </div>

      {tab === 'general' && (
        <GeneralTab board={board} isOwner={isOwner} onClose={onClose} onArchived={onArchived} onDeleted={onDeleted} />
      )}
      {tab === 'people' && <PeopleTab board={board} />}
      {tab === 'agents' && <AgentsTab board={board} />}
    </Modal>
  )
}

function GeneralTab({
  board,
  isOwner,
  onClose,
  onArchived,
  onDeleted,
}: {
  board: Board
  isOwner: boolean
  onClose: () => void
  onArchived: () => void
  onDeleted: () => void
}) {
  const qc = useQueryClient()
  const [name, setName] = useState(board.name)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const archived = !!board.archivedAt
  const refreshBoards = () => qc.invalidateQueries({ queryKey: ['boards'] })

  const commitName = async () => {
    const n = name.trim()
    if (!n || n === board.name) return setName(board.name)
    await renameBoard(board.id, n)
    refreshBoards()
  }

  return (
    <div className="space-y-5">
      <div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Name</label>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={commitName}
          onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
          className="w-full"
        />
      </div>

      <div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">QA judge</label>
        <Select
          value={board.judgeMode ?? 'inherit'}
          onChange={async (e) => {
            await setBoardJudgeMode(board.id, e.target.value as 'inherit' | 'off' | 'advisory' | 'enforcing')
            refreshBoards()
          }}
          className="w-full"
        >
          <option value="inherit">Default (follow the org setting)</option>
          <option value="off">Off — no automated review</option>
          <option value="advisory">Advisory — judge posts a verdict, human decides</option>
          <option value="enforcing">Enforcing — auto-send failing work back to the agent (up to 3×), then a human</option>
        </Select>
        <div className="mt-1 text-[11px] text-muted">
          When an agent hands a ticket to quality review, the judge reviews it. Enforcing bounces “revise” verdicts back to the agent with the issues before a human sees them.
        </div>
      </div>

      <div className="rounded-xl border border-line-subtle p-3">
        <div className="mb-2 text-[11px] uppercase tracking-wide text-muted">Danger zone</div>
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="text-sm text-fg">{archived ? 'Restore board' : 'Archive board'}</div>
            <div className="text-xs text-muted">
              {archived ? 'Make it active and visible again.' : 'Hide it from the sidebar and boards list. Reversible.'}
            </div>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={async () => {
              await archiveBoard(board.id, !archived)
              refreshBoards()
              void qc.invalidateQueries({ queryKey: ['boards', 'archived'] })
              onClose()
              if (!archived) onArchived()
            }}
          >
            {archived ? 'Restore' : 'Archive'}
          </Button>
        </div>

        {isOwner && (
          <div className="mt-3 flex items-center justify-between gap-3 border-t border-line-subtle pt-3">
            <div className="min-w-0">
              <div className="text-sm text-fg">Delete board</div>
              <div className="text-xs text-muted">Permanently removes the board and all its tickets.</div>
            </div>
            {confirmDelete ? (
              <div className="flex shrink-0 gap-2">
                <Button variant="ghost" size="sm" onClick={() => setConfirmDelete(false)}>
                  Cancel
                </Button>
                <Button
                  variant="danger"
                  size="sm"
                  onClick={async () => {
                    await deleteBoard(board.id)
                    refreshBoards()
                    onClose()
                    onDeleted()
                  }}
                >
                  Confirm delete
                </Button>
              </div>
            ) : (
              <Button variant="danger" size="sm" className="shrink-0" onClick={() => setConfirmDelete(true)}>
                Delete
              </Button>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

function PeopleTab({ board }: { board: Board }) {
  const qc = useQueryClient()
  const { data: members = [] } = useBoardMembers(board.id)
  const [role, setRole] = useState<'editor' | 'viewer'>('editor')
  const [err, setErr] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['board-members', board.id] })

  const invite = async (email: string) => {
    setErr(null)
    const res = await shareBoard(board.id, email, role)
    const data = (await res.json().catch(() => ({}))) as { ok?: boolean; error?: string }
    if (!res.ok || !data.ok) return setErr(data.error ?? 'Could not share')
    refresh()
  }

  return (
    <div>
      <div className="flex items-center gap-2">
        <UserPicker
          className="min-w-0 flex-1"
          size="sm"
          exclude={members.map((m) => m.userId)}
          onPick={(u) => {
            if (u.email) void invite(u.email)
          }}
        />
        <Select value={role} onChange={(e) => setRole(e.target.value as 'editor' | 'viewer')} size="sm" className="shrink-0">
          <option value="editor">Editor</option>
          <option value="viewer">Viewer</option>
        </Select>
      </div>
      {err && <div className="mt-2 text-xs" style={{ color: 'var(--theme-danger)' }}>{err}</div>}

      <div className="mt-4 space-y-1">
        <div className="mb-1 text-xs uppercase tracking-wide text-muted">People with access</div>
        {members.map((m) => (
          <div key={m.userId} className="flex items-center gap-2 rounded-lg px-1 py-1.5">
            <Avatar name={m.name ?? m.email} className="h-7 w-7" />
            <span className="min-w-0 flex-1 truncate text-sm text-fg">
              {m.name ?? m.email ?? m.userId}
              {m.name && m.email && <span className="ml-1.5 text-xs text-muted">{m.email}</span>}
            </span>
            <span className="text-xs text-muted">{m.role}</span>
            {m.role !== 'owner' && (
              <button
                onClick={() => void unshareBoard(board.id, m.userId).then(refresh)}
                className="text-xs text-muted hover:text-[color:var(--theme-danger)]"
              >
                Remove
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}

function AgentsTab({ board }: { board: Board }) {
  const qc = useQueryClient()
  const { data: fleet } = useAgents()
  const options = (fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))
  const { data: config } = useBoardAgents(board.id)
  const [allowAll, setAllowAll] = useState(false)
  const [agents, setAgents] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    if (config) {
      setAllowAll(config.allowAll)
      setAgents(config.models)
    }
  }, [config])

  const save = async () => {
    setBusy(true)
    try {
      await setBoardAgents(board.id, allowAll, allowAll ? [] : agents)
      await qc.invalidateQueries({ queryKey: ['board-agents', board.id] })
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div>
      <p className="mb-3 text-xs text-muted">
        Restrictive by default — a ticket can only be assigned to agents allowed here.
      </p>
      {!allowAll && <Combobox options={options} selected={agents} onChange={setAgents} multiple placeholder="Select agents…" />}
      <div className="mt-4 flex items-center justify-between gap-2">
        <label className="flex cursor-pointer items-center gap-2 text-sm text-fg">
          <input
            type="checkbox"
            checked={allowAll}
            onChange={(e) => setAllowAll(e.target.checked)}
            className="accent-[color:var(--theme-accent)]"
          />
          Allow all agents
        </label>
        <div className="flex items-center gap-2">
          {saved && <span className="text-xs text-[color:var(--theme-success)]">Saved</span>}
          <Button size="sm" onClick={() => void save()} disabled={busy}>
            Save
          </Button>
        </div>
      </div>
    </div>
  )
}
