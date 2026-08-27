<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronUp, ChevronDown } from '@lucide/svelte'
  import { useAgents } from '@/lib/agents'
  import { archiveTask, prefetchTask, updateTask, useBoardLabels, type BoardMember } from '@/lib/boards.svelte'
  import { assigneeInfo, userAssignee } from '@/lib/assignees'
  import { ticketMenuEntries } from '@/components/board/ticket-menu'
  import AssigneesPill from './AssigneesPill.svelte'
  import DuePill from './DuePill.svelte'
  import EstimatePill from './EstimatePill.svelte'
  import LabelsPill from './LabelsPill.svelte'
  import PriorityPill from './PriorityPill.svelte'
  import StatusPill from './StatusPill.svelte'
  import ColumnsMenu from './ColumnsMenu.svelte'
  import { LABEL_CSS, dotIcon } from './field-pills'
  import { statusColorOf, statusLabelOf, useBoardStatuses } from '@/lib/statuses'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import { useSession } from '@/lib/session'
  import CopyLinkButton from '@/components/ui/CopyLinkButton.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { cn } from '@/lib/cn'
  import { warmRoute } from '@/lib/warm-route'
  import { fade, listStagger, QUICK } from '@/lib/motion'
  import { EFFORT_LABEL, OFF_BOARD_STATUSES, PRIORITIES, PRIORITY_COLOR, TASK_STATUSES, pgNum, pgNumOr, taskTimeSpent, type Priority, type Task, type TaskStatus } from '@/lib/task-const'
  import { relativeTime } from '@/lib/fleet'
  import {
    ALL_KEYS,
    DEFAULT_COLUMNS,
    EFFORT_RANK,
    LIST_COLUMNS,
    PRIORITY_RANK,
    fmtTime,
    loadColumns,
    loadOrder,
    loadSort,
    orderKey,
    sortKey,
    storeKey,
    type ColumnKey,
    type GroupByKey,
    type RowGroup,
    type SortState,
  } from './board-list'

  // List view of a board's tasks: grouped (ClickUp-style collapsible sections),
  // configurable columns (persisted per board), inline property pills, and
  // bulk selection with a floating action bar.
  let {
    tasks,
    onOpen,
    boardId,
    members = [],
    canEdit = false,
    groupBy = 'status',
    showEmptyGroups = false,
  }: {
    tasks: Task[]
    onOpen: (id: string) => void
    boardId: string
    members?: BoardMember[]
    canEdit?: boolean
    groupBy?: GroupByKey
    /** Status/priority grouping: render EVERY group (drop lanes included). */
    showEmptyGroups?: boolean
  } = $props()

  const qc = useQueryClient()
  const menu = useContextMenu()
  const fleetQuery = useAgents()
  const sessionQuery = useSession()
  const me = $derived(sessionQuery.data)
  // The kanban's trap in list clothing: `= []` sends grouping and the status
  // picker down the `TASK_STATUSES` fallback path, so a failed read groups the
  // board under a workflow it may not use and offers statuses it may not have.
  const labelsQuery = useBoardLabels(() => boardId)
  const statusesQuery = useBoardStatuses(() => boardId)
  const boardLabels = $derived(labelsQuery.data ?? [])
  const boardStatuses = $derived(statusesQuery.data ?? [])
  // Same test, same word, same consequence as Kanban.svelte: errored AND
  // nothing cached to fall back on. A stale set still describes a real
  // workflow; an absent one does not, and `?? []` is indistinguishable from
  // "this board has no custom statuses", which is the branch that invents
  // TASK_STATUSES.
  const statusesFailed = $derived(statusesQuery.isError && statusesQuery.data === undefined)

  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', boardId] })
  const patch = async (taskId: string, p: Parameters<typeof updateTask>[1]) => {
    await updateTask(taskId, p)
    invalidate()
  }

  // Right-click a row — the SAME quick-control menu the kanban cards serve.
  const rowMenu = (e: MouseEvent, t: Task) =>
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
  const label = (id: string) => assigneeInfo(id, fleetQuery.data?.agents ?? [], members).label
  const assigneeText = (ids: string[]) => (ids.length ? ids.map(label).join(', ') : '—')

  // Init to defaults (SSR-safe), then hydrate from localStorage on the client.
  let visible = $state<ColumnKey[]>(DEFAULT_COLUMNS)
  let order = $state<ColumnKey[]>(ALL_KEYS)
  let sort = $state<SortState>({ key: null, dir: 'asc' })
  $effect(() => {
    visible = loadColumns(boardId)
    order = loadOrder(boardId)
    sort = loadSort(boardId)
  })
  const updateCols = (next: ColumnKey[]) => {
    visible = next
    try {
      localStorage.setItem(storeKey(boardId), JSON.stringify(next))
    } catch {
      /* ignore */
    }
  }
  const updateOrder = (next: ColumnKey[]) => {
    order = next
    try {
      localStorage.setItem(orderKey(boardId), JSON.stringify(next))
    } catch {
      /* ignore */
    }
  }
  // Click a header to sort: unsorted → asc → desc → unsorted.
  const onSort = (key: ColumnKey) => {
    const next: SortState =
      sort.key !== key ? { key, dir: 'asc' } : sort.dir === 'asc' ? { key, dir: 'desc' } : { key: null, dir: 'asc' }
    sort = next
    try {
      localStorage.setItem(sortKey(boardId), JSON.stringify(next))
    } catch {
      /* ignore */
    }
  }

  const byKey = (k: ColumnKey) => LIST_COLUMNS.find((c) => c.key === k)!
  const cols = $derived(order.map(byKey).filter((c) => c.fixed || visible.includes(c.key)))

  const sortVal = (t: Task, key: ColumnKey): number | string => {
    switch (key) {
      case 'ticket':
        return Number(t.ticketRef?.match(/(\d+)\s*$/)?.[1] ?? 0)
      case 'title':
        return t.title.toLowerCase()
      case 'status': {
        const keys = boardStatuses.length ? boardStatuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])
        return keys.indexOf(t.status)
      }
      case 'priority':
        return PRIORITY_RANK[t.priority] ?? 0
      case 'effort':
        return t.effort ? EFFORT_RANK[t.effort] ?? 0 : -1
      case 'estimate':
        return pgNum(t.estimatedHours) ?? -1 // wire numeric: sorting strings put "10" before "9"
      case 'assignees':
        return assigneeText(t.assignees).toLowerCase()
      case 'due':
        return t.dueDate ? Date.parse(t.dueDate) : Infinity // undated sort last (asc)
      case 'time':
        return taskTimeSpent(t) // wire bigint: comparing the raw strings put "100" before "99"
      case 'labels':
        return t.tags.join(',').toLowerCase()
      case 'updated':
        return Date.parse(t.updatedAt)
      case 'created':
        return Date.parse(t.createdAt)
      default:
        return 0
    }
  }

  const sorted = $derived.by(() => {
    if (!sort.key) return tasks
    const dir = sort.dir === 'asc' ? 1 : -1
    return [...tasks].sort((a, b) => {
      const av = sortVal(a, sort.key!)
      const bv = sortVal(b, sort.key!)
      return av < bv ? -dir : av > bv ? dir : 0
    })
  })

  // ── Grouping (ClickUp-style sections; a multi-assignee/multi-label ticket
  //    appears in each of its groups) ──────────────────────────────────────
  const groups = $derived.by((): RowGroup[] => {
    const sum = (ts: Task[]) => Math.round(ts.reduce((s, t) => s + pgNumOr(t.estimatedHours, 0), 0) * 10) / 10
    if (groupBy === 'none') return [{ key: 'all', label: '', hours: 0, tasks: sorted }]
    if (groupBy === 'status') {
      const boardKeys = boardStatuses.length ? boardStatuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])
      const order = [...boardKeys, ...OFF_BOARD_STATUSES]
      return order
        .map((st) => ({
          key: st,
          label: statusLabelOf(st, boardStatuses),
          dot: statusColorOf(st, boardStatuses),
          tasks: sorted.filter((t) => t.status === st),
        }))
        // Board statuses always show when empty-groups is on (drop lanes);
        // failed/cancelled only when occupied.
        .filter((g) => g.tasks.length > 0 || (showEmptyGroups && boardKeys.includes(g.key)))
        .map((g) => ({ ...g, hours: sum(g.tasks) }))
    }
    if (groupBy === 'priority') {
      return [...PRIORITIES]
        .reverse()
        .map((p) => ({ key: p, label: p, dot: PRIORITY_COLOR[p], tasks: sorted.filter((t) => t.priority === p) }))
        .filter((g) => g.tasks.length > 0 || showEmptyGroups)
        .map((g) => ({ ...g, hours: sum(g.tasks) }))
    }
    if (groupBy === 'assignee') {
      const byKey = new Map<string, Task[]>()
      for (const t of sorted) {
        const keys = t.assignees.length ? t.assignees : ['__none']
        for (const k of keys) byKey.set(k, [...(byKey.get(k) ?? []), t])
      }
      return [...byKey.entries()]
        .map(([k, ts]) => ({
          key: k,
          label: k === '__none' ? 'Unassigned' : assigneeInfo(k, fleetQuery.data?.agents ?? [], members).label,
          tasks: ts,
        }))
        .sort((a, b) => (a.key === '__none' ? 1 : b.key === '__none' ? -1 : a.label.localeCompare(b.label)))
        .map((g) => ({ ...g, hours: sum(g.tasks) }))
    }
    // label
    const byTag = new Map<string, Task[]>()
    for (const t of sorted) {
      const keys = t.tags.length ? t.tags : ['__none']
      for (const k of keys) byTag.set(k, [...(byTag.get(k) ?? []), t])
    }
    return [...byTag.entries()]
      .map(([k, ts]) => ({
        key: k,
        label: k === '__none' ? 'No label' : k,
        dot: k === '__none' ? undefined : LABEL_CSS[boardLabels.find((l) => l.name === k)?.color ?? 'slate'],
        tasks: ts,
      }))
      .sort((a, b) => (a.key === '__none' ? 1 : b.key === '__none' ? -1 : a.label.localeCompare(b.label)))
      .map((g) => ({ ...g, hours: sum(g.tasks) }))
  })

  let collapsed = $state<Set<string>>(new Set())
  const toggleGroup = (k: string) => {
    const next = new Set(collapsed)
    if (next.has(k)) next.delete(k)
    else next.add(k)
    collapsed = next
  }

  // ── Drag rows between groups (status/priority groups are droppable) ─────
  let dragRow = $state<string | null>(null)
  let dragOverGroup = $state<string | null>(null)
  const groupDroppable = $derived(canEdit && (groupBy === 'status' || groupBy === 'priority'))
  const dropOnGroup = (g: RowGroup) => {
    const id = dragRow
    dragRow = null
    dragOverGroup = null
    if (!id || !groupDroppable) return
    if (groupBy === 'status') void patch(id, { status: g.key as TaskStatus })
    else void patch(id, { priority: g.key as Priority })
  }

  // ── Bulk selection ───────────────────────────────────────────────────────
  let sel = $state<Set<string>>(new Set())
  $effect(() => {
    void boardId
    void groupBy
    sel = new Set()
  })
  const toggleSel = (id: string) => {
    const next = new Set(sel)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    sel = next
  }
  const bulk = async (action: { status?: TaskStatus; priority?: Priority; assignToMe?: boolean; archive?: boolean }) => {
    for (const id of sel) {
      if (action.archive) {
        await archiveTask(id, true)
      } else if (action.assignToMe && me) {
        const t = tasks.find((x) => x.id === id)
        const meKey = userAssignee(me.id)
        if (t && !t.assignees.includes(meKey)) await updateTask(id, { assignees: [...t.assignees, meKey] })
      } else {
        await updateTask(id, action)
      }
    }
    sel = new Set()
    invalidate()
  }

  // Property pills (FieldPill + DropdownMenu) — the shared clickable-inline
  // affordance; pills own their clicks so row-click still opens the ticket.
  const pctx = (t: Task) => ({
    canEdit,
    onPatch: (p: Parameters<typeof updateTask>[1]) => void patch(t.id, p),
    agents: fleetQuery.data?.agents ?? [],
    members,
    meId: me?.id,
    labels: boardLabels,
    statuses: boardStatuses,
    boardId,
  })
