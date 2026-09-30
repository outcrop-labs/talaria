<script lang="ts">
  // The workchains lens's pop-out: where the board's TICKETS and the board's
  // FILTERS both live while this view is up.
  //
  // Why it exists at all. The canvas is the surface; everything that is not the
  // graph was competing with it for the top of the page. The ticket picker was
  // a popover over a flat list — fine for six tickets, unusable for sixty — and
  // the board's filter row sat above the canvas filtering a list you could no
  // longer see. Both belong to the same question ("which tickets am I working
  // with?"), so they answer it in one place, and the toolbar above the canvas
  // is left to the chain.
  //
  // A row DRAGS onto the canvas (the house pattern: Gantt's unscheduled list
  // drops onto the chart, typed payload and all) and lands where it is dropped.
  // Clicking a row is the keyboard-free equivalent: it appends to the chain.
  import { ListFilter, PanelLeftClose, Search } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { statusColorOf, type BoardStatus } from '@/lib/statuses'
  import { EFFORT_LABEL, type Task } from '@/lib/task-const'
  import type { BoardLabel, BoardMember } from '@/lib/boards.svelte'
  import FilterBar from './FilterBar.svelte'
  import { filtersActive, type BoardFilters } from './filter-bar'
  import { SIDEBAR_TICKET_MIME } from '@/lib/wiring-canvas'

  let {
    unchained,
    chainedCount,
    doneHidden,
    boardStatuses,
    members,
    agents,
    labels,
    meId,
    q,
    onQ,
    filters,
    onFilters,
    showArchived,
    onArchived,
    canEdit,
    onAdd,
    onOpen,
    onClose,
  }: {
    /** The board's filtered tickets that no chain holds — the drag source. */
    unchained: Task[]
    /** How many of the filtered tickets ARE in a chain: the other half of the
     *  count, so an empty list reads as "all chained" rather than "no tickets". */
    chainedCount: number
    /** Finished tickets the panel is holding back. Never offered — you do not
     *  queue work that is over — but counted, so an empty list can say which
     *  kind of empty it is. */
    doneHidden: number
    boardStatuses: BoardStatus[]
    members: BoardMember[]
    agents: Array<{ id: string; label: string }>
    labels: BoardLabel[]
    meId?: string | null
    q: string
    onQ: (v: string) => void
    filters: BoardFilters
    onFilters: (f: BoardFilters) => void
    showArchived: boolean
    onArchived: (v: boolean) => void
    canEdit: boolean
    /** Append the ticket to the focused chain (the click path; a drag carries
     *  its own drop point and goes through the canvas instead). */
    onAdd: (taskId: string) => void
    onOpen: (taskId: string) => void
    onClose: () => void
  } = $props()

  // The filter block collapses: a reader who has set their filters wants the
  // tickets, not the controls. It springs back open whenever a filter is
  // actually on, so a narrowed list can never look like a short board.
  let filtersOpen = $state(false)
  const anyFilter = $derived(filtersActive(filters) || q.trim() !== '' || showArchived)
</script>

<aside class="flex w-72 shrink-0 flex-col border-r border-line bg-sidebar">
  <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-3">
    <span class="font-mono text-[10px] font-semibold uppercase tracking-[0.08em] text-ink-dim">Tickets</span>
    <!-- The LIST's own count, not the board's. The board's filtered read
         already sits in its header ("10 of 12"); repeating it here beside a
         list of a different length was the panel disagreeing with itself. -->
    <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{unchained.length}</span>
    <span class="ml-auto flex items-center gap-0.5">
      <IconButton
        size="sm"
        active={filtersOpen || anyFilter}
        title="Filter this board"
        onclick={() => (filtersOpen = !filtersOpen)}
      >
        <ListFilter size={14} />
      </IconButton>
      <IconButton size="sm" title="Hide the ticket panel" onclick={onClose}>
        <PanelLeftClose size={14} />
      </IconButton>
    </span>
  </div>

  <div class="shrink-0 border-b border-line-subtle px-3 py-2">
    <div class="relative">
      <Search size={13} class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-muted" />
      <Input
        value={q}
        oninput={(e) => onQ(e.currentTarget.value)}
        placeholder="Search tickets"
        size="sm"
        class="w-full pl-7"
      />
    </div>
    {#if filtersOpen || anyFilter}
      <!-- The board's own facets, unchanged — the same component the other
           lenses put in the header row, wrapping in a column instead. -->
      <div class="mt-2 flex flex-col gap-1">
        <FilterBar value={filters} onChange={onFilters} {members} {agents} {labels} statuses={boardStatuses} {meId} />
        <Chip onSelect={() => onArchived(!showArchived)} selected={showArchived} title="Include archived tickets">
          archived
        </Chip>
      </div>
    {/if}
  </div>

  <div class="min-h-0 flex-1 overflow-y-auto p-2">
    {#if unchained.length === 0}
      <!-- Which KIND of empty: everything chained, everything finished, or a
           filter holding the rest back. A bare "no tickets" over a board with
           forty of them is the panel lying. -->
      <EmptyState
        variant="compact"
        icon="⭆"
        title={chainedCount > 0 ? 'Every open ticket is chained' : doneHidden > 0 ? 'Nothing left to chain' : 'No tickets here'}
        hint={anyFilter
          ? 'The filters above are hiding the rest.'
          : doneHidden > 0
            ? `${doneHidden} finished ticket${doneHidden === 1 ? '' : 's'} stay out of the picker.`
            : 'Tickets you add to the board show up here.'}
      />
    {:else}
      <p class="px-1 pb-1.5 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">
        {canEdit ? 'drag onto the canvas, or click to append' : 'not in any workchain'}
      </p>
      {#if doneHidden > 0}
        <p class="px-1 pb-1.5 font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">
          {doneHidden} finished hidden
        </p>
      {/if}
      <div class="flex flex-col gap-0.5">
        {#each unchained as t (t.id)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            role="button"
            tabindex={0}
            draggable={canEdit}
            ondragstart={(e) => {
              e.dataTransfer?.setData(SIDEBAR_TICKET_MIME, t.id)
              if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
            }}
            onclick={() => (canEdit ? onAdd(t.id) : onOpen(t.id))}
            onkeydown={(e) => {
              if (e.key === 'Enter') onAdd(t.id)
            }}
            oncontextmenu={(e) => {
              e.preventDefault()
              onOpen(t.id)
            }}
            title={canEdit ? 'Drag onto the canvas, or click to append to the chain' : t.title}
            class={cn(
              'group flex min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-raised',
              canEdit ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer',
            )}
          >
            <StatusDot color={statusColorOf(t.status, boardStatuses)} />
            {#if t.ticketRef}
              <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.ticketRef}</span>
            {/if}
            <span class="min-w-0 flex-1 truncate font-sans text-xs text-fg">{t.title}</span>
            {#if t.effort}
              <span class="shrink-0 rounded border border-line-subtle px-1 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">
                {EFFORT_LABEL[t.effort]}
              </span>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</aside>
