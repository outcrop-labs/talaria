import { useEffect, useRef, useState } from 'react'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { useQueryClient } from '@tanstack/react-query'
import { motion, AnimatePresence } from 'framer-motion'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { inlineEditKeys } from '@/components/ui/control'
import { Maximize2, ChevronLeft, Archive, ArchiveRestore, Trash2 } from 'lucide-react'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { CloseButton } from '@/components/ui/close-button'
import { CopyLinkButton } from '@/components/ui/copy-link-button'
import { Select } from '@/components/ui/select'
import { Combobox } from '@/components/ui/combobox'
import { LabelPicker } from '@/components/board/label-picker'
import { Markdown } from '@/components/ui/markdown'
import { useAgents } from '@/lib/agents'
import { formatCost, formatTokens } from '@/lib/cost'
import { useSession } from '@/lib/session'
import {
  addComment,
  addDependency,
  archiveTask,
  createTask,
  deleteTask,
  removeDependency,
  reviewTask,
  unwatchTask,
  updateTask,
  useBoardAgents,
  useBoardLabels,
  useBoardMembers,
  useBoardTasks,
  useTask,
  watchTask,
  type Board,
  type JudgeReview,
} from '@/lib/boards'
import { useNavigate } from '@tanstack/react-router'
import { userAssignee } from '@/lib/assignees'
import { userMentionInsert, type Mentionable } from '@/components/chat/mentions'
import { LABEL_CSS } from '@/components/board/field-pills'
import {
  EFFORTS,
  EFFORT_LABEL,
  TICKET_COLORS,
  PRIORITIES,
  PRIORITY_ICON,
  STATUS_LABEL,
  TASK_STATUSES,
  type Effort,
  type Priority,
  type TaskStatus,
} from '@/lib/task-const'
import { relativeTime } from '@/lib/fleet'
import { cn } from '@/lib/cn'
import { Link } from '@tanstack/react-router'
import { BookOpen, FileText, Gem, X } from 'lucide-react'
import { AttachButton } from '@/components/chat/attachments'
import { attachmentUrl, humanSize, isImage, splitAttachments, type Attachment } from '@/lib/attachments'
import type { Task } from '@/lib/task-const'

const MOVE: TaskStatus[] = [...TASK_STATUSES, 'failed', 'cancelled']

