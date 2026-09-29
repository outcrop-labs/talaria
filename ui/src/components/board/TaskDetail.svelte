<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Archive, ArchiveRestore, Check, Eye, SlidersHorizontal, Trash2 } from '@lucide/svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Chip from '@/components/ui/Chip.svelte'
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
  import { isReviewStatus, statusColorOf, statusLabelOf, useBoardStatuses } from '@/lib/statuses'
  import { cn } from '@/lib/cn'
  import { fade, fly, listStagger, slide, QUICK } from '@/lib/motion'
  import DescriptionSection from './DescriptionSection.svelte'
  import AttachmentsSection from './AttachmentsSection.svelte'
  import JudgeVerdict from './JudgeVerdict.svelte'
  import Prop from './Prop.svelte'
  import PropGroup from './PropGroup.svelte'
  import ResultBlock from './ResultBlock.svelte'
  import Section from './Section.svelte'
  import SubtaskAdd from './SubtaskAdd.svelte'
  import WorkchainSection from './WorkchainSection.svelte'
  import { useBoardWorkchains } from '@/lib/workchain-client'
  import type { Workchain, WorkchainStep } from '@/lib/workchain-rules'
  import TicketMuseBar from './TicketMuseBar.svelte'
  import TicketWorkTab from './TicketWorkTab.svelte'
  import WorkbenchJobsStrip from './WorkbenchJobsStrip.svelte'
  import WorkbenchTicker from './WorkbenchTicker.svelte'
  import RunDetailModal from './RunDetailModal.svelte'
  import { useWorkSession } from '@/lib/work-session.svelte'

  const MOVE: TaskStatus[] = [...TASK_STATUSES, ...OFF_BOARD_STATUSES]

  // Linear/Plane-style ticket: content (left) + properties (right).
  //
  // ── THE LAYOUT CONTRACT, and the three things it fixes ────────────────────
  //
  // 1. ONE SCROLL PER REGION, AND THE PANE GETS THE SLACK. The content column
  //    is header → title → attention zone → description → tabs → pane. Every
  //    band but the pane is `shrink-0`; the pane is `min-h-0 flex-1`, so it
  //    absorbs whatever is left instead of being the thing that collapses.
  //
  // 2. RECORDS LIVE IN TABS, NOT ABOVE THE DESCRIPTION. What sits above the
  //    description is only what wants a decision NOW — the review gate, a plan
  //    awaiting approval, the live session — and that is bounded. The work
  //    LOG (every run this ticket ever saw) moved to the Work tab. It used to
  //    share a 144px cap with the gates, so starting a run squeezed the
  //    description and the discussion; on a short window the discussion lost,
  //    because the description carried a minimum height and would not yield.
  //
  // 3. THE PROPERTIES ARE ONE MARKUP, RENDERED TWO WAYS. `propertiesRail` is a
  //    snippet: at `lg` and up it is the side column, below `lg` it is a
  //    slide-over drawer off the header's toggle, because a 280px `shrink-0`
  //    column on a narrow window ate the content it was describing. One
  //    snippet, so the two can never drift.
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
  type TicketTab = 'discussion' | 'work' | 'attachments' | 'activity' | 'workchain' | 'result'
  let tab = $state<TicketTab>('discussion')
  let descEditor = $state<RichEditorHandle | null>(null)
  // DescriptionSection's read/edit mode is OURS now (it binds up): the muse
  // bar below gates on "the description is being edited".
  let descMode = $state<'read' | 'edit'>('read')
  // The properties drawer, below `lg` only. Closed on open, and closed again
  // whenever the ticket under the modal changes (the sibling-jump effect
  // below), so a jump never lands with someone else's drawer already up.
  let propsOpen = $state(false)
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
      // ticket's room is not the new one's, and its drawer is not the new
      // one's either.
      roomId = null
      roomTried = false
      propsOpen = false
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

  // ── The review gate ───────────────────────────────────────────────────────
  // `inReview` asks the BOARD, through the one predicate that mirrors the
  // server's (isReviewStatus). Spelling it `status === 'quality_review'` — as
  // this did — meant a board that renamed its review column had a review stage
  // the server would accept a sign-off for and no gate to offer it.
  //
  // `approved` is the human sign-off, and it is a MARK rather than a move:
  // approving no longer drags the ticket into done, because a reviewed ticket
  // usually still has a merge or a release in front of it. So the gate has
  // three faces — awaiting sign-off, signed off (and parked wherever the board
  // models that), or absent.
  const inReview = $derived(!!t && isReviewStatus(t.status, boardStatuses))
  const approved = $derived(!!t?.approvedAt)
  const statusColor = $derived(t ? statusColorOf(t.status, boardStatuses) : 'var(--theme-muted)')

  // Sub-tasks: one level deep. Children list + inline add on a
  // parent; a child shows its parent with a promote control.
  const parentTask = $derived(t?.parentId ? boardTasks.find((bt) => bt.id === t.parentId) : undefined)
  const subTasks = $derived(t ? boardTasks.filter((bt) => bt.parentId === t.id) : [])
  const hasResult = $derived(!!(t && (t.outcome || t.resolution || t.errorMessage)))
  // Discussion first — the thing a reader comes here for — then the record of
  // the work, then the files. Chain and Result appear only when the ticket has
  // them, so an empty tab is not another thing to miss. Work is always offered:
  // "has this been worked?" is a question the tab answers with a zero state,
  // and a tab that appears only sometimes is a tab people stop looking for.
  const tabItems = $derived.by((): TabItem<TicketTab>[] => {
    const items: TabItem<TicketTab>[] = [
      { id: 'discussion', label: `Discussion (${t?.commentCount ?? 0})` },
      { id: 'work', label: 'Work' },
      { id: 'attachments', label: `Files (${t?.attachments.length ?? 0})` },
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
  const watchLive = () => {
    if (!live) return
    watchFocus = 'agent'
    watchRun = live.runId
  }
  const viewRun = (runId: string) => {
    watchFocus = 'agent'
    watchRun = runId
  }
</script>

<!-- Escape closes the PROPERTIES DRAWER first, and only closes the ticket once
     the drawer is shut. Modal listens for Escape on the document too, so
     without this a reader who opened the drawer on a narrow window and pressed
     Escape lost the whole ticket instead of the panel they had just opened.
     Capture phase on the same node runs before Modal's bubble-phase listener,
     and stopping propagation there is what keeps the outer close from firing. -->
<svelte:document
  onkeydowncapture={(e) => {
    if (e.key === 'Escape' && propsOpen) {
      e.stopPropagation()
      propsOpen = false
    }
  }}
/>

<!-- The one Modal primitive (fixed height + unpadded): the ticket detail is
     content INSIDE it, not its own hand-rolled shell. Entrance/exit, backdrop,
     Escape, and portal all come from Modal.

     WIDTH IS THE APP'S OWN PAGE MEASURE (`--page-width`, 1152px). It was
     `max-w-4xl` (896px), which made the ticket NARROWER than any page surface
     in the app while carrying a side rail as well — ~656px of content for a
     title, a description, a discussion and five tabs. The token is the same
     one PageSurface centres on, so the ticket is as wide as a page and not one
     pixel of new width scale (UI-CONVENTIONS "One width scale, four names"). -->
<Modal open onClose={onClose} width="max-w-[var(--page-width)]" height="h-[88vh]" padded={false}>
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
      <!-- The skeleton is the shape of what replaces it, side rail included, so
           the swap is not itself a layout jump. -->
      <div class="flex h-full w-full">
        <div class="flex min-w-0 flex-1 flex-col gap-4 px-5 py-4">
          <Skeleton class="h-7 w-2/3 rounded-md" />
          <Skeleton class="h-[clamp(6rem,16vh,12rem)] w-full rounded-lg" />
          <SkeletonRows rows={4} />
        </div>
        <div class="hidden w-[17.5rem] shrink-0 space-y-3 border-l border-line-subtle p-4 lg:block">
          <SkeletonRows rows={6} />
        </div>
      </div>
    {:else}
      <!-- Content. LOCAL fades on purpose: on a cold load this branch toggles
           in after mount (skeleton → content) and the reveal plays; when the
           hover-prefetch already warmed the cache the branch renders AT mount,
           local intros stay quiet, and the modal entrance is the only motion. -->
      <div in:fade={{ duration: 250 }} class="flex min-w-0 flex-1 flex-col">
        <!-- HEADER. Identity on the left, actions on the right, and the close
             button LIVES HERE — it used to be the first child of the rail's
             own scroll box, so scrolling the properties carried the way out of
             the dialog off the top of the screen. The status is a pill up here
             too, not buried two thirds down the rail: it is the first thing
             anyone wants from a ticket and the thing they most often change. -->
        <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
          {#if t.ticketRef}
            <span class="shrink-0 font-mono text-xs tracking-[0.05em] text-muted">{t.ticketRef}</span>
          {/if}
          <span class="flex shrink-0 items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
            <StatusDot color={statusColor} />
            {statusLabelOf(t.status, boardStatuses)}
          </span>
          {#if approved}
            <!-- The sign-off, stated where the status is stated, because it
                 qualifies the status: "in review, and already approved" is a
                 different ticket from "in review". -->
            <Chip tone="success" title={`Approved ${relativeTime(t.approvedAt!)}`} class="inline-flex items-center gap-1">
              <Check size={10} />Approved
            </Chip>
          {/if}
          {#if t.archivedAt}<Chip title="This ticket is archived">Archived</Chip>{/if}
          <span class="hidden min-w-0 truncate font-mono text-[11px] text-muted sm:block">
            opened by {t.createdBy}
          </span>

          <div class="ml-auto flex shrink-0 items-center gap-0.5">
            {#if live || queuedWait}
              <!-- The same watch affordance the board surfaces carry, opening
                   the run-detail modal on the LIVE run. Like the card
                   corner's, it waits disabled while work is only queued. -->
              <IconButton size="sm" title="Watch the work" disabled={!live} onclick={watchLive} class="text-accent">
                <Eye size={13} />
              </IconButton>
            {/if}
            <CopyLinkButton
              path={`/boards/${board.id}/${taskId}`}
              label="Copy link"
              title="Copy link to this ticket"
              class="px-1.5 py-0.5 text-xs"
            />
            <!-- Below `lg` the properties are a drawer, and this is its door.
                 Above `lg` they are always on screen, so the door would be a
                 control that does nothing. -->
            <IconButton
              size="sm"
              title="Ticket properties"
              class="lg:hidden"
              active={propsOpen}
              onclick={() => (propsOpen = !propsOpen)}
            >
              <SlidersHorizontal size={14} />
            </IconButton>
            <CloseButton onClick={onClose} />
          </div>
        </div>

        <!-- Title, then anything that WANTS A DECISION, then the description as
             a fixed anchor. Nothing stacks under the description — that zone is
             the tab pane. -->
        <div class="shrink-0 px-5 pt-4">
          <Input
            bind:value={title}
            disabled={!canEdit}
            onblur={() => title.trim() && title !== t.title && void save({ title: title.trim() })}
            onkeydown={inlineEditKeys(() => (title = t!.title))}
            class="border-0 bg-transparent px-0 font-sans text-lg font-semibold focus:border-0"
          />

          <!-- THE ATTENTION ZONE: the judge verdict on a ticket awaiting
               sign-off, the human gate, a workbench plan waiting on a person,
               and the live session. The work LOG is not here — it is the Work
               tab — so in practice this is empty or one row.

               It still carries a ceiling, because `WorkbenchJobsStrip` lists
               every live job and a ticket with several would otherwise push the
               description and the tab pane down, which is the squeeze this
               layout exists to stop, just wearing different clothes. The cap is
               deliberately generous (unlike the 144px one it replaces, which
               was ALWAYS spent because the run log lived in it): two gates fit
               without scrolling, and only the rare pile-up scrolls. -->
          <div class="mt-3 max-h-[min(38vh,17rem)] space-y-2 overflow-y-auto empty:hidden">
            <!-- QA judge verdict (advisory) — the most recent, while the ticket
                 is still awaiting sign-off. Once signed off it is history, and
                 history is in the Work tab. -->
            {#if inReview && !approved && data?.judgeReviews?.[0]}
              <JudgeVerdict review={data.judgeReviews[0]} />
            {/if}

            {#if inReview && canEdit}
              {#if approved}
                <!-- SIGNED OFF, NOT SHIPPED. Approval marks the ticket and
                     leaves it in its column; this says so plainly and points
                     at the move that is genuinely a person's call, rather than
                     making that move for them and calling the work done. -->
                <div
                  transition:slide={{ duration: 150 }}
                  class="flex flex-wrap items-center gap-2 rounded-lg border border-[color:var(--theme-success)]/40 bg-[color:var(--theme-success)]/5 p-2 font-sans text-sm"
                >
                  <Check size={14} class="shrink-0 text-[color:var(--theme-success)]" />
                  <span class="min-w-0 flex-1 text-fg">
                    Approved {relativeTime(t.approvedAt!)}. Move it on when the work has actually landed.
                  </span>
                  <Button
                    variant="outline"
                    size="sm"
                    onclick={async () => {
                      await reviewTask(taskId, 'rejected')
                      refresh()
                    }}>Reopen</Button
                  >
                </div>
              {:else}
                <div
                  transition:slide={{ duration: 150 }}
                  class="flex flex-wrap items-center gap-2 rounded-lg border border-[color:var(--theme-accent-border)] bg-accent-soft p-2 font-sans text-sm"
                >
                  <span class="min-w-0 flex-1 text-fg">Ready for review. Approving marks it signed off.</span>
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
            {/if}

            <WorkbenchJobsStrip {taskId} {canEdit} />
            <WorkbenchTicker {taskId} />
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
          <div class="shrink-0 px-5 pt-3">
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
          {:else if tab === 'work'}
            <TicketWorkTab {taskId} judgeReviews={data?.judgeReviews ?? []} onViewRun={viewRun} />
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

      <!-- ── PROPERTIES ────────────────────────────────────────────────────
           ONE markup, two homes. The rail is grouped rather than a flat run of
           fifteen labelled fields: identity at the top with no heading (you are
           looking at it), then Planning, Relationships, Classification,
           Telemetry, People, and the timestamps — each behind a hairline, so a
           field has a neighbourhood instead of a position in a list. -->
      {#snippet propertiesRail()}
        <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
          <PropGroup>
            <Prop label="Status">
              <Select
                value={t!.status}
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
                value={t!.priority}
                disabled={!canEdit}
                onchange={(e) => void save({ priority: e.currentTarget.value as Priority })}
                size="sm"
                class="w-full"
              >
                {#each PRIORITIES as p (p)}<option value={p}>{PRIORITY_ICON[p]} {p}</option>{/each}
              </Select>
            </Prop>
            <Prop label="Assignees">
              <Combobox
                options={assigneeOptions}
                selected={t!.assignees}
                onChange={(arr) => canEdit && void save({ assignees: arr })}
                disabled={!canEdit}
                multiple
                size="sm"
                placeholder="Unassigned"
              />
              {#if membersList.notice}<QueryError {...membersList.notice} />{/if}
            </Prop>
          </PropGroup>

          <PropGroup label="Planning">
            <div class="grid grid-cols-2 gap-2">
              <Prop label="Effort">
                <Select
                  value={t!.effort ?? ''}
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
                {#key `est-${t!.id}-${t!.estimatedHours ?? ''}`}
                  <Input
                    type="number"
                    min={0}
                    max={999}
                    step={0.5}
                    size="sm"
                    disabled={!canEdit}
                    value={t!.estimatedHours ?? ''}
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
              <!-- Dates go through the shared local-day helpers: a picked
                   date is an instant at 09:00/17:00 LOCAL, same as the due
                   pill, the quick-picks and the Gantt. Writing
                   `new Date(value)` here stored UTC midnight instead, so the
                   same field meant a different instant depending on which
                   surface you edited it from. -->
              <Prop label="Start date">
                <Input
                  type="date"
                  value={dateInputValue(t!.startDate)}
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
                  value={dateInputValue(t!.dueDate)}
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
            <Prop label="Time spent">
              <div class="flex h-9 items-center text-sm text-fg">{fmtDuration(t!.timeSpentSeconds)}</div>
            </Prop>
          </PropGroup>

          <PropGroup label="Relationships">
            <!-- Sub-tasks: one level deep. Children list + inline add on a
                 parent; a child shows its parent with a promote control. -->
            {#if t!.parentId}
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
          </PropGroup>

          <PropGroup label="Classification">
            <Prop label="Labels">
              <LabelPicker
                value={t!.tags}
                options={boardLabels.map((l) => l.name)}
                onChange={(next) => void save({ tags: next })}
                disabled={!canEdit}
                size="sm"
              />
              {#if labelsList.notice}<QueryError {...labelsList.notice} />{/if}
            </Prop>
            <Prop label="Color">
              <ColorPill value={t!.color} onChange={(c) => void save({ color: c as Parameters<typeof save>[0]['color'] })} disabled={!canEdit} />
            </Prop>
          </PropGroup>

          <!-- Agent-reported token spend (MCP log_usage) — priced like the ledger. -->
          {#if data!.usage.promptTokens + data!.usage.completionTokens > 0}
            <PropGroup label="Spend">
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
            </PropGroup>
          {/if}

          <PropGroup label="People">
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
          </PropGroup>

          <div class="space-y-1 border-t border-line-subtle pt-3 font-mono text-[10px] tracking-[0.05em] text-muted">
            <div>Created {relativeTime(t!.createdAt)}</div>
            <div>Updated {relativeTime(t!.updatedAt)}</div>
            {#if t!.approvedAt}<div>Approved {relativeTime(t!.approvedAt)}</div>{/if}
            {#if t!.completedAt}<div>Completed {relativeTime(t!.completedAt)}</div>{/if}
          </div>
        </div>

        {#if canEdit}
          <div class="flex shrink-0 items-center gap-2 border-t border-line-subtle p-3">
            <!-- Secondary: raised tile + hairline + readout mono (spec §8). -->
            <Button variant="outline" size="xs" class="flex-1 gap-1.5 py-1.5" onclick={async () => {
                await archiveTask(taskId, !t!.archivedAt)
                refresh()
                onClose()
              }}>
              {#if t!.archivedAt}<ArchiveRestore size={14} />{:else}<Archive size={14} />{/if}
              {t!.archivedAt ? 'Restore' : 'Archive'}
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
      {/snippet}

      <!-- Wide: the side column. 280px, up from 240px, because the fields it
           holds (a ticket combobox, a date pair) were cramped at 240. -->
      <aside
        in:fade={{ duration: 250, delay: 60 }}
        class="hidden w-[17.5rem] shrink-0 flex-col border-l border-line-subtle bg-sidebar lg:flex"
      >
        {@render propertiesRail()}
      </aside>

      <!-- Narrow: the same snippet as a slide-over. The modal panel is
           `relative`, so inset-0 covers it edge to edge. `lg:hidden` on the
           overlay means a window widened while it is open simply reveals the
           side column instead — no resize listener, no state to reconcile. -->
      {#if propsOpen}
        <div class="absolute inset-0 z-20 flex lg:hidden">
          <button
            type="button"
            aria-label="Close properties"
            class="flex-1 bg-black/40"
            onclick={() => (propsOpen = false)}
          ></button>
          <aside
            transition:fly={{ x: '100%', duration: 200 }}
            class="flex w-[17.5rem] max-w-[85%] shrink-0 flex-col border-l border-line bg-sidebar shadow-[var(--theme-shadow-3)]"
          >
            <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
              <span class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Properties</span>
              <CloseButton onClick={() => (propsOpen = false)} class="ml-auto" />
            </div>
            {@render propertiesRail()}
          </aside>
        </div>
      {/if}
    {/if}
  </div>

<!-- The one shared run-detail modal: the header eye (the live run) and the
     work log's View-log buttons (any retained run) both land here. Modal
     portals itself, so nesting inside the ticket modal's tree is safe. -->
{#if watchRun}
  <RunDetailModal open={!!watchRun} onClose={() => (watchRun = null)} runId={watchRun} {taskId} focus={watchFocus} />
{/if}
</Modal>
