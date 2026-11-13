<script lang="ts">
  import { tick } from 'svelte'
  import { Columns3, List } from '@lucide/svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { glyphOf, LANE_CAP, LANES, refOf, shownCards, type InboxCard, type InboxGlyph, type InboxLane, type InboxLayout } from '@/lib/inbox-board'
  import { flip, LIST, prefersReducedMotion, rowCrossfade } from '@/lib/motion'
  import BoardCard from './BoardCard.svelte'
  import InboxMore from './InboxMore.svelte'
  import { BOARD_MIN_PX, COLUMN, COLUMNS, DRAWER_PX, SECTION, SECTION_COUNT, type InboxBoardState } from './inbox-board.svelte'

  let {
    board,
    selectedKey,
    beside,
    onOpen,
    onBoard,
  }: {
    board: InboxBoardState
    /** The card the drawer shows. */
    selectedKey: string | null
    /** The side drawer is in the row beside the page, taking its width. */
    beside: boolean
    /** A card was clicked: show it in the drawer. */
    onOpen: (card: InboxCard) => void
    /** Board was chosen with the drawer where the lanes would go: close it. */
    onBoard: () => void
  } = $props()

  const VIEW_KEY = 'talaria:home-inbox-view'

  function storedView(): InboxLayout {
    try {
      return localStorage.getItem(VIEW_KEY) === 'list' ? 'list' : 'board'
    } catch {
      return 'board'
    }
  }

  let width = $state(0)
  const tight = $derived(width > 0 && width < BOARD_MIN_PX)
  const fits = $derived(width === 0 || width + (beside ? DRAWER_PX : 0) >= BOARD_MIN_PX)
  const empty = $derived(!board.pending && board.cards.length === 0)
  let view = $state<InboxLayout>(storedView())
  const layout = $derived<InboxLayout>(tight ? 'list' : view)

  function choose(next: InboxLayout): void {
    view = next
    try {
      localStorage.setItem(VIEW_KEY, next)
    } catch {
      // A per-viewer nicety: the toggle still works for this visit.
    }
    if (next === 'board' && tight) onBoard()
  }

  let showAll = $state<Partial<Record<InboxLane, boolean>>>({})
  const [send, receive] = rowCrossfade()

  const FLASH_MS = 1200

  export async function ring(glyph: InboxGlyph): Promise<void> {
    if (board.lanes.needs.slice(LANE_CAP).some((c) => glyphOf(c) === glyph)) {
      showAll.needs = true
      await tick()
    }
    const els = [...document.querySelectorAll<HTMLElement>(`#inbox-lane-needs [data-inbox-card][data-glyph="${glyph}"]`)]
    els[0]?.scrollIntoView({ behavior: prefersReducedMotion() ? 'auto' : 'smooth', block: 'center' })
    for (const el of els) el.setAttribute('data-flash', '')
    setTimeout(() => els.forEach((el) => el.removeAttribute('data-flash')), FLASH_MS)
  }

  const VIEWS = [
    { id: 'board', label: boardLabel, data: { 'data-inbox-view': 'board' } },
    { id: 'list', label: listLabel, data: { 'data-inbox-view': 'list' } },
  ] as const
</script>

