<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Archive, ArchiveRestore, Eye, Trash2 } from '@lucide/svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import type { RichEditorHandle } from '@/components/ui/rich-editor'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import CopyLinkButton from '@/components/ui/CopyLinkButton.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import type { TabItem } from '@/components/ui/tabs'
  import Select from '@/components/ui/Select.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import LabelPicker from '@/components/board/LabelPicker.svelte'
  import ChannelView from '@/components/chat/ChannelView.svelte'
  import { useAgents } from '@/lib/agents'
  import { fmtDuration, formatTokens, formatUsd } from '@/lib/format'
  import { useSession } from '@/lib/session'
  import {
    addDependency,
    archiveTask,
    createTask,
    deleteTask,
    openTaskDiscussion,
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
  import WorkchainSection from './WorkchainSection.svelte'
  import { useBoardWorkchains } from '@/lib/workchain-client'
  import type { Workchain, WorkchainStep } from '@/lib/workchain-rules'
  import TicketMuseBar from './TicketMuseBar.svelte'
  import WorkbenchJobsStrip from './WorkbenchJobsStrip.svelte'
  import WorkbenchTicker from './WorkbenchTicker.svelte'
  import WorkLogStrip from './WorkLogStrip.svelte'
  import RunDetailModal from './RunDetailModal.svelte'
  import { useWorkSession } from '@/lib/work-session.svelte'

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

  // The ticket's chain membership, read off the board's workchains lens.
  // Cheap (one list read shared with the workchains view via the query key);
  // renders nothing when the ticket is in no chain.
  const chainsQuery = useBoardWorkchains(() => board.id)
  const myChain = $derived(
    (chainsQuery.data ?? []).find((w: Workchain) => w.steps.some((s: WorkchainStep) => s.taskId === taskId)) ?? null,
  )
  const myStep = $derived(myChain?.steps.find((s: WorkchainStep) => s.taskId === taskId) ?? null)
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
  // @mention board members in the description — the people the server notifies
  // (tasks comment/description paths). Tokens mirror the server's.
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
  type TicketTab = 'discussion' | 'attachments' | 'activity' | 'workchain' | 'result'
  let tab = $state<TicketTab>('discussion')
  let descEditor = $state<RichEditorHandle | null>(null)
  // DescriptionSection's read/edit mode is OURS now (it binds up): the muse
  // bar below gates on "the description is being edited".
  let descMode = $state<'read' | 'edit'>('read')
  // The ticket's discussion ROOM — a channel linked to the task. `roomId` is
  // this view's own record of the room it opened (or found); the link lives
  // server-side on the channel, and the ensure route is the door to it.
  let roomId = $state<string | null>(null)
  let openingRoom = $state(false)
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
      // An editable ticket with no description opens straight into Edit —
      // this initial used to be DescriptionSection's own.
      descMode = canEdit && !task.description ? 'edit' : 'read'
      // A sibling jump (openTask) swaps the ticket under this modal: the old
      // ticket's room is not the new one's.
      roomId = null
      roomTried = false
    }
  })

  // OPENING THE DISCUSSION TAB OPENS THE ROOM. A room is not created with
  // its ticket — most tickets are read and moved, and a channel row per one
  // of those would be a room nobody ever spoke in — so the ensure route is
  // what makes the room exist, and this is the thing that calls it. One
  // attempt per ticket: a refused ensure (409 — nobody can own the room)
  // must not re-POST on every refetch while the tab sits open.
  let roomTried = false
  $effect(() => {
    if (tab !== 'discussion' || !t) return
    if (roomTried || roomId) return
    roomTried = true
    openingRoom = true
    openTaskDiscussion(taskId)
      .then((r) => {
        roomId = r.channelId
        refresh()
      })
      .catch(() => {})
      .finally(() => (openingRoom = false))
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
  // The room's empty state: what an untouched discussion should say. The
  // assigned agent answers un-mentioned messages the relevance gate passes;
  // everything else is an @mention away — the hint teaches exactly that.
  const assignedAgent = $derived(t ? agents.find((a) => t.assignees.includes(a.id)) : undefined)
  const roomZeroHint = $derived(
    agents.length === 0
      ? 'This board has no agents.'
      : assignedAgent
        ? `${assignedAgent.label} is on this ticket — it answers what's relevant. @mention any agent to address it directly.`
        : 'No agent is assigned — @mention an agent to bring it into the room.',
  )

  // Sub-tasks: one level deep. Children list + inline add on a
  // parent; a child shows its parent with a promote control.
  const parentTask = $derived(t?.parentId ? boardTasks.find((bt) => bt.id === t.parentId) : undefined)
  const subTasks = $derived(t ? boardTasks.filter((bt) => bt.parentId === t.id) : [])
  const hasResult = $derived(!!(t && (t.outcome || t.resolution || t.errorMessage)))
  // Discussion, then Attachments — the two things a reader comes here for —
  // then Activity. Chain and Result appear only when the ticket has them,
  // so an empty tab is not another thing to miss.
  const tabItems = $derived.by((): TabItem<TicketTab>[] => {
    const items: TabItem<TicketTab>[] = [
      { id: 'discussion', label: `Discussion (${t?.commentCount ?? 0})` },
      { id: 'attachments', label: `Attachments (${t?.attachments.length ?? 0})` },
      { id: 'activity', label: 'Activity' },
    ]
    if (myChain) items.push({ id: 'workchain', label: 'Workchain' })
    if (hasResult) items.push({ id: 'result', label: 'Result' })
    return items
  })
  // A sibling jump, or a chain/result that goes away, must not leave the
  // strip pointing at a tab that is no longer rendered.
  $effect(() => {
    if (!tabItems.some((item) => item.id === tab)) tab = 'discussion'
  })

  // ── Watching the work ──────────────────────────────────────────────────
  // The header eye and the work log's View-log buttons share ONE run-detail
  // modal; `watchRun` is whichever run it is open on. The live-session read
  // is the same query the ticker holds (same key), so this adds no fetch.
  const workSession = useWorkSession(() => taskId)
  const live = $derived(workSession.data?.session ?? null)
  const queuedWait = $derived(workSession.data?.wait ?? null)
  let watchRun = $state<string | null>(null)
  // Both eyes open on Agent. Turns is the harness exchange, one tab over.
  let watchFocus = $state<'agent' | 'turns'>('agent')
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
      <div class="flex h-full w-full">
        <div class="flex min-w-0 flex-1 flex-col gap-4 px-5 py-4">
          <Skeleton class="h-7 w-2/3 rounded-md" />
          <Skeleton class="h-[clamp(9rem,24vh,14rem)] w-full rounded-lg" />
          <SkeletonRows rows={4} />
        </div>
        <div class="w-60 shrink-0 space-y-3 border-l border-line-subtle p-4">
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
          {#if live || queuedWait}
            <!-- The header eye — the same watch affordance the board surfaces
                 carry, opening the run-detail modal on the LIVE run. It sits
                 beside the copy link and, like the card corner's, waits
                 disabled while work is only queued (no run yet). -->
            <button
              type="button"
              title="Watch the work"
              aria-label="Watch the work"
              disabled={!live}
              onclick={() => {
                if (!live) return
                watchFocus = 'agent'
                watchRun = live.runId
              }}
              class="ml-auto flex items-center rounded-md p-1 text-accent transition-colors hover:text-fg disabled:opacity-50"
            >
              <Eye size={13} />
            </button>
          {/if}
          <CopyLinkButton
            path={`/boards/${board.id}/${taskId}`}
            label="Copy link"
            title="Copy link to this ticket"
            class={live || queuedWait ? 'px-1.5 py-0.5 text-xs' : 'ml-auto px-1.5 py-0.5 text-xs'}
          />
        </div>

        <!-- Title, then the description as a fixed anchor. Status banners sit
             above the description and scroll in their own cap, so a long work
             log cannot push the description off screen. Nothing stacks under
             the description — that zone is the tab pane. -->
        <div class="shrink-0 px-5 pt-4">
          <Input
            bind:value={title}
            disabled={!canEdit}
            onblur={() => title.trim() && title !== t.title && void save({ title: title.trim() })}
            onkeydown={inlineEditKeys(() => (title = t!.title))}
            class="border-0 bg-transparent px-0 font-sans text-lg font-semibold focus:border-0"
          />
          <div class="mt-4 max-h-36 space-y-3 overflow-y-auto empty:hidden">
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

            <WorkbenchTicker {taskId} />
            <!-- The record under the live strip: every session this ticket has
                 seen, each row opening the shared run-detail modal. -->
            <WorkLogStrip
              {taskId}
              onView={(runId) => {
                watchFocus = 'agent'
                watchRun = runId
              }}
            />
            <WorkbenchJobsStrip {taskId} {canEdit} />
          </div>
        </div>

        <div class="shrink-0 px-5 pb-4 pt-4">
          {#key `ds-${t.id}`}
            <DescriptionSection
              bind:mode={descMode}
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
        </div>

        <!-- Tabs fill whatever the description does not. The room itself is
             ChannelView: same composer, edits, reactions, threads, and
             @mention-driven agent replies as every other channel. -->
        <div class="flex min-h-0 flex-1 flex-col border-t border-line-subtle">
          <div class="px-5 pt-3">
            <Tabs items={tabItems} value={tab} onChange={(id) => (tab = id)} />
          </div>

          {#if tab === 'discussion'}
            {#if roomId}
              <div class="min-h-0 flex-1">
                <ChannelView
                  channelId={roomId}
                  channelName={t.title}
                  fleet={agents}
                  onLiveMessage={refresh}
                  zeroTitle="Nothing here yet"
                  zeroHint={roomZeroHint}
                  composerPlaceholder="Reply here — @mention an agent to bring it into the room"
                />
              </div>
            {:else if openingRoom}
              <div class="px-5 py-3"><SkeletonRows rows={3} /></div>
            {:else}
              <!-- The ensure route refused (409: nobody can own the room) —
                   say it, rather than a composer that cannot send. -->
              <div class="grid min-h-0 flex-1 place-items-center px-5 py-3 text-center font-sans text-xs text-muted">
                The discussion could not be opened for this ticket.
              </div>
            {/if}
          {:else if tab === 'attachments'}
            <AttachmentsSection task={t} {canEdit} onSaved={() => qc.invalidateQueries({ queryKey: ['task', taskId] })} />
          {:else if tab === 'workchain'}
            <div class="min-h-0 flex-1 overflow-y-auto px-5 py-3">
              <WorkchainSection
                chain={myChain}
                step={myStep}
                {canEdit}
                onChanged={() => {
                  void qc.invalidateQueries({ queryKey: ['board-workchains', board.id] })
                  void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
                }}
              />
            </div>
          {:else if tab === 'result'}
            <div class="min-h-0 flex-1 overflow-y-auto px-5 py-3">
              <Section label="Result">
                {#if t.outcome}<ResultBlock title="Outcome">{t.outcome}</ResultBlock>{/if}
                {#if t.resolution}<ResultBlock title="Resolution">{t.resolution}</ResultBlock>{/if}
                {#if t.errorMessage}<ResultBlock title="Error" danger>{t.errorMessage}</ResultBlock>{/if}
              </Section>
            </div>
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
             of the description while editing. Edit mode only: the bar
             acts on the description's editor, and the read view should
             read. -->
        {#if canEdit && descMode === 'edit'}
          <div transition:slide={{ duration: 150 }}>
            <TicketMuseBar
              {t}
              editor={descEditor}
              onPatch={async (patch: TicketMusePatch) => {
                await save(patch as Parameters<typeof save>[0])
              }}
            />
          </div>
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
              <div class="flex h-9 items-center text-sm text-fg">{fmtDuration(t.timeSpentSeconds)}</div>
            </Prop>
          </div>
          <!-- Agent-reported token spend (MCP log_usage) — priced like the ledger. -->
          {#if data!.usage.promptTokens + data!.usage.completionTokens > 0}
            <Prop label="Tokens">
              <div class="space-y-1 text-sm text-fg">
                <div>
                  {formatTokens(data!.usage.promptTokens + data!.usage.completionTokens)}
                  {#if data!.usage.cost > 0}<span class="text-muted"> · {formatUsd(data!.usage.cost)}</span>{/if}
                  {#if data!.usage.unpricedTokens > 0}<span class="text-muted"> · partly unpriced</span>{/if}
                </div>
                {#each data!.usage.perModel as m (m.llmModel ?? '?')}
                  <div class="truncate text-xs text-muted">
                    {m.llmModel ?? 'unattributed'} · {formatTokens(m.tokens)}
                    {m.cost !== null && m.cost > 0 ? ` · ${formatUsd(m.cost)}` : ''}
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

<!-- The one shared run-detail modal: the header eye (the live run) and the
     work log's View-log buttons (any retained run) both land here. Modal
     portals itself, so nesting inside the ticket modal's tree is safe. -->
{#if watchRun}
  <RunDetailModal open={!!watchRun} onClose={() => (watchRun = null)} runId={watchRun} {taskId} focus={watchFocus} onEnded={() => (watchRun = null)} />
{/if}
</Modal>
