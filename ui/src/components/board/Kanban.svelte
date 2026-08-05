<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ticketMenuEntries } from '@/components/board/ticket-menu'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import KanbanAddCard from './KanbanAddCard.svelte'
  import KanbanCard from './KanbanCard.svelte'
  import { COL_ACCENT, fmtHours } from './kanban'
  import { cn } from '@/lib/cn'
  import { fade, flip, LIST, listStagger, QUICK } from '@/lib/motion'
  import { useAgents } from '@/lib/agents'
  import { archiveTask, createTask, prefetchTask, updateTask, useBoardLabels, type Board, type BoardMember } from '@/lib/boards.svelte'
  import { OFF_BOARD_STATUSES, STATUS_LABEL, TASK_STATUSES, pgNumOr, type Task, type TaskStatus } from '@/lib/task-const'
  import { statusColorOf, useBoardStatuses } from '@/lib/statuses'
  import { useSession } from '@/lib/session'

  // Kanban board — polished columns, per-column add, drag-and-drop between columns.
  // Tasks are passed in (the board page owns fetching + filtering).
  let {
    board,
    tasks,
    onOpen,
    members = [],
  }: {
    board: Board
    tasks: Task[]
    onOpen: (taskId: string) => void
    members?: BoardMember[]
  } = $props()

  const qc = useQueryClient()
  const fleetQuery = useAgents()
  const sessionQuery = useSession()
  const me = $derived(sessionQuery.data)
  // Both reads used `= []`, which is indistinguishable from "the server said
  // this board has no labels / no custom statuses". For STATUSES that is the
  // worse of the two: an empty set falls through to the default column list
  // below, so a failed read invents a workflow this board may not have — and
  // any ticket whose status isn't in the invented set renders in NO column at
  // all, i.e. work silently disappears off the board.
  const labelsQuery = useBoardLabels(() => board.id)
  const statusesQuery = useBoardStatuses(() => board.id)
  const boardLabels = $derived(labelsQuery.data ?? [])
  const boardStatuses = $derived(statusesQuery.data ?? [])
  const statusesFailed = $derived(statusesQuery.isError && statusesQuery.data === undefined)
  // Columns: the board's status set (custom or defaults), plus the legacy
  // terminal extras only when occupied.
  const columns = $derived(
    boardStatuses.length
      ? [
          ...boardStatuses.map((st) => ({ key: st.key, label: st.label, color: statusColorOf(st.key, boardStatuses) })),
          // The legacy terminal extras, from the ONE off-board list (lib/task-const).
          // This was a `['failed','cancelled']` literal — a third copy of a set
          // that decides which columns exist, sitting eight lines under a comment
          // about work vanishing when a ticket's status has no column.
          ...OFF_BOARD_STATUSES.filter((k) => tasks.some((t) => t.status === k))
            .map((k) => ({ key: k as string, label: STATUS_LABEL[k] ?? k, color: COL_ACCENT[k] ?? 'var(--theme-muted)' })),
        ]
      : TASK_STATUSES.map((k) => ({ key: k as string, label: STATUS_LABEL[k] ?? k, color: COL_ACCENT[k] ?? 'var(--theme-muted)' })),
  )
  const agents = $derived(fleetQuery.data?.agents ?? [])
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  let dragOver = $state<string | null>(null)
  let dragging = $state<string | null>(null)
  const menu = useContextMenu()

  // Sub-task rollup: children by parent id (children render as cards too).
  const childrenOf = $derived.by(() => {
    const m = new Map<string, Task[]>()
    for (const t of tasks) {
      if (t.parentId) m.set(t.parentId, [...(m.get(t.parentId) ?? []), t])
    }
    return m
  })
  const parentRef = (t: Task) => {
    if (!t.parentId) return null
    const p = tasks.find((x) => x.id === t.parentId)
    return p ? (p.ticketRef ?? p.title) : null
  }

  const patch = async (taskId: string, p: Parameters<typeof updateTask>[1]) => {
    await updateTask(taskId, p)
    invalidate()
  }

  // Right-click a card — the shared ticket menu (quick controls + shortcuts).
  const cardMenu = (e: MouseEvent, t: Task) =>
    menu.openMenu(
      e,
      ticketMenuEntries(t, {
        canEdit,
        meId: me?.id,
        statuses: boardStatuses,
        onOpen: () => onOpen(t.id),
        onPatch: (p) => void patch(t.id, p),
        onArchive: () => void archiveTask(t.id, !t.archivedAt).then(invalidate),
      }),
    )

  const addTo = async (status: TaskStatus, title: string) => {
    const { task } = (await createTask(board.id, { title })) as { task: Task }
    if (task.status !== status) await updateTask(task.id, { status })
    invalidate()
  }
  const move = async (taskId: string, status: TaskStatus) => {
    await updateTask(taskId, { status })
    invalidate()
  }
