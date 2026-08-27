<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Archive, ArchiveRestore, Trash2 } from '@lucide/svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import type { RichEditorHandle } from '@/components/ui/rich-editor'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import CopyLinkButton from '@/components/ui/CopyLinkButton.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import LabelPicker from '@/components/board/LabelPicker.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import { useAgents } from '@/lib/agents'
  import { formatCost, formatTokens } from '@/lib/cost.svelte'
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
  } from '@/lib/boards.svelte'
  import { navigate } from '@/router'
  import { userAssignee } from '@/lib/assignees'
  import { userMentionInsert } from '@/components/chat/mentions.svelte'
  import ColorPill from '@/components/board/ColorPill.svelte'
  import { dateInputValue, dueIsoFromDateInput, startIsoFromDateInput } from '@/lib/dates'
  import {
    EFFORTS,
    EFFORT_LABEL,
    OFF_BOARD_STATUSES,
    PRIORITIES,
    PRIORITY_ICON,
    STATUS_LABEL,
    TASK_STATUSES,
    type Effort,
    type Priority,
    type TaskStatus,
  } from '@/lib/task-const'
  import { relativeTime } from '@/lib/fleet'
  import type { TicketMusePatch } from '@/lib/muse.svelte'
  import { statusLabelOf, useBoardStatuses } from '@/lib/statuses'
  import { cn } from '@/lib/cn'
  import { fade, listStagger, slide, QUICK } from '@/lib/motion'
  import DescriptionSection from './DescriptionSection.svelte'
  import AttachmentsSection from './AttachmentsSection.svelte'
  import JudgeVerdict from './JudgeVerdict.svelte'
  import Prop from './Prop.svelte'
  import ResultBlock from './ResultBlock.svelte'
  import Section from './Section.svelte'
  import SubtaskAdd from './SubtaskAdd.svelte'
  import TicketMuseBar from './TicketMuseBar.svelte'
  import WorkbenchJobsStrip from './WorkbenchJobsStrip.svelte'

  const MOVE: TaskStatus[] = [...TASK_STATUSES, ...OFF_BOARD_STATUSES]

  // Linear/Plane-style ticket: content (left) + properties rail (right).
  let { taskId, board, onClose }: { taskId: string; board: Board; onClose: () => void } = $props()

  const qc = useQueryClient()
  // Three answers, three faces. `data === undefined` is still in flight,
  // `data === null` is the 404 getJsonOr404 hands back (deleted, or never
  // yours), and isError is a real failure. Collapsing all three into the
  // skeleton left a modal of shimmering placeholders on screen for ever, with
  // no words on it at all, for a ticket that simply no longer exists.
  const taskQuery = useTask(() => taskId)
  const data = $derived(taskQuery.data)
  const fleetQuery = useAgents()
  const sessionQuery = useSession()
  const boardCfgQuery = useBoardAgents(() => board.id)
  const user = $derived(sessionQuery.data)
  const allAgents = $derived(fleetQuery.data?.agents ?? [])
  // Restrict the assignee list to the board's agent policy (allow-all or list).
  const agents = $derived(
    boardCfgQuery.data?.allowAll ? allAgents : allAgents.filter((a) => boardCfgQuery.data?.models.includes(a.id)),
  )
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  const me = $derived(user?.email ?? user?.name ?? '')
  // Board tickets for the dependency picker (exclude self + already-linked).
  // Four `{ data: x = [] }` defaults used to live here, and each one furnished a
  // control with a confident wrong answer: no dependencies to link, no
  // teammates to assign, no labels on this board, and a Status menu quietly
  // showing the hard-coded fallback instead of this board's own columns. None
  // of the four could reach its error — the query object was thrown away on the
  // same line it was created. `listQuery` hands back the rows AND the sentence.
  const tasksList = listQuery(useBoardTasks(() => board.id), { title: 'Could not load this board’s tickets', variant: 'inline' })
  const boardTasks = $derived(tasksList.rows)
  // @mention board members in comments + description — the people the server
  // notifies (tasks comment/description paths). Tokens mirror the server's.
  const membersList = listQuery(useBoardMembers(() => board.id), { title: 'Could not load who’s on this board', variant: 'inline' })
  const boardMembers = $derived(membersList.rows)
  const labelsList = listQuery(useBoardLabels(() => board.id), { title: 'Could not load this board’s labels', variant: 'inline' })
  const boardLabels = $derived(labelsList.rows)
  const statusesList = listQuery(useBoardStatuses(() => board.id), { title: 'Could not load this board’s columns', variant: 'inline' })
  const boardStatuses = $derived(statusesList.rows)
  const mentionables = $derived(
    boardMembers
      .map((m) => ({ insert: userMentionInsert(m), label: m.name ?? m.email ?? m.userId, sub: m.email ?? undefined }))
      .filter((m) => m.insert),
  )
  // Assignees mix humans (board members, `user:<id>`) and the board's agents.
  const assigneeOptions = $derived([
    ...boardMembers.map((m) => ({
      value: userAssignee(m.userId),
      label: (m.name ?? m.email ?? 'teammate') + (m.userId === user?.id ? ' (me)' : ''),
      sub: 'teammate',
    })),
    ...agents.map((a) => ({ value: a.id, label: a.label, sub: a.role })),
  ])

  let title = $state('')
  let tab = $state<'comments' | 'activity'>('comments')
  const tabs = ['comments', 'activity'] as const
  let commentEditor = $state<RichEditorHandle | null>(null)
  let descEditor = $state<RichEditorHandle | null>(null)
  // Jump to a sibling ticket (parent/sub-task links) — same overlay route.
  // (React's `search: prev => prev` kept the board filters; pass the current
  // query string through for the same effect.)
  const openTask = (id: string) =>
    void navigate('/boards/:boardId/:taskId', {
      params: { boardId: board.id, taskId: id },
      search: Object.fromEntries(new URLSearchParams(location.search)),
    })

  // Initialise editable fields ONCE per task (not on every refetch) so live
  // updates behind the modal don't reset what the user is typing.
  let loadedId: string | null = null
  $effect(() => {
    const task = taskQuery.data?.task
    if (task && loadedId !== task.id) {
      loadedId = task.id
      title = task.title
    }
  })

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['task', taskId] })
    void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  }
  const save = async (patch: Parameters<typeof updateTask>[1]) => {
    await updateTask(taskId, patch)
    refresh()
  }
  const t = $derived(data?.task)

  // Sub-tasks: one level deep. Children list + inline add on a
  // parent; a child shows its parent with a promote control.
  const parentTask = $derived(t?.parentId ? boardTasks.find((bt) => bt.id === t.parentId) : undefined)
  const subTasks = $derived(t ? boardTasks.filter((bt) => bt.parentId === t.id) : [])

  /** Accumulated agent time → compact "2h 15m" / "45m" / "30s" / "—". */
  function formatDuration(seconds: number): string {
    if (!seconds) return '—'
    const h = Math.floor(seconds / 3600)
    const m = Math.floor((seconds % 3600) / 60)
    if (h) return m ? `${h}h ${m}m` : `${h}h`
    if (m) return `${m}m`
    return `${seconds}s`
  }