// Linear/Plane-style ticket: content (left) + properties rail (right).
export function TaskDetail({ taskId, board, onClose }: { taskId: string; board: Board; onClose: () => void }) {
  const qc = useQueryClient()
  const { data } = useTask(taskId)
  const { data: fleet } = useAgents()
  const { data: user } = useSession()
  const { data: boardCfg } = useBoardAgents(board.id)
  const allAgents = fleet?.agents ?? []
  // Restrict the assignee list to the board's agent policy (allow-all or list).
  const agents = boardCfg?.allowAll ? allAgents : allAgents.filter((a) => boardCfg?.models.includes(a.id))
  const canEdit = board.role === 'owner' || board.role === 'editor'
  const me = user?.email ?? user?.name ?? ''
  // Board tickets for the dependency picker (exclude self + already-linked).
  const { data: boardTasks = [] } = useBoardTasks(board.id)
  // @mention board members in comments + description — the people the server
  // notifies (tasks comment/description paths). Tokens mirror the server's.
  const { data: boardMembers = [] } = useBoardMembers(board.id)
  const { data: boardLabels = [] } = useBoardLabels(board.id)
  const mentionables = boardMembers
    .map((m) => ({ insert: userMentionInsert(m), label: m.name ?? m.email ?? m.userId, sub: m.email ?? undefined }))
    .filter((m) => m.insert)
  // Assignees mix humans (board members, `user:<id>`) and the board's agents.
  const assigneeOptions = [
    ...boardMembers.map((m) => ({
      value: userAssignee(m.userId),
      label: (m.name ?? m.email ?? 'teammate') + (m.userId === user?.id ? ' (me)' : ''),
      sub: 'teammate',
    })),
    ...agents.map((a) => ({ value: a.id, label: a.label, sub: a.role })),
  ]

  const [title, setTitle] = useState('')
  const [tab, setTab] = useState<'comments' | 'activity'>('comments')
  const commentRef = useRef<RichEditorHandle>(null)
  const navigate = useNavigate()
  // Jump to a sibling ticket (parent/sub-task links) — same overlay route.
  const openTask = (id: string) =>
    void navigate({
      to: '/boards/$boardId/$taskId',
      params: { boardId: board.id, taskId: id },
      search: (prev: Record<string, unknown>) => prev,
    })

  // Initialise editable fields ONCE per task (not on every refetch) so live
  // updates behind the modal don't reset what the user is typing.
  const loadedRef = useRef<string | null>(null)
  useEffect(() => {
    if (data?.task && loadedRef.current !== data.task.id) {
      loadedRef.current = data.task.id
      setTitle(data.task.title)
    }
  }, [data?.task])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [onClose])

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['task', taskId] })
    void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  }
  const save = async (patch: Parameters<typeof updateTask>[1]) => {
    await updateTask(taskId, patch)
    refresh()
  }
  const t = data?.task

  return (
    <AnimatePresence>
      <div className="fixed inset-0 z-40 grid place-items-center p-4">
        <motion.div className="absolute inset-0 bg-black/50" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onClick={onClose} />
        <motion.div
          initial={{ opacity: 0, y: 12, scale: 0.99 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 12 }}
          transition={{ duration: 0.16 }}
          className="mercury-panel relative z-10 flex h-[85vh] w-full max-w-4xl overflow-hidden rounded-2xl"
        >
          {!t ? (
            <div className="flex h-full w-full gap-6 p-6">
              <div className="min-w-0 flex-1 space-y-4">
                <Skeleton className="h-5 w-2/3 rounded-full" />
                <SkeletonRows rows={5} />
              </div>
              <div className="w-56 shrink-0 space-y-3">
                <SkeletonRows rows={6} />
              </div>
            </div>
          ) : (
            <>
              {/* Content */}
              <div className="flex min-w-0 flex-1 flex-col">
                <div className="flex items-center gap-2 border-b border-line-subtle px-5 py-2.5">
                  {t.ticketRef && <span className="font-[var(--font-mono)] text-xs text-muted">{t.ticketRef}</span>}
                  <span className="text-xs text-muted">·</span>
                  <span className="text-xs text-muted">opened by {t.createdBy}</span>
                  {t.archivedAt && (
                    <span className="rounded-md border border-line-subtle bg-card px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted">
                      Archived
                    </span>
                  )}
                  <CopyLinkButton
                    path={`/boards/${board.id}/${taskId}`}
                    label="Copy link"
                    title="Copy link to this ticket"
                    className="ml-auto px-1.5 py-0.5 text-xs"
                  />
                </div>

                {/* Details — scrolls independently, capped so discussion gets room */}
                <div className="max-h-[46%] shrink-0 space-y-5 overflow-y-auto px-5 py-4">
                  <Input
                    value={title}
                    disabled={!canEdit}
                    onChange={(e) => setTitle(e.target.value)}
                    onBlur={() => title.trim() && title !== t.title && save({ title: title.trim() })}
                    onKeyDown={inlineEditKeys(() => setTitle(t.title))}
                    className="border-0 bg-transparent px-0 text-lg font-semibold focus:border-0"
                  />

                  {/* QA judge verdict (advisory) — most recent first */}
                  {t.status === 'quality_review' && data?.judgeReviews?.[0] && <JudgeVerdict review={data.judgeReviews[0]} />}

                  {/* Approval gate */}
                  {t.status === 'quality_review' && canEdit && (
                    <div className="flex items-center gap-2 rounded-lg border border-[color:var(--theme-accent-border)] bg-accent-soft p-2 text-sm">
                      <span className="flex-1 text-fg">Ready for review. Approve to complete.</span>
                      <Button size="sm" onClick={async () => { await reviewTask(taskId, 'approved'); refresh() }}>Approve</Button>
                      <Button variant="outline" size="sm" onClick={async () => { await reviewTask(taskId, 'rejected'); refresh() }}>Request changes</Button>
                    </div>
                  )}

                  <DescriptionSection
                    key={`ds-${t.id}`}
                    title={t.ticketRef ? `${t.ticketRef} · ${t.title}` : t.title}
                    value={t.description ?? ''}
                    canEdit={canEdit}
                    mentions={mentionables}
                    onSave={(md) => {
                      // RichEditor only fires this on a real change. Refresh just
                      // the board's cards — never refetch the open ticket.
                      void updateTask(taskId, { description: md || null }).then(() =>
                        qc.invalidateQueries({ queryKey: ['board-tasks', board.id] }),
                      )
                    }}
                  />

                  <AttachmentsSection task={t} canEdit={canEdit} onSaved={() => qc.invalidateQueries({ queryKey: ['task', taskId] })} />

                  {/* Agent-reported result */}
                  {(t.outcome || t.resolution || t.errorMessage) && (
                    <Section label="Result">
                      {t.outcome && <ResultBlock title="Outcome">{t.outcome}</ResultBlock>}
                      {t.resolution && <ResultBlock title="Resolution">{t.resolution}</ResultBlock>}
                      {t.errorMessage && <ResultBlock title="Error" danger>{t.errorMessage}</ResultBlock>}
                    </Section>
                  )}
                </div>

                {/* Discussion — Comments / Activity tabs; comment composer pinned */}
                <div className="flex min-h-0 flex-1 flex-col border-t border-line-subtle">
                  <div className="flex items-center gap-1 px-5 pt-3">
                    {(['comments', 'activity'] as const).map((tb) => (
                      <button
                        key={tb}
                        onClick={() => setTab(tb)}
                        className={cn(
                          'rounded-md px-3 py-1 text-xs font-medium transition-colors',
                          tab === tb ? 'bg-card text-fg' : 'text-muted hover:text-fg',
                        )}
                      >
                        {tb === 'comments' ? `Comments (${data!.comments.length})` : 'Activity'}
                      </button>
                    ))}
                  </div>

                  {tab === 'comments' ? (
                    <>
                      <ul className="min-h-0 flex-1 space-y-2 overflow-y-auto px-5 py-3">
                        {data!.comments.map((c) => (
                          <li key={c.id} className="rounded-lg border border-line-subtle bg-card p-2">
                            <div className="mb-0.5 flex items-center justify-between text-xs">
                              <span className="text-accent">{c.author}</span>
                              <span className="text-muted">{relativeTime(c.createdAt)}</span>
                            </div>
                            <div className="text-sm text-fg"><Markdown>{c.content}</Markdown></div>
                          </li>
                        ))}
                        {data!.comments.length === 0 && <li className="text-xs text-muted">No comments yet.</li>}
                      </ul>
                      <RichEditor
                        ref={commentRef}
                        key={`comment-${t.id}`}
                        value=""
                        editable
                        bare
                        mentions={mentionables}
                        className="shrink-0 border-t border-line-subtle"
                        placeholder="Write a comment  (Ctrl+Enter to send)"
                        minHeight="5rem"
                        onSubmit={() => {
                          const md = (commentRef.current?.getMarkdown() ?? '').trim()
                          if (!md) return
                          commentRef.current?.clear()
                          void addComment(taskId, md).then(refresh)
                        }}
                      />
                    </>
                  ) : (
                    <ul className="min-h-0 flex-1 space-y-1 overflow-y-auto px-5 py-3">
                      {data!.activity.map((a) => (
                        <li key={a.id} className="flex items-center gap-2 text-xs text-muted">
                          <span className="text-accent">{a.actor}</span>
                          <span className="min-w-0 flex-1 truncate">{a.description}</span>
                          <span className="shrink-0">{relativeTime(a.createdAt)}</span>
                        </li>
                      ))}
                      {data!.activity.length === 0 && <li className="text-xs text-muted">No activity yet.</li>}
                    </ul>
                  )}
                </div>
              </div>

              {/* Properties rail */}
              <aside className="flex w-60 shrink-0 flex-col border-l border-line-subtle bg-sidebar">
                <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
                <CloseButton onClick={onClose} className="-mr-1 ml-auto" />
                <Prop label="Status">
                  <Select value={t.status} disabled={!canEdit} onChange={(e) => save({ status: e.target.value as TaskStatus })} size="sm" className="w-full">
                    {MOVE.map((s) => <option key={s} value={s}>{STATUS_LABEL[s]}</option>)}
                  </Select>
                </Prop>
                <Prop label="Priority">
                  <Select value={t.priority} disabled={!canEdit} onChange={(e) => save({ priority: e.target.value as Priority })} size="sm" className="w-full">
                    {PRIORITIES.map((p) => <option key={p} value={p}>{PRIORITY_ICON[p]} {p}</option>)}
                  </Select>
                </Prop>
                <Prop label="Color">
                  <div className="flex flex-wrap items-center gap-2 py-1">
                    {TICKET_COLORS.map((c) => {
                      const active = t.color === c
                      return (
                        <button
                          key={c}
                          title={active ? `${c} — click to clear` : c}
                          disabled={!canEdit}
                          onClick={() => save({ color: active ? null : c })}
                          className={cn(
                            'grid h-5 w-5 place-items-center rounded-full transition-all',
                            // Clear but calm: a quiet offset ring marks the pick.
                            active
                              ? 'ring-1 ring-[color:var(--theme-fg)]/60 ring-offset-2 ring-offset-[color:var(--theme-panel)]'
                              : 'ring-1 ring-transparent hover:ring-line',
                          )}
                          style={{ background: LABEL_CSS[c] }}
                        />
                      )
                    })}
                  </div>
                </Prop>
                <Prop label="Assignees">
                  <Combobox
                    options={assigneeOptions}
                    selected={t.assignees}
                    onChange={(arr) => canEdit && save({ assignees: arr })}
                    disabled={!canEdit}
                    multiple
                    size="sm"
                    placeholder="Unassigned"
                  />
                </Prop>
                <div className="grid grid-cols-2 gap-2">
                  <Prop label="Effort">
                    <Select value={t.effort ?? ''} disabled={!canEdit}
                      onChange={(e) => save({ effort: (e.target.value || null) as Effort | null })} size="sm" className="w-full">
                      <option value="">—</option>
                      {EFFORTS.map((ef) => <option key={ef} value={ef}>{EFFORT_LABEL[ef]}</option>)}
                    </Select>
                  </Prop>
                  <Prop label="Estimate (h)">
                    <Input
                      type="number"
                      min={0}
                      max={999}
                      step={0.5}
                      size="sm"
                      disabled={!canEdit}
                      defaultValue={t.estimatedHours ?? ''}
                      key={`est-${t.id}-${t.estimatedHours ?? ''}`}
                      onBlur={(e) => {
                        const v = e.target.value.trim()
                        const n = v === '' ? null : Number(v)
                        if (n !== t.estimatedHours && (n === null || (!Number.isNaN(n) && n >= 0))) save({ estimatedHours: n })
                      }}
                      placeholder="—"
                      className="w-full"
                    />
                  </Prop>
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <Prop label="Time spent">
                    <div className="flex h-9 items-center text-sm text-fg">{formatDuration(t.timeSpentSeconds)}</div>
                  </Prop>
                </div>
                {/* Agent-reported token spend (MCP log_usage) — priced like the ledger. */}
                {data!.usage.promptTokens + data!.usage.completionTokens > 0 && (
                  <Prop label="Tokens">
                    <div className="space-y-1 text-sm text-fg">
                      <div>
                        {formatTokens(data!.usage.promptTokens + data!.usage.completionTokens)}
                        {data!.usage.cost > 0 && <span className="text-muted"> · {formatCost(data!.usage.cost)}</span>}
                        {data!.usage.unpricedTokens > 0 && <span className="text-muted"> · partly unpriced</span>}
                      </div>
                      {data!.usage.perModel.map((m) => (
                        <div key={m.llmModel ?? '?'} className="truncate text-xs text-muted">
                          {m.llmModel ?? 'unattributed'} · {formatTokens(m.tokens)}
                          {m.cost !== null && m.cost > 0 && ` · ${formatCost(m.cost)}`}
                        </div>
                      ))}
                    </div>
                  </Prop>
                )}
                <div className="grid grid-cols-2 gap-2">
                  <Prop label="Start date">
                    <Input type="date" value={t.startDate ? t.startDate.slice(0, 10) : ''} disabled={!canEdit}
                      onChange={(e) => save({ startDate: e.target.value ? new Date(e.target.value).toISOString() : null })}
                      size="sm" className="w-full" />
                  </Prop>
                  <Prop label="Due date">
                    <Input type="date" value={t.dueDate ? t.dueDate.slice(0, 10) : ''} disabled={!canEdit}
                      onChange={(e) => save({ dueDate: e.target.value ? new Date(e.target.value).toISOString() : null })}
                      size="sm" className="w-full" />
                  </Prop>
                </div>
                {/* Sub-tasks: one level deep. Children list + inline add on a
                    parent; a child shows its parent with a promote control. */}
                {t.parentId ? (
                  <Prop label="Parent">
                    <div className="flex items-center gap-1 text-xs">
                      <button
                        onClick={() => {
                          const p = boardTasks.find((bt) => bt.id === t.parentId)
                          if (p) openTask(p.id)
                        }}
                        className="min-w-0 flex-1 truncate text-left text-muted transition-colors hover:text-fg"
                      >
                        {(() => {
                          const p = boardTasks.find((bt) => bt.id === t.parentId)
                          return p ? (
                            <>
                              {p.ticketRef && <span className="font-[var(--font-mono)]">{p.ticketRef} </span>}
                              {p.title}
                            </>
                          ) : (
                            'parent ticket'
                          )
                        })()}
                      </button>
                      {canEdit && (
                        <button
                          onClick={() => save({ parentId: null })}
                          title="Promote to top level"
                          className="shrink-0 text-muted hover:text-fg"
                        >
                          ✕
                        </button>
                      )}
                    </div>
                  </Prop>
                ) : (
                  <Prop label={`Sub-tasks (${boardTasks.filter((bt) => bt.parentId === t.id).length})`}>
                    <div className="space-y-1">
                      {boardTasks
                        .filter((bt) => bt.parentId === t.id)
                        .map((st) => (
                          <div key={st.id} className="flex items-center gap-1.5 text-xs">
                            <span
                              className="h-1.5 w-1.5 shrink-0 rounded-full"
                              style={{ background: st.status === 'done' ? 'var(--theme-success)' : 'var(--theme-line)' }}
                            />
                            <button
                              onClick={() => openTask(st.id)}
                              className={cn('min-w-0 flex-1 truncate text-left transition-colors hover:text-fg', st.status === 'done' ? 'text-muted line-through' : 'text-muted')}
                            >
                              {st.ticketRef && <span className="font-[var(--font-mono)]">{st.ticketRef} </span>}
                              {st.title}
                            </button>
                          </div>
                        ))}
                      {canEdit && (
                        <SubtaskAdd
                          onAdd={async (title) => {
                            await createTask(board.id, { title, parentId: t.id })
                            qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
                            refresh()
                          }}
                        />
                      )}
                    </div>
                  </Prop>
                )}
                <Prop label={`Blocked by (${data!.blockedBy.length})`}>
                  <div className="space-y-1">
                    {data!.blockedBy.map((d) => (
                      <div key={d.id} className="flex items-center gap-1 text-xs">
                        <span className="min-w-0 flex-1 truncate text-muted">
                          {d.ticketRef && <span className="font-[var(--font-mono)]">{d.ticketRef} </span>}{d.title}
                        </span>
                        {canEdit && (
                          <button onClick={async () => { await removeDependency(taskId, d.id); refresh() }}
                            className="shrink-0 text-muted hover:text-[color:var(--theme-danger)]">✕</button>
                        )}
                      </div>
                    ))}
                    {data!.blockedBy.length === 0 && <div className="text-xs text-muted">None</div>}
                    {canEdit && (
                      <Combobox
                        options={boardTasks
                          .filter((bt) => bt.id !== taskId && !data!.blockedBy.some((b) => b.id === bt.id))
                          .map((bt) => ({ value: bt.id, label: `${bt.ticketRef ? bt.ticketRef + ' ' : ''}${bt.title}`, sub: STATUS_LABEL[bt.status] }))}
                        selected={[]}
                        onChange={async (arr) => { if (arr[0]) { await addDependency(taskId, arr[0]); refresh() } }}
                        size="sm"
                        placeholder="Add dependency"
                      />
                    )}
                  </div>
                </Prop>
                {data!.blocks.length > 0 && (
                  <Prop label={`Blocks (${data!.blocks.length})`}>
                    <div className="space-y-1">
                      {data!.blocks.map((d) => (
                        <div key={d.id} className="truncate text-xs text-muted">
                          {d.ticketRef && <span className="font-[var(--font-mono)]">{d.ticketRef} </span>}{d.title}
                        </div>
                      ))}
                    </div>
                  </Prop>
                )}
                <Prop label="Labels">
                  <LabelPicker
                    value={t.tags}
                    options={boardLabels.map((l) => l.name)}
                    onChange={(next) => save({ tags: next })}
                    disabled={!canEdit}
                    size="sm"
                  />
                </Prop>
                <Prop label={`Watchers (${data!.watchers.length})`}>
                  <div className="space-y-1">
                    {data!.watchers.map((w) => <div key={w} className="truncate text-xs text-muted">{w}</div>)}
                    {me && (
                      <button className="text-xs text-accent hover:underline"
                        onClick={async () => { data!.watchers.includes(me) ? await unwatchTask(taskId, me) : await watchTask(taskId, me); refresh() }}>
                        {data!.watchers.includes(me) ? 'Unwatch' : 'Watch'}
                      </button>
                    )}
                  </div>
                </Prop>

                <div className="space-y-1 border-t border-line-subtle pt-3 text-[11px] text-muted">
                  <div>Created {relativeTime(t.createdAt)}</div>
                  <div>Updated {relativeTime(t.updatedAt)}</div>
                  {t.completedAt && <div>Completed {relativeTime(t.completedAt)}</div>}
                </div>
                </div>

                {canEdit && (
                  <div className="flex items-center gap-2 p-3 pt-0">
                    <button
                      onClick={async () => { await archiveTask(taskId, !t.archivedAt); refresh(); onClose() }}
                      className="flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-line px-2 py-1.5 text-xs text-muted transition-colors hover:bg-card hover:text-fg"
                    >
                      {t.archivedAt ? <ArchiveRestore size={14} /> : <Archive size={14} />}
                      {t.archivedAt ? 'Restore' : 'Archive'}
                    </button>
                    <button
                      onClick={async () => { await deleteTask(taskId); refresh(); onClose() }}
                      className="flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-[color:var(--theme-danger)]/40 px-2 py-1.5 text-xs text-[color:var(--theme-danger)] transition-colors hover:bg-[color:var(--theme-danger)]/10"
                    >
                      <Trash2 size={14} />
                      Delete
                    </button>
                  </div>
                )}
              </aside>
            </>
          )}
        </motion.div>
      </div>
    </AnimatePresence>
  )
}