{#snippet boardLabel()}<span class="inline-flex items-center gap-1.5"><Columns3 size={13} aria-hidden="true" /> Board</span>{/snippet}
{#snippet listLabel()}<span class="inline-flex items-center gap-1.5"><List size={13} aria-hidden="true" /> List</span>{/snippet}

<section aria-labelledby="inbox-board-title" hidden={empty && !board.failure} bind:clientWidth={width}>
  <h2 id="inbox-board-title" class="sr-only">Inbox board</h2>

  {#if board.failure}
    <QueryError
      variant="inline"
      class="mb-3 rounded-lg border border-[color:var(--theme-danger)]/40 bg-[color:var(--theme-danger)]/5 px-4 py-2.5"
      error={board.failure.error}
      title="Could not read your {board.failure.sources.join(', ')}, so this board may be missing items"
      onRetry={board.retry}
    />
  {/if}

  <div class="mb-3 flex min-h-[30px] items-center">
    {#if fits}
      <Segmented variant="solid" voice="sans" size="xs" class="bg-lane" options={VIEWS} value={selectedKey ? layout : view} onChange={choose} />
    {:else}
      <p class="px-2 font-sans text-[13px] leading-5 text-muted">List view · the board needs a wider window</p>
    {/if}
  </div>

  <Materialize loading={board.pending} count={1}>
    {#snippet skeleton()}
      <div aria-hidden="true" class={cn('grid gap-2.5', layout === 'board' && 'grid-cols-3 items-start')}>
        {#each LANES as lane (lane.id)}
          <div class="rounded-xl bg-lane p-1.5">
            <div class="flex h-7 items-center px-[15px]"><Skeleton class="h-2.5 w-16 rounded-full" /></div>
            <div class="grid gap-1.5">
              {#each [0, 1] as i (i)}
                <div class="rounded-lg border border-line bg-raised px-3.5 py-3">
                  <Skeleton class="h-3.5 w-3/4 rounded-full" />
                  <Skeleton class="mt-2.5 h-3 w-1/2 rounded-full" />
                </div>
              {/each}
            </div>
          </div>
        {/each}
      </div>
    {/snippet}

    <div class={cn('grid gap-2.5', layout === 'board' ? 'grid-cols-3 items-start' : 'grid-cols-1')}>
      {#each LANES as lane (lane.id)}
        {@const cards = board.lanes[lane.id]}
        {@const shown = shownCards(cards, !!showAll[lane.id])}
        {@const cols = { hasStatus: lane.hasStatusColumn, hasTypes: cards.some((c) => refOf(c).kind), hasRefs: cards.some((c) => refOf(c).ref) }}
        <section
          id="inbox-lane-{lane.id}"
          aria-labelledby="inbox-lane-title-{lane.id}"
          class={cn('@container min-w-0 rounded-xl bg-lane p-1.5', lane.id === 'needs' && cards.length > 0 && 'border-t-2 border-t-warning pt-1')}
        >
          <header class={cn('flex h-7 items-center gap-3 pl-[15px]', layout === 'list' ? 'pr-[39px]' : 'pr-[13px]')}>
            <h3 id="inbox-lane-title-{lane.id}" class={cn(SECTION, 'flex min-w-0 flex-1 items-center gap-1.5')}>
              {lane.label}
              <span
                class={lane.id === 'needs' && cards.length > 0
                  ? 'min-w-[18px] rounded-full bg-warning px-1.5 text-center text-[11.5px] font-medium leading-[18px] tabular-nums text-raised'
                  : SECTION_COUNT}
              >
                {cards.length}
              </span>
            </h3>
            {#if layout === 'list' && cards.length > 0}
              <span aria-hidden="true" class={cn('font-sans text-[12px] leading-5 text-muted', COLUMNS)}>
                {#if cols.hasStatus}<span class={COLUMN.status}>Status</span>{/if}
                {#if cols.hasTypes}<span class={COLUMN.type}>Type</span>{/if}
                <span class={COLUMN.time}>{lane.timeHeader}</span>
                {#if cols.hasRefs}<span class={COLUMN.ref}>Ref</span>{/if}
                <span class={COLUMN.owner}>Owner</span>
              </span>
            {/if}
          </header>
          <ul
            class={cn(
              layout === 'board'
                ? 'grid gap-1.5'
                : cards.length > 0 &&
                    cn('divide-y divide-line overflow-hidden rounded-lg border border-line', lane.id === 'done' ? 'bg-[var(--inbox-done-card)]' : 'bg-raised'),
            )}
          >
            {#each shown as card (card.key)}
              <li animate:flip={LIST} in:receive={{ key: card.key }} out:send={{ key: card.key }}>
                <BoardCard {card} now={board.now} row={layout === 'list'} {cols} selected={selectedKey === card.key} onOpen={() => onOpen(card)} />
              </li>
            {:else}
              <li class="px-[15px] pb-1 font-sans text-[13px] leading-5 text-muted">{lane.empty}</li>
            {/each}
          </ul>
          {#if cards.length > LANE_CAP}
            <InboxMore class="mx-[15px] my-1.5" expanded={!!showAll[lane.id]} onclick={() => (showAll[lane.id] = !showAll[lane.id])}>
              {showAll[lane.id] ? 'Show fewer' : `Show ${cards.length - shown.length} more`}
            </InboxMore>
          {/if}
        </section>
      {/each}
    </div>
  </Materialize>
</section>