</script>

{#snippet cell(t: Task, key: ColumnKey)}
  {#if key === 'ticket'}
    <span class="font-mono text-xs tracking-[0.05em] text-muted">{t.ticketRef ?? ''}</span>
  {:else if key === 'title'}
    <span class="font-sans text-fg">{t.title}</span>
  {:else if key === 'status'}
    <StatusPill {t} ctx={pctx(t)} />
  {:else if key === 'priority'}
    <PriorityPill {t} ctx={pctx(t)} />
  {:else if key === 'effort'}
    <span class="font-mono text-xs tracking-[0.05em] text-muted">{t.effort ? EFFORT_LABEL[t.effort] : '—'}</span>
  {:else if key === 'estimate'}
    <EstimatePill {t} ctx={pctx(t)} />
  {:else if key === 'assignees'}
    <!-- Raw model ids would flash then swap to labels (jumping the column
        width) — hold the cell with a short bar until the fleet resolves. -->
    {#if fleetQuery.isLoading && t.assignees.length > 0}
      <Skeleton class="h-2.5 w-16 rounded-full" />
    {:else}
      <AssigneesPill {t} ctx={pctx(t)} />
    {/if}
  {:else if key === 'due'}
    <DuePill {t} ctx={pctx(t)} />
  {:else if key === 'time'}
    <!-- taskTimeSpent, not t.timeSpentSeconds: the wire value is a bigint
        string, and fmtTime's arithmetic on a string produced nonsense. -->
    <span class="font-mono text-xs tracking-[0.05em] text-muted">{fmtTime(taskTimeSpent(t))}</span>
  {:else if key === 'labels'}
    <LabelsPill {t} ctx={pctx(t)} />
  {:else if key === 'updated'}
    <span class="font-mono text-xs tracking-[0.05em] text-muted">{relativeTime(t.updatedAt)}</span>
  {:else if key === 'created'}
    <span class="font-mono text-xs tracking-[0.05em] text-muted">{relativeTime(t.createdAt)}</span>
  {/if}
{/snippet}

{#if statusesFailed}
  <!-- Nothing to fall back on: refuse to draw a made-up board. The list looked
      like the safer of the two views because the ROWS load independently — but
      "grouping falls back to defaults" is the same failure the kanban refuses
      outright, wearing a warning banner: `groupBy: 'status'` builds its lanes
      from TASK_STATUSES, and a ticket whose status is not in that invented set
      is filtered into no group and rendered nowhere. Warning the user that the
      board they are looking at is fictional, and then drawing it anyway, is not
      a weaker version of refusing — it is the bug with a label on it. (Same
      condition, same component, same copy as Kanban.svelte on purpose: a reader
      comparing the two views should see one rule, not two policies.) -->
  <div class="grid h-full place-items-center p-4">
    <QueryError
      title="Could not load this board’s columns"
      error={statusesQuery.error}
      onRetry={() => void statusesQuery.refetch()}
    />
  </div>
{:else}
  <div class="flex h-full flex-col">
    <!-- The rows below are the tickets, which loaded. Keep them and mark the
        reference read that failed — grouping headers, status colours and the
        status picker are all derived from it. Reaching here with
        `statusesQuery.isError` means a CACHED set is being shown: stale, but
        real. The no-data case returned above. -->
    {#if statusesQuery.isError || labelsQuery.isError}
      <QueryError
        variant="inline"
        class="border-b border-line-subtle px-4 py-2"
        title={statusesQuery.isError
          ? 'Statuses may be out of date'
          : labelsQuery.data === undefined
            ? 'Could not load labels, so the pills below show names without their colours'
            : 'Labels may be out of date'}
        error={statusesQuery.isError ? statusesQuery.error : labelsQuery.error}
        onRetry={() => {
          if (statusesQuery.isError) void statusesQuery.refetch()
          if (labelsQuery.isError) void labelsQuery.refetch()
        }}
      />
    {/if}
    <div class="relative min-h-0 flex-1 overflow-auto p-4">
      {#if tasks.length === 0}
        <div class="grid h-full place-items-center font-sans text-sm text-muted">No tasks match.</div>
      {:else}
        <table class="w-full text-sm">
          <thead class="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            <tr>
              <th class="w-7 py-2 pl-2">
                {#if canEdit}
                  <Checkbox
                    bare
                    title="Select all"
                    checked={sorted.length > 0 && sorted.every((t) => sel.has(t.id))}
                    onChange={(checked) => (sel = checked ? new Set(sorted.map((t) => t.id)) : new Set())}
                  />
                {/if}
              </th>
              {#each cols as c (c.key)}
                <th class={cn('px-3 py-2 font-medium', c.align === 'right' ? 'text-right' : 'text-left')}>
                  <button
                    onclick={() => onSort(c.key)}
                    class={cn(
                      'inline-flex items-center gap-1 uppercase tracking-[0.08em] transition-colors hover:text-fg',
                      sort.key === c.key ? 'text-fg' : 'text-ink-dim',
                      c.align === 'right' && 'flex-row-reverse',
                    )}
                  >
                    {c.label}
                    {#if sort.key === c.key}
                      {#if sort.dir === 'asc'}<ChevronUp size={12} />{:else}<ChevronDown size={12} />{/if}
                    {/if}
                  </button>
                </th>
              {/each}
              <th class="w-8 py-1 pr-2 text-right">
                <ColumnsMenu {visible} {order} onChangeVisible={updateCols} onChangeOrder={updateOrder} />
              </th>
            </tr>
          </thead>
          {#each groups as g (g.key)}
            {@const open = !collapsed.has(g.key)}
            <tbody
              use:listStagger
              class={cn('divide-y divide-line-subtle', dragOverGroup === g.key && dragRow && 'bg-accent/5')}
              ondragover={groupDroppable
                ? (e) => {
                    e.preventDefault()
                    dragOverGroup = g.key
                  }
                : undefined}
              ondragleave={() => {
                if (dragOverGroup === g.key) dragOverGroup = null
              }}
              ondrop={groupDroppable
                ? (e) => {
                    e.preventDefault()
                    dropOnGroup(g)
                  }
                : undefined}
            >
              {#if groupBy !== 'none'}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <tr
                  class={cn('cursor-pointer select-none bg-panel/60', dragOverGroup === g.key && dragRow && 'ring-1 ring-inset ring-accent')}
                  onclick={() => toggleGroup(g.key)}
                >
                  <td colspan={cols.length + 2} class="px-2 py-1.5">
                    <span class="flex items-center gap-2 font-mono text-[10px] tracking-[0.05em]">
                      <span class="text-muted">{open ? '▾' : '▸'}</span>
                      {#if g.dot}<span class="h-1.5 w-1.5 rounded-full" style:background={g.dot}></span>{/if}
                      <span class="font-medium uppercase tracking-[0.08em] text-ink-dim">{g.label}</span>
                      <span class="text-muted">{g.tasks.length}</span>
                      {#if g.hours > 0}<span class="text-muted">Σ {g.hours}h</span>{/if}
                    </span>
                  </td>
                </tr>
              {/if}
              {#if open && g.tasks.length === 0}
                <tr>
                  <td colspan={cols.length + 2} class={cn('px-4 py-2 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim', dragOverGroup === g.key && dragRow && 'text-accent')}>
                    {dragRow ? 'Drop here' : 'No tickets'}
                  </td>
                </tr>
              {/if}
              {#if open}
                {#each g.tasks as t (`${g.key}:${t.id}`)}
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                  <tr
                    in:fade={{ duration: 150 }}
                    out:fade={QUICK}
                    draggable={groupDroppable}
                    ondragstart={(e) => {
                      e.dataTransfer?.setData('text/task', t.id)
                      if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
                      dragRow = t.id
                    }}
                    ondragend={() => {
                      dragRow = null
                      dragOverGroup = null
                    }}
                    onclick={() => onOpen(t.id)}
                    use:warmRoute={{ path: '/boards/:boardId/:taskId', warm: () => prefetchTask(qc, t.id) }}
                    oncontextmenu={(e) => rowMenu(e, t)}
                    class={cn('group cursor-pointer transition-colors dither-fill', sel.has(t.id) && 'bg-raised/70', dragRow === t.id && 'opacity-40')}
                  >
                    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                    <td class="pl-2" onclick={(e) => e.stopPropagation()}>
                      {#if canEdit}
                        <Checkbox
                          bare
                          title="Select task"
                          checked={sel.has(t.id)}
                          onChange={() => toggleSel(t.id)}
                          class={cn('transition-opacity', sel.size === 0 && 'opacity-0 group-hover:opacity-100')}
                        />
                      {:else}
                        <CopyLinkButton path={`/boards/${t.boardId}/${t.id}`} class="opacity-0 group-hover:opacity-100" />
                      {/if}
                    </td>
                    {#each cols as c (c.key)}
                      <td class={cn('px-3 py-2', c.align === 'right' && 'text-right')}>
                        {@render cell(t, c.key)}
                      </td>
                    {/each}
                    <td></td>
                  </tr>
                {/each}
              {/if}
            </tbody>
          {/each}
        </table>
      {/if}
    </div>

    <!-- Bulk action bar — appears with a selection, acts on every selected
        ticket, then clears. -->
    {#if sel.size > 0}
      <div class="pointer-events-none absolute inset-x-0 bottom-4 z-30 flex justify-center" transition:fade={QUICK}>
        <div class="pointer-events-auto flex items-center gap-1 rounded-lg border border-line bg-panel px-2 py-1.5 shadow-[var(--theme-shadow-3)]">
          <span class="px-2 font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-fg">{sel.size} selected</span>
          <DropdownMenu
            up
            align="left"
            items={(boardStatuses.length ? boardStatuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])).map((st) => ({
              label: statusLabelOf(st, boardStatuses),
              icon: dotIcon(statusColorOf(st, boardStatuses), 'h-2 w-2 rounded-full'),
              onSelect: () => void bulk({ status: st as TaskStatus }),
            }))}
          >
            {#snippet trigger(open)}
              <FieldPill active={open}>
                Move to
              </FieldPill>
            {/snippet}
          </DropdownMenu>
          <DropdownMenu
            up
            align="left"
            items={[...PRIORITIES].reverse().map((pr) => ({
              label: pr,
              icon: dotIcon(PRIORITY_COLOR[pr], 'h-2 w-2 rounded-full'),
              onSelect: () => void bulk({ priority: pr }),
            }))}
          >
            {#snippet trigger(open)}
              <FieldPill active={open}>
                Priority
              </FieldPill>
            {/snippet}
          </DropdownMenu>
          {#if me}
            <Button variant="ghost" size="xs" class="py-1 dither-fill" onclick={() => void bulk({ assignToMe: true })}>
              Assign to me
            </Button>
          {/if}
          <Button variant="ghost" size="xs" class="py-1 dither-fill hover:text-danger" onclick={() => void bulk({ archive: true })}>
            Archive
          </Button>
          <button onclick={() => (sel = new Set())} title="Clear selection" class="rounded-md px-1.5 py-1 font-mono text-[10px] text-muted transition-colors hover:text-fg">
            ✕
          </button>
        </div>
      </div>
    {/if}
    <ContextMenu {menu} />
  </div>
{/if}