// Description with a Read (rendered markdown) / Edit (WYSIWYG) toggle plus an
// expand button for comfortable full-screen reading. Keeps a local draft so the
// read view reflects edits without refetching the ticket.
function DescriptionSection({
  title,
  value,
  canEdit,
  mentions,
  onSave,
}: {
  title: string
  value: string
  canEdit: boolean
  mentions?: Mentionable[]
  onSave: (md: string) => void
}) {
  const [draft, setDraft] = useState(value)
  const [mode, setMode] = useState<'read' | 'edit'>(canEdit && !value ? 'edit' : 'read')
  const [reading, setReading] = useState(false)
  // Bumped on every save so the other (unfocused) editor instance remounts with
  // the latest draft — keeps the inline + expanded views in sync.
  const [rev, setRev] = useState(0)

  const save = (md: string) => {
    setDraft(md)
    setRev((r) => r + 1)
    onSave(md)
  }

  const ModeToggle = () =>
    canEdit ? (
      <div className="flex rounded-md border border-line p-0.5">
        {(['read', 'edit'] as const).map((m) => (
          <button
            key={m}
            onClick={() => setMode(m)}
            className={cn(
              'rounded px-2 py-0.5 text-[11px] capitalize transition-colors',
              mode === m ? 'bg-card text-fg' : 'text-muted hover:text-fg',
            )}
          >
            {m}
          </button>
        ))}
      </div>
    ) : null

  const body = (keyPrefix: string, minHeight: string, readMax?: string) =>
    mode === 'edit' && canEdit ? (
      <RichEditor key={`${keyPrefix}-${rev}`} value={draft} editable mentions={mentions} onSave={save} placeholder="Add detail" minHeight={minHeight} />
    ) : draft ? (
      <div className={cn('rounded-xl border border-line-subtle bg-card px-4 py-3 text-sm leading-relaxed', readMax && `${readMax} overflow-y-auto`)}>
        <Markdown>{draft}</Markdown>
      </div>
    ) : (
      <div className="rounded-xl border border-dashed border-line-subtle px-4 py-6 text-center text-xs text-muted">
        No description{canEdit ? '. Switch to Edit to add one.' : '.'}
      </div>
    )

  return (
    <div>
      <div className="mb-2 flex items-center gap-2">
        <div className="text-xs font-semibold uppercase tracking-wide text-muted">Description</div>
        <div className="ml-auto flex items-center gap-1">
          <ModeToggle />
          <button
            onClick={() => setReading(true)}
            title="Expand"
            aria-label="Expand description"
            className="grid h-6 w-6 place-items-center rounded text-muted transition-colors hover:bg-card hover:text-fg"
          >
            <Maximize2 size={13} />
          </button>
        </div>
      </div>

      {body('inline', '16rem')}

      {/* Expanded view — slides in over the whole ticket modal (no stacked modal).
          The modal panel is `relative`, so inset-0 covers it edge to edge. */}
      <AnimatePresence>
        {reading && (
          <motion.div
            className="mercury-panel absolute inset-0 z-30 flex flex-col overflow-hidden rounded-2xl"
            initial={{ x: '100%' }}
            animate={{ x: 0 }}
            exit={{ x: '100%' }}
            transition={{ duration: 0.22, ease: [0.4, 0, 0.2, 1] }}
          >
            <div className="flex items-center gap-3 border-b border-line-subtle px-5 py-3">
              <button
                onClick={() => setReading(false)}
                className="flex items-center gap-1 rounded-lg px-2 py-1 text-sm text-muted transition-colors hover:bg-card hover:text-fg"
              >
                <ChevronLeft size={16} /> Back
              </button>
              <div className="min-w-0 flex-1 truncate text-center text-sm font-semibold text-fg">{title}</div>
              <div className="flex w-[4.5rem] justify-end">
                <ModeToggle />
              </div>
            </div>
            {mode === 'edit' && canEdit ? (
              <div className="min-h-0 flex-1">
                <RichEditor key={`exp-${rev}`} value={draft} editable bare fill mentions={mentions} onSave={save} placeholder="Add detail" />
              </div>
            ) : (
              <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
                {draft ? (
                  <div className="mx-auto max-w-2xl text-sm leading-relaxed">
                    <Markdown>{draft}</Markdown>
                  </div>
                ) : (
                  <div className="text-sm text-muted">No description yet.</div>
                )}
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

/** Accumulated agent time → compact "2h 15m" / "45m" / "30s" / "—". */
function formatDuration(seconds: number): string {
  if (!seconds) return '—'
  const h = Math.floor(seconds / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  if (h) return m ? `${h}h ${m}m` : `${h}h`
  if (m) return `${m}m`
  return `${seconds}s`
}

// Files + knowledge/artifact refs pinned to the ticket. Same chips as chat;
// every change saves immediately (the list on the ticket IS the state).
function AttachmentsSection({ task, canEdit, onSaved }: { task: Task; canEdit: boolean; onSaved: () => void }) {
  const items = task.attachments ?? []
  const save = (next: Attachment[]) => {
    void updateTask(task.id, splitAttachments(next)).then(onSaved)
  }
  if (!items.length && !canEdit) return null
  return (
    <Section label="Attachments">
      {items.length > 0 && (
        <div className="mb-2 flex flex-wrap gap-2">
          {items.map((a) => (
            <span key={a.id} className="inline-flex items-center gap-2 rounded-lg border border-line-subtle bg-card/50 px-2.5 py-1.5 text-xs">
              {a.refType ? (
                <Link
                  to={a.refType === 'kb-doc' ? '/knowledge' : '/artifacts'}
                  className="inline-flex max-w-48 items-center gap-2 truncate text-fg transition-colors hover:text-accent"
                >
                  {a.refType === 'kb-doc' ? <BookOpen size={14} className="shrink-0 text-muted" /> : <Gem size={14} className="shrink-0 text-muted" />}
                  <span className="truncate">{a.filename}</span>
                </Link>
              ) : (
                <a href={attachmentUrl(a.id)} target="_blank" rel="noreferrer" className="inline-flex max-w-48 items-center gap-2 truncate text-fg transition-colors hover:text-accent">
                  {isImage(a.mime) ? (
                    <img src={attachmentUrl(a.id)} alt="" className="h-5 w-5 rounded object-cover" />
                  ) : (
                    <FileText size={14} className="shrink-0 text-muted" />
                  )}
                  <span className="truncate">{a.filename}</span>
                  {a.size > 0 && <span className="text-muted">{humanSize(a.size)}</span>}
                </a>
              )}
              {canEdit && (
                <button
                  type="button"
                  onClick={() => save(items.filter((x) => x.id !== a.id))}
                  className="text-muted hover:text-[color:var(--theme-danger)]"
                  title="Remove attachment"
                >
                  <X size={13} />
                </button>
              )}
            </span>
          ))}
        </div>
      )}
      {canEdit && <AttachButton disabled={false} onAttach={(a) => save([...items.filter((x) => x.id !== a.id), a])} />}
    </Section>
  )
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">{label}</div>
      {children}
    </div>
  )
}
/** Inline add for sub-tasks — quiet trigger, Enter creates, Esc closes. */
function SubtaskAdd({ onAdd }: { onAdd: (title: string) => Promise<void> | void }) {
  const [open, setOpen] = useState(false)
  const [title, setTitle] = useState('')
  const submit = () => {
    const v = title.trim()
    if (!v) return setOpen(false)
    setTitle('')
    setOpen(false)
    void onAdd(v)
  }
  if (!open)
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="w-full rounded-md px-1 py-0.5 text-left text-xs text-muted transition-colors hover:text-accent"
      >
        + Add sub-task
      </button>
    )
  return (
    <Input
      autoFocus
      value={title}
      onChange={(e) => setTitle(e.target.value)}
      onKeyDown={(e) => (e.key === 'Enter' ? submit() : e.key === 'Escape' ? setOpen(false) : null)}
      onBlur={submit}
      placeholder="Sub-task title"
      size="sm"
      className="w-full"
    />
  )
}

function Prop({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">{label}</div>
      {children}
    </div>
  )
}
function ResultBlock({ title, danger, children }: { title: string; danger?: boolean; children: React.ReactNode }) {
  return (
    <div className="mb-2 rounded-lg border border-line-subtle bg-card p-2">
      <div className="mb-0.5 text-[11px] uppercase tracking-wide" style={danger ? { color: 'var(--theme-danger)' } : undefined}>{title}</div>
      <div className="whitespace-pre-wrap text-sm text-fg">{children}</div>
    </div>
  )
}

// The QA judge's advisory verdict, shown above the human approval gate.
function JudgeVerdict({ review }: { review: JudgeReview }) {
  const tone =
    review.verdict === 'pass'
      ? { color: 'var(--theme-success)', label: 'Pass' }
      : review.verdict === 'revise'
        ? { color: 'var(--theme-warning)', label: 'Revise' }
        : { color: 'var(--theme-danger)', label: 'Escalate' }
  return (
    <div className="mb-2 rounded-lg border border-line-subtle bg-card p-2.5 text-sm">
      <div className="mb-1 flex items-center gap-2">
        <span className="text-[11px] uppercase tracking-wide text-muted">QA judge</span>
        <span className="rounded px-1.5 py-0.5 text-[11px] font-semibold" style={{ color: tone.color, border: `1px solid ${tone.color}` }}>{tone.label}</span>
        {review.model && <span className="text-[11px] text-muted">{review.model}</span>}
        <span className="ml-auto text-[11px] text-muted">advisory</span>
      </div>
      {review.summary && <div className="text-fg">{review.summary}</div>}
      {review.issues.length > 0 && (
        <ul className="mt-1.5 list-disc space-y-0.5 pl-4 text-[13px] text-muted">
          {review.issues.map((i, n) => <li key={n}>{i}</li>)}
        </ul>
      )}
    </div>
  )
}