</script>

{#if statusesFailed}
  <!-- Nothing to fall back on: refuse to draw a made-up board. -->
  <div class="grid h-full place-items-center p-4">
    <QueryError
      title="Could not load this board’s columns"
      error={statusesQuery.error}
      onRetry={() => void statusesQuery.refetch()}
    />
  </div>
{:else}
  <div class="flex h-full flex-col">
    <!-- Stale columns/labels beat no board at all — but say which read is old.
        Labels only tint the pills, so their failure never takes the board. -->
    {#if statusesQuery.isError || labelsQuery.isError}
      <QueryError
        variant="inline"
        class="border-b border-line-subtle px-4 py-2"
        title={statusesQuery.isError
          ? 'Columns may be out of date'
          : labelsQuery.data === undefined
            ? 'Could not load labels — the pills below show names without their colours'
            : 'Labels may be out of date'}
        error={statusesQuery.isError ? statusesQuery.error : labelsQuery.error}
        onRetry={() => {
          if (statusesQuery.isError) void statusesQuery.refetch()
          if (labelsQuery.isError) void labelsQuery.refetch()
        }}
      />
    {/if}
    <div class="flex min-h-0 flex-1 gap-3 overflow-x-auto p-4">
      {#each columns as col (col.key)}
        {@const colTasks = tasks.filter((t) => t.status === col.key)}
        <!-- Column estimate rollup — visible planning weight per column.
            pgNumOr, not `?? 0`: estimatedHours arrives as a STRING (see
            PgNumeric), so `s + t.estimatedHours` concatenated — "04.5" — and
            fmtHours then called .toFixed on a string. -->
        {@const colHours = colTasks.reduce((s, t) => s + pgNumOr(t.estimatedHours, 0), 0)}
        <div
          ondragover={canEdit
            ? (e) => {
                e.preventDefault()
                dragOver = col.key
              }
            : undefined}
          ondragleave={() => {
            if (dragOver === col.key) dragOver = null
          }}
          ondrop={canEdit
            ? (e) => {
                e.preventDefault()
                const id = e.dataTransfer?.getData('text/task')
                dragOver = null
                if (id) void move(id, col.key as TaskStatus)
              }
            : undefined}
          class={cn(
            'flex w-80 shrink-0 flex-col rounded-lg bg-panel/60 ring-1 ring-transparent transition-shadow',
            dragOver === col.key && 'ring-accent',
          )}
        >
          <div class="flex items-center gap-2 px-3 py-2">
            <span class="h-1.5 w-1.5 rounded-full" style:background={col.color}></span>
            <span class="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-ink-dim">{col.label}</span>
            <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{colTasks.length}</span>
            {#if colHours > 0}
              <span class="ml-auto font-mono text-[10px] tracking-[0.05em] text-muted" title="Total estimated hours in this column">
                Σ {fmtHours(colHours)}
              </span>
            {/if}
          </div>
          <div class="flex-1 space-y-2 overflow-y-auto px-2 pb-2" use:listStagger>
            {#each colTasks as t (t.id)}
              <!-- flip makes reorders and drops glide; cheap even in big
                   columns — it only transforms cards whose position changed,
                   as compositor transforms. A cross-column move fades out of
                   one column and into the other (flip can't span lists). -->
              <!-- Hover warms the ticket cache so the detail modal opens with
                   content already in place instead of skeleton-then-pop. -->
              <div
                animate:flip={LIST}
                in:fade={{ duration: 150 }}
                out:fade={QUICK}
                onpointerenter={() => prefetchTask(qc, t.id)}
              >
                <KanbanCard
                  task={t}
                  pillCtx={{ canEdit, onPatch: (p) => void patch(t.id, p), agents, members, meId: me?.id, labels: boardLabels, statuses: boardStatuses, boardId: board.id }}
                  subtasks={childrenOf.get(t.id) ?? []}
                  parentRef={parentRef(t)}
                  draggable={canEdit}
                  dim={dragging === t.id}
                  onDragStart={(e) => {
                    e.dataTransfer?.setData('text/task', t.id)
                    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
                    dragging = t.id
                  }}
                  onDragEnd={() => (dragging = null)}
                  onOpen={() => onOpen(t.id)}
                  onContextMenu={(e) => cardMenu(e, t)}
                />
              </div>
            {/each}
            {#if canEdit}
              <KanbanAddCard onAdd={(title) => addTo(col.key as TaskStatus, title)} />
            {/if}
          </div>
        </div>
      {/each}
      <ContextMenu {menu} />
    </div>
  </div>
{/if}
