import { useEffect, useState } from 'react'
import { Link } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { GripVertical, Trash2 } from 'lucide-react'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { DangerLink } from '@/components/ui/chip'
import { DropdownMenu } from '@/components/ui/context-menu'
import { confirm } from '@/components/ui/confirm'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Avatar } from '@/components/ui/avatar'
import { Combobox } from '@/components/ui/combobox'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { UserPicker } from '@/components/app/user-picker'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { setBoardTemplates, useBoardTemplates, useTemplates } from '@/lib/templates'
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
  useBoardLabels,
  createBoardLabel,
  updateBoardLabel,
  deleteBoardLabel,
  type Board,
  type LabelColor,
} from '@/lib/boards'
import { LABEL_CSS } from '@/components/board/field-pills'
import {
  useBoardStatuses,
  createBoardStatus,
  updateBoardStatus,
  reorderBoardStatuses,
  deleteBoardStatus,
  type BoardStatus,
} from '@/lib/statuses'

type Tab = 'general' | 'statuses' | 'labels' | 'people' | 'agents'

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
        {(['general', 'statuses', 'labels', 'people', 'agents'] as const).map((t) => (
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
      {tab === 'statuses' && <StatusesTab board={board} />}
      {tab === 'labels' && <LabelsTab board={board} />}
      {tab === 'people' && <PeopleTab board={board} />}
      {tab === 'agents' && <AgentsTab board={board} />}
    </Modal>
  )
}

// The ticket templates this board uses: bind from the org library, mark one as
// the default (it seeds bare tickets and shapes agent drafting on this board).
function TemplatesSection({ board }: { board: Board }) {
  const qc = useQueryClient()
  // Same class as the tabs below: "No ticket templates in the library yet" is
  // a statement about the org library, and `= []` let a failed read make it.
  // The bindings read matters more — its `[]` says "this board binds none",
  // and saving from that state would UNBIND every template it never read.
  const templatesQuery = useTemplates()
  const bindingsQuery = useBoardTemplates(board.id)
  const templates = templatesQuery.data ?? []
  const bindings = bindingsQuery.data ?? []
  const templatesLoading = templatesQuery.isLoading
  const bindingsLoading = bindingsQuery.isLoading
  const templatesFailed =
    (templatesQuery.isError && templatesQuery.data === undefined) ||
    (bindingsQuery.isError && bindingsQuery.data === undefined)
  const ticketTemplates = templates.filter((t) => t.kind === 'ticket')
  const bound = new Set(bindings.map((b) => b.templateId))
  const defaultId = bindings.find((b) => b.isDefault)?.templateId ?? null

  const save = async (ids: string[], def: string | null) => {
    await setBoardTemplates(board.id, ids, def && ids.includes(def) ? def : (ids[0] ?? null))
    await qc.invalidateQueries({ queryKey: ['board-templates', board.id] })
  }
  const toggle = (id: string) => {
    const ids = bound.has(id) ? [...bound].filter((x) => x !== id) : [...bound, id]
    void save(ids, defaultId)
  }

  return (
    <div>
      <div className="mb-1 flex items-center">
        <label className="text-[11px] uppercase tracking-wide text-muted">Ticket templates</label>
        <Link to="/templates" className="ml-auto text-xs text-accent hover:underline">
          Manage library →
        </Link>
      </div>
      {templatesFailed ? (
        // No checkboxes at all while a read is missing: an unchecked box here
        // is a binding you could destroy by pressing Save.
        <QueryError
          variant="compact"
          title="Could not load ticket templates"
          error={templatesQuery.error ?? bindingsQuery.error}
          onRetry={() => {
            void templatesQuery.refetch()
            void bindingsQuery.refetch()
          }}
        />
      ) : templatesLoading || bindingsLoading ? (
        <SkeletonRows rows={3} className="rounded-xl border border-line-subtle p-2" />
      ) : ticketTemplates.length === 0 ? (
        <div className="text-xs text-muted">No ticket templates in the library yet. Create one to templatize this board's tickets.</div>
      ) : (
        <div className="space-y-1 rounded-xl border border-line-subtle p-2">
          {ticketTemplates.map((t) => (
            <div key={t.id} className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={bound.has(t.id)}
                onChange={() => toggle(t.id)}
                className="shrink-0 accent-[var(--theme-accent)]"
              />
              <span className="min-w-0 flex-1 truncate text-fg">{t.name}</span>
              {bound.has(t.id) && (
                <label className="flex shrink-0 items-center gap-1 text-[11px] text-muted">
                  <input
                    type="radio"
                    name={`default-template-${board.id}`}
                    checked={defaultId === t.id}
                    onChange={() => void save([...bound], t.id)}
                    className="accent-[var(--theme-accent)]"
                  />
                  default
                </label>
              )}
            </div>
          ))}
        </div>
      )}
      <div className="mt-1 text-[11px] text-muted">
        The default seeds new tickets and formats agent-drafted ones on this board (an agent's own template binding wins).
      </div>
    </div>
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
          <option value="off">Off: no automated review</option>
          <option value="advisory">Advisory: judge posts a verdict, human decides</option>
          <option value="enforcing">Enforcing: auto-send failing work back to the agent (up to 3×), then a human</option>
        </Select>
        <div className="mt-1 text-[11px] text-muted">
          When an agent hands a ticket to quality review, the judge reviews it. Enforcing bounces “revise” verdicts back to the agent with the issues before a human sees them.
        </div>
      </div>

      <TemplatesSection board={board} />

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
              <DangerLink className="shrink-0" onClick={() => setConfirmDelete(true)}>
                Delete board
              </DangerLink>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

function PeopleTab({ board }: { board: Board }) {
  const qc = useQueryClient()
  // This list IS the access-control answer. `= []` under it meant a 500 on
  // /api/boards/:id/members rendered "People with access" over nothing —
  // telling an owner the board is private to them, from a read that failed.
  const membersQuery = useBoardMembers(board.id)
  const members = membersQuery.data ?? []
  const membersLoading = membersQuery.isLoading
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
        {membersLoading && <SkeletonRows rows={3} avatar className="px-1 py-1.5" />}
        {membersQuery.isError && (
          <QueryError
            variant={membersQuery.data === undefined ? 'compact' : 'inline'}
            className="px-1 py-1.5"
            title={membersQuery.data === undefined ? 'Could not load who has access' : 'This list may be out of date'}
            error={membersQuery.error}
            onRetry={() => void membersQuery.refetch()}
          />
        )}
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
  const fleetQuery = useAgents()
  const fleet = fleetQuery.data
  const fleetLoading = fleetQuery.isLoading
  const options = (fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))
  // The checkbox and the picker are SEEDED from `config`. On rejection the
  // effect below never runs, so the panel renders "Allow all agents" unchecked
  // with an empty selection — a maximally restrictive policy invented from a
  // read that failed, and one Save away from becoming real.
  const configQuery = useBoardAgents(board.id)
  const config = configQuery.data
  const configLoading = configQuery.isLoading
  const configFailed =
    (configQuery.isError && config === undefined) || (fleetQuery.isError && fleet === undefined)
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
        Restrictive by default: a ticket can only be assigned to agents allowed here.
      </p>
      {configFailed ? (
        <QueryError
          variant="compact"
          title="Could not load the agent policy"
          error={configQuery.error ?? fleetQuery.error}
          onRetry={() => {
            void configQuery.refetch()
            void fleetQuery.refetch()
          }}
        />
      ) : configLoading || fleetLoading ? (
        // The checkbox is seeded from the fetched config — showing it unchecked
        // now would render a false policy that flips after load.
        <div>
          <Skeleton className="h-11 w-full" />
          <div className="mt-4 flex items-center justify-between gap-2">
            <Skeleton className="h-4 w-32" />
            <Skeleton className="h-8 w-16" delay={0.12} />
          </div>
        </div>
      ) : (
        <>
          {!allowAll && <Combobox options={options} selected={agents} onChange={setAgents} multiple placeholder="Select agents" />}
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
        </>
      )}
    </div>
  )
}

// ── Labels: the board's label registry — create, rename (cascades into
//    tickets), recolor, delete (strips off tickets). ─────────────────────
function LabelsTab({ board }: { board: Board }) {
  const qc = useQueryClient()
  // A failed registry read used to render an empty label list under prose that
  // explains what labels do — read as "this board has none", which invites
  // creating duplicates of labels that already exist.
  const labelsQuery = useBoardLabels(board.id)
  const labels = labelsQuery.data ?? []
  const canEdit = board.role === 'owner' || board.role === 'editor'
  const [draft, setDraft] = useState('')
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['board-labels', board.id] })
    void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  }

  return (
    <div className="space-y-3">
      <p className="font-sans text-xs text-muted">
        Labels are shared by everyone on this board. Renaming updates every ticket carrying the label; deleting removes it
        from tickets.
      </p>
      {labelsQuery.isError && (
        <QueryError
          variant={labelsQuery.data === undefined ? 'compact' : 'inline'}
          title={labelsQuery.data === undefined ? 'Could not load labels' : 'Labels may be out of date'}
          error={labelsQuery.error}
          onRetry={() => void labelsQuery.refetch()}
        />
      )}
      <ul className="divide-y divide-line-subtle">
        {labels.map((l) => (
          <li key={l.id} className="flex items-center gap-2 py-2">
            <DropdownMenu
              align="left"
              trigger={(open) => (
                <button
                  title="Color"
                  disabled={!canEdit}
                  className={cn('h-4 w-4 shrink-0 rounded-full ring-2 transition-shadow', open ? 'ring-[var(--theme-accent-border)]' : 'ring-transparent')}
                  style={{ background: LABEL_CSS[l.color] }}
                />
              )}
              items={(Object.keys(LABEL_CSS) as LabelColor[]).map((c) => ({
                label: c,
                icon: <span className="h-2.5 w-2.5 rounded-full" style={{ background: LABEL_CSS[c] }} />,
                checked: l.color === c,
                onSelect: () => void updateBoardLabel(board.id, l.id, { color: c }).then(refresh),
              }))}
            />
            <Input
              size="sm"
              defaultValue={l.name}
              key={`${l.id}-${l.name}`}
              disabled={!canEdit}
              onBlur={(e) => {
                const v = e.target.value.trim()
                if (v && v !== l.name) void updateBoardLabel(board.id, l.id, { name: v }).then(refresh)
              }}
              onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
              className="flex-1"
            />
            {canEdit && (
              <button
                title="Delete label (removes it from tickets)"
                onClick={() =>
                  void (async () => {
                    if (await confirm({ title: `Delete label "${l.name}"?`, message: 'It is removed from every ticket carrying it.', danger: true })) {
                      await deleteBoardLabel(board.id, l.id)
                      refresh()
                    }
                  })()
                }
                className="shrink-0 text-muted transition-colors hover:text-[color:var(--theme-danger)]"
              >
                <Trash2 size={14} />
              </button>
            )}
          </li>
        ))}
        {/* Only once the server actually SAID so — otherwise the error above
            and "No labels yet." sit on screen contradicting each other. */}
        {labels.length === 0 && labelsQuery.data !== undefined && (
          <li className="py-3 text-xs text-muted">No labels yet.</li>
        )}
      </ul>
      {/* Nothing to add against: the registry never loaded, so "Add" here is a
          coin flip on whether the label already exists. */}
      {canEdit && labelsQuery.data !== undefined && (
        <div className="flex gap-2">
          <Input
            size="sm"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="New label"
            onKeyDown={(e) => {
              if (e.key !== 'Enter' || !draft.trim()) return
              void createBoardLabel(board.id, draft.trim()).then(() => {
                setDraft('')
                refresh()
              })
            }}
            className="flex-1"
          />
          <Button
            size="sm"
            disabled={!draft.trim()}
            onClick={() =>
              void createBoardLabel(board.id, draft.trim()).then(() => {
                setDraft('')
                refresh()
              })
            }
          >
            Add
          </Button>
        </div>
      )}
    </div>
  )
}

