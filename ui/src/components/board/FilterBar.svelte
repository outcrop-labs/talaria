<script lang="ts">
  // The board filter bar — ClickUp-grade facets, Mercury-clean. Each facet is a
  // FieldPill opening a keep-open multi-select; the pill itself reads as the
  // active filter ("Status · 2"). Values are ORed within a facet, ANDed across
  // facets; everything lives in the URL (the route owns encoding).
  import { CalendarDays, Flag, Tag, UserRound, X } from '@lucide/svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { ContextMenuEntry, MenuIcon } from '@/components/ui/context-menu.svelte'
  import { userAssignee } from '@/lib/assignees'
  import type { BoardLabel, BoardMember } from '@/lib/boards.svelte'
  import { OFF_BOARD_STATUSES, PRIORITIES, PRIORITY_COLOR, TASK_STATUSES, type Priority } from '@/lib/task-const'
  import { LABEL_CSS, STATUS_COLOR, dotIcon } from './field-pills'
  import { statusColorOf, statusLabelOf, type BoardStatus } from '@/lib/statuses'
  import FacetPill from './FacetPill.svelte'
  import { DUE_LABEL, EMPTY_FILTERS, filtersActive, type BoardFilters } from './filter-bar'
  import { fade, QUICK } from '@/lib/motion'

  let {
    value,
    onChange,
    members,
    agents,
    labels,
    statuses,
    meId,
  }: {
    value: BoardFilters
    onChange: (next: BoardFilters) => void
    members: BoardMember[]
    agents: Array<{ id: string; label: string }>
    labels: BoardLabel[]
    statuses?: BoardStatus[]
    meId?: string | null
  } = $props()

  const toggle = (facet: 'statuses' | 'assignees' | 'priorities' | 'labels', v: string) => {
    const cur = value[facet]
    onChange({ ...value, [facet]: cur.includes(v) ? cur.filter((x) => x !== v) : [...cur, v] })
  }

  const flagIcon = (p: Priority): MenuIcon => [Flag, { size: 12, style: `color: ${PRIORITY_COLOR[p]}` }]

  const statusItems = $derived(
    [...(statuses?.length ? statuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])), ...OFF_BOARD_STATUSES].map((s) => ({
      label: statusLabelOf(s, statuses ?? []),
      icon: dotIcon(statuses?.length ? statusColorOf(s, statuses) : STATUS_COLOR[s] ?? 'var(--theme-muted)'),
      checked: value.statuses.includes(s),
      keepOpen: true,
      onSelect: () => toggle('statuses', s),
    })),
  )

  const assigneeItems = (): ContextMenuEntry[] => [
    ...(meId
      ? [
          {
            label: 'Me',
            checked: value.assignees.includes(userAssignee(meId)),
            keepOpen: true,
            onSelect: () => toggle('assignees', userAssignee(meId)),
          },
        ]
      : []),
    {
      label: 'Unassigned',
      checked: value.assignees.includes('__none'),
      keepOpen: true,
      onSelect: () => toggle('assignees', '__none'),
    },
    'sep' as const,
    ...members
      .filter((m) => m.userId !== meId)
      .map((m) => ({
        label: m.name ?? m.email ?? 'teammate',
        checked: value.assignees.includes(userAssignee(m.userId)),
        keepOpen: true,
        onSelect: () => toggle('assignees', userAssignee(m.userId)),
      })),
    ...(agents.length ? (['sep'] as ContextMenuEntry[]) : []),
    ...agents.map((a) => ({
      label: a.label,
      checked: value.assignees.includes(a.id),
      keepOpen: true,
      onSelect: () => toggle('assignees', a.id),
    })),
  ]

  const priorityItems = $derived(
    [...PRIORITIES].reverse().map((p) => ({
      label: p,
      icon: flagIcon(p),
      checked: value.priorities.includes(p),
      keepOpen: true,
      onSelect: () => toggle('priorities', p),
    })),
  )

  const labelItems = $derived(
    labels.map((l) => ({
      label: l.name,
      icon: dotIcon(LABEL_CSS[l.color]),
      checked: value.labels.includes(l.name),
      keepOpen: true,
      onSelect: () => toggle('labels', l.name),
    })),
  )

  const dueItems = $derived(
    (Object.keys(DUE_LABEL) as Array<Exclude<BoardFilters['due'], ''>>).map((d) => ({
      label: DUE_LABEL[d],
      checked: value.due === d,
      onSelect: () => onChange({ ...value, due: value.due === d ? '' : d }),
    })),
  )
</script>

<div class="flex flex-wrap items-center gap-1">
  <FacetPill label="Status" count={value.statuses.length} items={statusItems}>
    {#snippet icon()}<StatusDot status="accent" />{/snippet}
  </FacetPill>
  <FacetPill label="Assignee" count={value.assignees.length} items={assigneeItems}>
    {#snippet icon()}<UserRound size={12} />{/snippet}
  </FacetPill>
  <FacetPill label="Priority" count={value.priorities.length} items={priorityItems}>
    {#snippet icon()}<Flag size={12} />{/snippet}
  </FacetPill>
  {#if labels.length > 0}
    <span class="inline-flex" in:fade={{ duration: 150 }} out:fade={QUICK}>
      <FacetPill label="Label" count={value.labels.length} items={labelItems}>
        {#snippet icon()}<Tag size={12} />{/snippet}
      </FacetPill>
    </span>
  {/if}
  <FacetPill label={value.due ? DUE_LABEL[value.due] : 'Due'} count={value.due ? 1 : 0} items={dueItems}>
    {#snippet icon()}<CalendarDays size={12} />{/snippet}
  </FacetPill>
  {#if filtersActive(value)}
    <button
      in:fade={{ duration: 150 }}
      out:fade={QUICK}
      onclick={() => onChange(EMPTY_FILTERS)}
      title="Clear all filters"
      class="flex h-9 items-center gap-1 rounded-md px-2 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger"
    >
      <X size={12} /> Clear
    </button>
  {/if}
</div>