</script>

<!-- The one Modal primitive (fixed height + unpadded): the ticket detail is
     content INSIDE it, not its own hand-rolled shell. Entrance/exit, backdrop,
     Escape, and portal all come from Modal. -->
<Modal open onClose={onClose} width="max-w-4xl" height="h-[85vh]" padded={false}>
  <div class="flex h-full w-full overflow-hidden">
    {#if taskQuery.isError && data === undefined}
      <div class="grid h-full w-full place-items-center p-6">
        <CloseButton onClick={onClose} class="absolute right-3 top-3" />
        <QueryError error={taskQuery.error} title="Could not load this ticket" onRetry={() => void taskQuery.refetch()} />
      </div>
    {:else if data === null}
      <div class="grid h-full w-full place-items-center p-6">
        <CloseButton onClick={onClose} class="absolute right-3 top-3" />
        <EmptyState
          icon="⧉"
          title="This ticket no longer exists"
          hint="It was deleted, or you no longer have access to it."
        >
          {#snippet action()}
            <Button variant="outline" size="sm" onclick={onClose}>
              Back to the board
            </Button>
          {/snippet}
        </EmptyState>
      </div>
    {:else if !t}
      <div class="flex h-full w-full gap-6 p-6">
        <div class="min-w-0 flex-1 space-y-4">
          <Skeleton class="h-5 w-2/3 rounded-full" />
          <SkeletonRows rows={5} />
        </div>
        <div class="w-56 shrink-0 space-y-3">
          <SkeletonRows rows={6} />
        </div>
      </div>
    {:else}
      <!-- Content. LOCAL fades on purpose: on a cold load this branch toggles
           in after mount (skeleton → content) and the reveal plays; when the
           hover-prefetch already warmed the cache the branch renders AT mount,
           local intros stay quiet, and the modal entrance is the only motion. -->
      <div in:fade={{ duration: 250 }} class="flex min-w-0 flex-1 flex-col">
        <div class="flex items-center gap-2 border-b border-line-subtle px-5 py-2.5">
          {#if t.ticketRef}<span class="font-mono text-xs tracking-[0.05em] text-muted">{t.ticketRef}</span>{/if}
          <span class="text-xs text-muted">·</span>
          <span class="font-mono text-[11px] text-muted">opened by {t.createdBy}</span>
          {#if t.archivedAt}
            <span class="rounded-md border border-line bg-raised px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
              Archived
            </span>
          {/if}
          <CopyLinkButton
            path={`/boards/${board.id}/${taskId}`}
            label="Copy link"
            title="Copy link to this ticket"
            class="ml-auto px-1.5 py-0.5 text-xs"
          />
        </div>

        <!-- Details — scrolls independently, capped so discussion gets room -->
        <div class="max-h-[46%] shrink-0 space-y-5 overflow-y-auto px-5 py-4">
          <Input
            bind:value={title}
            disabled={!canEdit}
            onblur={() => title.trim() && title !== t.title && void save({ title: title.trim() })}
            onkeydown={inlineEditKeys(() => (title = t!.title))}
            class="border-0 bg-transparent px-0 font-sans text-lg font-semibold focus:border-0"
          />

          <!-- QA judge verdict (advisory) — most recent first -->
          {#if t.status === 'quality_review' && data?.judgeReviews?.[0]}<JudgeVerdict review={data.judgeReviews[0]} />{/if}

          <!-- Approval gate -->
          {#if t.status === 'quality_review' && canEdit}
            <div
              transition:slide={{ duration: 150 }}
              class="flex items-center gap-2 rounded-lg border border-[color:var(--theme-accent-border)] bg-accent-soft p-2 font-sans text-sm"
            >
              <span class="flex-1 text-fg">Ready for review. Approve to complete.</span>
              <Button
                size="sm"
                onclick={async () => {
                  await reviewTask(taskId, 'approved')
                  refresh()
                }}>Approve</Button
              >
              <Button
                variant="outline"
                size="sm"
                onclick={async () => {
                  await reviewTask(taskId, 'rejected')
                  refresh()
                }}>Request changes</Button
              >
            </div>
          {/if}

          <WorkbenchJobsStrip {taskId} {canEdit} />

          {#key `ds-${t.id}`}
            <DescriptionSection
              bind:editor={descEditor}
              title={t.ticketRef ? `${t.ticketRef} · ${t.title}` : t.title}
              value={t.description ?? ''}
              {canEdit}
              mentions={mentionables}
              onSave={(md) => {
                // RichEditor only fires this on a real change. Refresh just
                // the board's cards — never refetch the open ticket.
                void updateTask(taskId, { description: md || null }).then(() =>
                  qc.invalidateQueries({ queryKey: ['board-tasks', board.id] }),
                )
              }}
            />
          {/key}

          <AttachmentsSection task={t} {canEdit} onSaved={() => qc.invalidateQueries({ queryKey: ['task', taskId] })} />

          <!-- Agent-reported result -->
          {#if t.outcome || t.resolution || t.errorMessage}
            <Section label="Result">
              {#if t.outcome}<ResultBlock title="Outcome">{t.outcome}</ResultBlock>{/if}
              {#if t.resolution}<ResultBlock title="Resolution">{t.resolution}</ResultBlock>{/if}
              {#if t.errorMessage}<ResultBlock title="Error" danger>{t.errorMessage}</ResultBlock>{/if}
            </Section>
          {/if}
        </div>

        <!-- Discussion — Comments / Activity tabs; comment composer pinned -->
        <div class="flex min-h-0 flex-1 flex-col border-t border-line-subtle">
          <div class="flex items-center gap-1 px-5 pt-3">
            {#each tabs as tb (tb)}
              <button
                onclick={() => (tab = tb)}
                class={cn(
                  'rounded-md px-3 py-1 font-mono text-[10px] font-medium uppercase tracking-[0.05em] transition-colors',
                  tab === tb ? 'bg-raised text-fg' : 'text-muted hover:text-fg',
                )}
              >
                {tb === 'comments' ? `Comments (${data!.comments.length})` : 'Activity'}
              </button>
            {/each}
          </div>

          {#if tab === 'comments'}
            <ul class="min-h-0 flex-1 space-y-2 overflow-y-auto px-5 py-3" use:listStagger>
              {#each data!.comments as c (c.id)}
                <li in:fade={{ duration: 150 }} out:fade={QUICK} class="rounded-lg border border-line bg-card p-2">
                  <div class="mb-0.5 flex items-center justify-between text-xs">
                    <span class="font-mono text-[11px] tracking-[0.05em] text-accent">{c.author}</span>
                    <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(c.createdAt)}</span>
                  </div>
                  <div class="font-sans text-sm text-fg"><Markdown children={c.content} /></div>
                </li>
              {/each}
              {#if data!.comments.length === 0}<li class="font-sans text-xs text-muted">No comments yet.</li>{/if}
            </ul>
            {#key `comment-${t.id}`}
              <RichEditor
                bind:this={commentEditor}
                value=""
                editable
                bare
                mentions={mentionables}
                class="shrink-0 border-t border-line-subtle"
                placeholder="Write a comment  (Ctrl+Enter to send)"
                minHeight="5rem"
                onSubmit={() => {
                  const md = (commentEditor?.getMarkdown() ?? '').trim()
                  if (!md) return
                  commentEditor?.clear()
                  void addComment(taskId, md).then(refresh)
                }}
              />
            {/key}
          {:else}
            <ul class="min-h-0 flex-1 space-y-1 overflow-y-auto px-5 py-3" use:listStagger>
              {#each data!.activity as a (a.id)}
                <li in:fade={{ duration: 150 }} out:fade={QUICK} class="flex items-center gap-2 text-xs text-muted">
                  <span class="font-mono text-[11px] tracking-[0.05em] text-accent">{a.actor}</span>
                  <span class="min-w-0 flex-1 truncate font-sans">{a.description}</span>
                  <span class="shrink-0 font-mono text-[10px] tracking-[0.05em]">{relativeTime(a.createdAt)}</span>
                </li>
              {/each}
              {#if data!.activity.length === 0}<li class="font-sans text-xs text-muted">No activity yet.</li>{/if}
            </ul>
          {/if}
        </div>

        <!-- Muse — fast natural-language edits: fields from the base
             view ("high priority, due friday"), or a selected passage
             of the description while editing. -->
        {#if canEdit}
          <TicketMuseBar
            {t}
            editor={descEditor}
            onPatch={async (patch: TicketMusePatch) => {
              await save(patch as Parameters<typeof save>[0])
            }}
          />
        {/if}
      </div>

      <!-- Properties rail -->
      <aside in:fade={{ duration: 250, delay: 60 }} class="flex w-60 shrink-0 flex-col border-l border-line-subtle bg-sidebar">
        <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
          <CloseButton onClick={onClose} class="-mr-1 ml-auto" />
          <Prop label="Status">
            <Select
              value={t.status}
              disabled={!canEdit}
              onchange={(e) => void save({ status: e.currentTarget.value as TaskStatus })}
              size="sm"
              class="w-full"
            >
              {#each boardStatuses.length ? [...boardStatuses.map((st) => st.key), ...OFF_BOARD_STATUSES] : MOVE as k (k)}
                <option value={k}>
                  {statusLabelOf(k, boardStatuses)}
                </option>
              {/each}
            </Select>
            <!-- Without this the menu silently degrades to the built-in
                 statuses and looks like the board simply has those. -->
            {#if statusesList.notice}<QueryError {...statusesList.notice} />{/if}
          </Prop>
          <Prop label="Priority">
            <Select
              value={t.priority}
              disabled={!canEdit}
              onchange={(e) => void save({ priority: e.currentTarget.value as Priority })}
              size="sm"
              class="w-full"
            >
              {#each PRIORITIES as p (p)}<option value={p}>{PRIORITY_ICON[p]} {p}</option>{/each}
            </Select>
          </Prop>
          <Prop label="Color">
            <ColorPill value={t.color} onChange={(c) => void save({ color: c as Parameters<typeof save>[0]['color'] })} disabled={!canEdit} />
          </Prop>
          <Prop label="Assignees">
            <Combobox
              options={assigneeOptions}
              selected={t.assignees}
              onChange={(arr) => canEdit && void save({ assignees: arr })}
              disabled={!canEdit}
              multiple
              size="sm"
              placeholder="Unassigned"
            />
            {#if membersList.notice}<QueryError {...membersList.notice} />{/if}
          </Prop>
          <div class="grid grid-cols-2 gap-2">
            <Prop label="Effort">
              <Select
                value={t.effort ?? ''}
                disabled={!canEdit}
                onchange={(e) => void save({ effort: (e.currentTarget.value || null) as Effort | null })}
                size="sm"
                class="w-full"
              >
                <option value="">—</option>
                {#each EFFORTS as ef (ef)}<option value={ef}>{EFFORT_LABEL[ef]}</option>{/each}
              </Select>
            </Prop>
            <Prop label="Estimate (h)">
              {#key `est-${t.id}-${t.estimatedHours ?? ''}`}
                <Input
                  type="number"
                  min={0}
                  max={999}
                  step={0.5}
                  size="sm"
                  disabled={!canEdit}
                  value={t.estimatedHours ?? ''}
                  onblur={(e) => {
                    const v = (e.target as HTMLInputElement).value.trim()
                    const n = v === '' ? null : Number(v)
                    if (n !== t!.estimatedHours && (n === null || (!Number.isNaN(n) && n >= 0))) void save({ estimatedHours: n })
                  }}
                  placeholder="—"
                  class="w-full"
                />
              {/key}
            </Prop>
          </div>
          <div class="grid grid-cols-2 gap-2">
            <Prop label="Time spent">
              <div class="flex h-9 items-center text-sm text-fg">{formatDuration(t.timeSpentSeconds)}</div>
            </Prop>
          </div>
          <!-- Agent-reported token spend (MCP log_usage) — priced like the ledger. -->
          {#if data!.usage.promptTokens + data!.usage.completionTokens > 0}
            <Prop label="Tokens">
              <div class="space-y-1 text-sm text-fg">
                <div>
                  {formatTokens(data!.usage.promptTokens + data!.usage.completionTokens)}
                  {#if data!.usage.cost > 0}<span class="text-muted"> · {formatCost(data!.usage.cost)}</span>{/if}
                  {#if data!.usage.unpricedTokens > 0}<span class="text-muted"> · partly unpriced</span>{/if}
                </div>
                {#each data!.usage.perModel as m (m.llmModel ?? '?')}
                  <div class="truncate text-xs text-muted">
                    {m.llmModel ?? 'unattributed'} · {formatTokens(m.tokens)}
                    {m.cost !== null && m.cost > 0 ? ` · ${formatCost(m.cost)}` : ''}
                  </div>
                {/each}
              </div>
            </Prop>
          {/if}
          <div class="grid grid-cols-2 gap-2">
            <!-- Dates go through the shared local-day helpers: a picked
                 date is an instant at 09:00/17:00 LOCAL, same as the due
                 pill, the quick-picks and the Gantt. Writing
                 `new Date(value)` here stored UTC midnight instead, so the
                 same field meant a different instant depending on which
                 surface you edited it from. -->
            <Prop label="Start date">
              <Input
                type="date"
                value={dateInputValue(t.startDate)}
                disabled={!canEdit}
                oninput={(e) => {
                  const v = e.currentTarget.value
                  const iso = v ? startIsoFromDateInput(v) : null
                  if (!v || iso) void save({ startDate: iso })
                }}
                size="sm"
                class="w-full"
              />
            </Prop>
            <Prop label="Due date">
              <Input
                type="date"
                value={dateInputValue(t.dueDate)}
                disabled={!canEdit}
                oninput={(e) => {
                  const v = e.currentTarget.value
                  const iso = v ? dueIsoFromDateInput(v) : null
                  if (!v || iso) void save({ dueDate: iso })
                }}
                size="sm"
                class="w-full"
              />
            </Prop>
          </div>
          <!-- Sub-tasks: one level deep. Children list + inline add on a
               parent; a child shows its parent with a promote control. -->
          {#if t.parentId}
            <Prop label="Parent">
              <div class="flex items-center gap-1 text-xs">
                <button
                  onclick={() => {
                    if (parentTask) openTask(parentTask.id)
                  }}
                  class="min-w-0 flex-1 truncate text-left text-muted transition-colors hover:text-fg"
                >
                  {#if parentTask}
                    {#if parentTask.ticketRef}<span class="font-mono">{parentTask.ticketRef} </span>{/if}
                    {parentTask.title}
                  {:else}
                    parent ticket
                  {/if}
                </button>
                {#if canEdit}
                  <button
                    onclick={() => void save({ parentId: null })}
                    title="Promote to top level"
                    class="shrink-0 text-muted hover:text-fg"
                  >
                    ✕
                  </button>
                {/if}
              </div>
            </Prop>
          {:else}
            <Prop label={`Sub-tasks (${subTasks.length})`}>
              <div class="space-y-1" use:listStagger>
                {#each subTasks as st (st.id)}
                  <div class="flex items-center gap-1.5 text-xs">
                    <StatusDot status={st.status === 'done' ? 'ok' : 'idle'} />
                    <button
                      onclick={() => openTask(st.id)}
                      class={cn('min-w-0 flex-1 truncate text-left transition-colors hover:text-fg', st.status === 'done' ? 'text-muted line-through' : 'text-muted')}
                    >
                      {#if st.ticketRef}<span class="font-mono">{st.ticketRef} </span>{/if}
                      {st.title}
                    </button>
                  </div>
                {/each}
                {#if canEdit}
                  <SubtaskAdd
                    onAdd={async (subtaskTitle) => {
                      await createTask(board.id, { title: subtaskTitle, parentId: t!.id })
                      void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
                      refresh()
                    }}
                  />
                {/if}
              </div>
            </Prop>
          {/if}
          <Prop label={`Blocked by (${data!.blockedBy.length})`}>
            <div class="space-y-1">
              {#each data!.blockedBy as d (d.id)}
                <div class="flex items-center gap-1 text-xs">
                  <span class="min-w-0 flex-1 truncate text-muted">
                    {#if d.ticketRef}<span class="font-mono">{d.ticketRef} </span>{/if}{d.title}
                  </span>
                  {#if canEdit}
                    <button
                      onclick={async () => {
                        await removeDependency(taskId, d.id)
                        refresh()
                      }}
                      class="shrink-0 text-muted transition-colors hover:text-danger">✕</button
                    >
                  {/if}
                </div>
              {/each}
              {#if data!.blockedBy.length === 0}<div class="text-xs text-muted">None</div>{/if}
              {#if canEdit}
                <Combobox
                  options={boardTasks
                    .filter((bt) => bt.id !== taskId && !data!.blockedBy.some((b) => b.id === bt.id))
                    .map((bt) => ({ value: bt.id, label: `${bt.ticketRef ? bt.ticketRef + ' ' : ''}${bt.title}`, sub: STATUS_LABEL[bt.status] }))}
                  selected={[]}
                  onChange={async (arr) => {
                    if (arr[0]) {
                      await addDependency(taskId, arr[0])
                      refresh()
                    }
                  }}
                  size="sm"
                  placeholder="Add dependency"
                />
              {/if}
              <!-- The picker is fed by the board's ticket list. When that
                   read fails it offers nothing, which reads as "this board
                   has no other tickets" — say what actually happened. -->
              {#if tasksList.notice}<QueryError {...tasksList.notice} />{/if}
            </div>
          </Prop>
          {#if data!.blocks.length > 0}
            <Prop label={`Blocks (${data!.blocks.length})`}>
              <div class="space-y-1">
                {#each data!.blocks as d (d.id)}
                  <div class="truncate text-xs text-muted">
                    {#if d.ticketRef}<span class="font-mono">{d.ticketRef} </span>{/if}{d.title}
                  </div>
                {/each}
              </div>
            </Prop>
          {/if}
          <Prop label="Labels">
            <LabelPicker
              value={t.tags}
              options={boardLabels.map((l) => l.name)}
              onChange={(next) => void save({ tags: next })}
              disabled={!canEdit}
              size="sm"
            />
            {#if labelsList.notice}<QueryError {...labelsList.notice} />{/if}
          </Prop>
          <Prop label={`Watchers (${data!.watchers.length})`}>
            <div class="space-y-1">
              {#each data!.watchers as w (w)}<div class="truncate text-xs text-muted">{w}</div>{/each}
              {#if me}
                <button
                  class="text-xs text-accent hover:underline"
                  onclick={async () => {
                    if (data!.watchers.includes(me)) await unwatchTask(taskId, me)
                    else await watchTask(taskId, me)
                    refresh()
                  }}
                >
                  {data!.watchers.includes(me) ? 'Unwatch' : 'Watch'}
                </button>
              {/if}
            </div>
          </Prop>

          <div class="space-y-1 border-t border-line-subtle pt-3 font-mono text-[10px] tracking-[0.05em] text-muted">
            <div>Created {relativeTime(t.createdAt)}</div>
            <div>Updated {relativeTime(t.updatedAt)}</div>
            {#if t.completedAt}<div>Completed {relativeTime(t.completedAt)}</div>{/if}
          </div>
        </div>

        {#if canEdit}
          <div class="flex items-center gap-2 p-3 pt-0">
            <!-- Secondary: raised tile + hairline + readout mono (spec §8). -->
            <Button variant="outline" size="xs" class="flex-1 gap-1.5 py-1.5" onclick={async () => {
                await archiveTask(taskId, !t!.archivedAt)
                refresh()
                onClose()
              }}>
              {#if t.archivedAt}<ArchiveRestore size={14} />{:else}<Archive size={14} />{/if}
              {t.archivedAt ? 'Restore' : 'Archive'}
            </Button>
            <!-- Destructive: ORANGE OUTLINE — never an orange fill (spec §8). -->
            <Button variant="ghost" size="xs" class="flex-1 gap-1.5 border border-danger py-1.5 text-danger hover:bg-danger/10" onclick={async () => {
                await deleteTask(taskId)
                refresh()
                onClose()
              }}>
              <Trash2 size={14} />
              Delete
            </Button>
          </div>
        {/if}
      </aside>
    {/if}
  </div>
</Modal>