// ── Statuses: the board's workflow columns. Order = column order; category
//    carries the semantics; agentStart = "agents may pick up work here".
//    Blocked is system — pinned, not editable. ─────────────────────────────
function StatusesTab({ board }: { board: Board }) {
  const qc = useQueryClient()
  // Worse here than anywhere: an empty list under "Columns and their meaning"
  // reads as a board with no workflow at all, and the Add box beside it will
  // happily create a duplicate of a status the server already has.
  const statusesQuery = useBoardStatuses(board.id)
  const statuses = statusesQuery.data ?? []
  const canEdit = board.role === 'owner' || board.role === 'editor'
  const [draft, setDraft] = useState('')
  const [err, setErr] = useState<string | null>(null)
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['board-statuses', board.id] })
    void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  }
  const editable = statuses.filter((st) => !st.system)
  const run = (fn: () => Promise<unknown>) => {
    setErr(null)
    void fn()
      .then(refresh)
      .catch((e: Error) => setErr(e.message))
  }
  // Drag-to-reorder — the same grammar as column reordering (grip handle,
  // before/after indicator, drop commits). Blocked is system: not draggable,
  // not a target (the server places it automatically).
  const [dragKey, setDragKey] = useState<string | null>(null)
  const [overKey, setOverKey] = useState<string | null>(null)
  const [overPos, setOverPos] = useState<'before' | 'after'>('before')
  const dropStatus = (target: BoardStatus) => {
    if (dragKey && dragKey !== target.key && !target.system) {
      const next = editable.map((x) => x.key).filter((k) => k !== dragKey)
      const idx = next.indexOf(target.key) + (overPos === 'after' ? 1 : 0)
      next.splice(idx, 0, dragKey)
      run(() => reorderBoardStatuses(board.id, next))
    }
    setDragKey(null)
    setOverKey(null)
  }

  return (
    <div className="space-y-3">
      <p className="font-sans text-xs text-muted">
        Columns and their meaning. <strong>Category</strong> drives the workflow: intake statuses receive new tickets,
        review is where agent work lands for sign-off, done completes. <strong>Agent start</strong> marks the columns
        where assignment counts as approval — agents only pick up work sitting there. Blocked is always present.
      </p>
      {err && <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>{err}</div>}
      {statusesQuery.isError && (
        <QueryError
          variant={statusesQuery.data === undefined ? 'compact' : 'inline'}
          title={statusesQuery.data === undefined ? 'Could not load this board’s columns' : 'Columns may be out of date'}
          error={statusesQuery.error}
          onRetry={() => void statusesQuery.refetch()}
        />
      )}
      <ul className="divide-y divide-line-subtle">
        {statuses.map((st) => (
          <li
            key={st.key}
            draggable={canEdit && !st.system}
            onDragStart={(e) => {
              e.dataTransfer.effectAllowed = 'move'
              setDragKey(st.key)
            }}
            onDragEnd={() => {
              setDragKey(null)
              setOverKey(null)
            }}
            onDragOver={(e) => {
              if (!dragKey || st.system) return
              e.preventDefault()
              const rect = e.currentTarget.getBoundingClientRect()
              setOverKey(st.key)
              setOverPos(e.clientY > rect.top + rect.height / 2 ? 'after' : 'before')
            }}
            onDrop={(e) => {
              e.preventDefault()
              dropStatus(st)
            }}
            className={cn('relative flex items-center gap-2 py-2', st.system && 'opacity-70', dragKey === st.key && 'opacity-40')}
          >
            {overKey === st.key && dragKey !== st.key && (
              <span
                className={cn(
                  'pointer-events-none absolute inset-x-0 h-0.5 rounded-full bg-accent',
                  overPos === 'before' ? '-top-px' : '-bottom-px',
                )}
              />
            )}
            <GripVertical
              size={13}
              className={cn('shrink-0', canEdit && !st.system ? 'cursor-grab text-muted' : 'text-muted/30')}
            />
            <DropdownMenu
              align="left"
              trigger={(open) => (
                <button
                  title="Color"
                  disabled={!canEdit || st.system}
                  className={cn('h-4 w-4 shrink-0 rounded-full ring-2 transition-shadow', open ? 'ring-[var(--theme-accent-border)]' : 'ring-transparent')}
                  style={{ background: LABEL_CSS[st.color as keyof typeof LABEL_CSS] ?? 'var(--theme-muted)' }}
                />
              )}
              items={(Object.keys(LABEL_CSS) as Array<keyof typeof LABEL_CSS>).map((c) => ({
                label: c,
                icon: <span className="h-2.5 w-2.5 rounded-full" style={{ background: LABEL_CSS[c] }} />,
                checked: st.color === c,
                onSelect: () => run(() => updateBoardStatus(board.id, st.key, { color: c })),
              }))}
            />
            {st.system ? (
              <span className="flex-1 text-sm text-fg">Blocked <span className="ml-1 text-[10px] uppercase tracking-wide text-muted">system</span></span>
            ) : (
              <Input
                size="sm"
                defaultValue={st.label}
                key={`${st.key}-${st.label}`}
                disabled={!canEdit}
                onBlur={(e) => {
                  const v = e.target.value.trim()
                  if (v && v !== st.label) run(() => updateBoardStatus(board.id, st.key, { label: v }))
                }}
                onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
                className="flex-1"
              />
            )}
            {!st.system && (
              <>
                <Select
                  size="sm"
                  value={st.category}
                  disabled={!canEdit}
                  title="Workflow category"
                  onChange={(e) => run(() => updateBoardStatus(board.id, st.key, { category: e.target.value }))}
                  className="w-28 shrink-0"
                >
                  <option value="open">intake</option>
                  <option value="active">active</option>
                  <option value="review">review</option>
                  <option value="done">done</option>
                </Select>
                <label
                  title="Agents may pick up work in this column (assignment here = approval to start)"
                  className="flex shrink-0 cursor-pointer items-center gap-1 text-[10px] uppercase tracking-wide text-muted"
                >
                  <input
                    type="checkbox"
                    checked={st.agentStart}
                    disabled={!canEdit}
                    onChange={(e) => run(() => updateBoardStatus(board.id, st.key, { agentStart: e.target.checked }))}
                    className="accent-[var(--theme-accent)]"
                  />
                  agent start
                </label>
                {canEdit && (
                  <DropdownMenu
                    align="right"
                    trigger={() => (
                      <button title="Delete status (tickets move to another column)" className="shrink-0 text-muted transition-colors hover:text-[color:var(--theme-danger)]">
                        <Trash2 size={14} />
                      </button>
                    )}
                    items={statuses
                      .filter((o) => o.key !== st.key && !o.system)
                      .map((o) => ({
                        label: `Move tickets to ${o.label}`,
                        onSelect: () => run(() => deleteBoardStatus(board.id, st.key, o.key)),
                      }))}
                  />
                )}
              </>
            )}
          </li>
        ))}
      </ul>
      {/* Same rule as Labels: adding a column while the column set is unknown
          risks duplicating one the server already has. */}
      {canEdit && statusesQuery.data !== undefined && (
        <div className="flex gap-2">
          <Input
            size="sm"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="New status"
            onKeyDown={(e) => {
              if (e.key !== 'Enter' || !draft.trim()) return
              run(() => createBoardStatus(board.id, { label: draft.trim() }))
              setDraft('')
            }}
            className="flex-1"
          />
          <Button
            size="sm"
            disabled={!draft.trim()}
            onClick={() => {
              run(() => createBoardStatus(board.id, { label: draft.trim() }))
              setDraft('')
            }}
          >
            Add
          </Button>
        </div>
      )}
    </div>
  )
}
