<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import { focusRing } from '@/components/ui/control'
  import { cn } from '@/lib/cn'
  import { blockerOf, cardMeta, glyphOf, type InboxCard } from '@/lib/inbox-board'
  import { wordClamp } from '@/lib/word-clamp'
  import Owners from './Owners.svelte'
  import StateGlyph from './StateGlyph.svelte'
  import { COLUMN, COLUMNS, type ListColumns } from './inbox-board.svelte'

  let {
    card,
    now,
    selected,
    row = false,
    cols = { hasStatus: true, hasTypes: true, hasRefs: true },
    onOpen,
  }: {
    card: InboxCard
    now: number
    /** The drawer is showing this card. */
    selected: boolean
    /** Drawn as a row of the list, not a card of the board. */
    row?: boolean
    /** The columns this row's list section has. */
    cols?: ListColumns
    onOpen: () => void
  } = $props()

  const meta = $derived(cardMeta(card, now))
  const done = $derived(card.state === 'done')
  const stuck = $derived(blockerOf(card) !== null)
  const lead = $derived(!row && !done ? card.summary : null)
  const EDGE = 'border-l-[3px] border-l-danger'
  const WASH = 'bg-[image:linear-gradient(var(--inbox-danger-wash),var(--inbox-danger-wash))]'
  const RING = 'ring-[1.5px] ring-[color:var(--inbox-ink-soft)]'
</script>

{#snippet word()}<span class="font-medium text-fg">{meta.word}</span>{/snippet}
{#snippet opens(place: string)}
  <ChevronRight
    size={16}
    aria-hidden="true"
    class={cn(
      'absolute text-muted transition-opacity duration-150 group-hover:text-fg group-hover:opacity-100 group-focus-visible:text-fg group-focus-visible:opacity-100 motion-reduce:transition-none',
      !selected && 'opacity-0',
      place,
    )}
  />
{/snippet}
{#snippet dot()}<span aria-hidden="true">{' · '}</span>{/snippet}
{#snippet ticket()}<span class="font-mono text-[11px]">{meta.ref.ref}</span>{/snippet}

<button
  id="inbox-card:{card.key}"
  type="button"
  data-inbox-card
  data-state={card.state}
  data-glyph={glyphOf(card)}
  aria-haspopup="dialog"
  aria-expanded={selected}
  onclick={onOpen}
  class={cn(
    'group relative grid w-full text-left transition-[border-color,box-shadow,background-color] duration-150 data-[flash]:ring-2 data-[flash]:ring-accent/60',
    focusRing,
    row
      ? cn(
          'min-h-11 grid-cols-[auto_minmax(0,1fr)_auto] items-baseline gap-x-3 py-3 pl-3 pr-[38px] hover:bg-hover focus-visible:-outline-offset-2',
          selected && cn(RING, 'ring-inset'),
          stuck && cn(EDGE, 'pl-[9px]', !selected && WASH),
        )
      : cn(
          'gap-0.5 rounded-lg border px-3 py-2.5',
          done ? 'bg-[var(--inbox-done-card)]' : 'bg-raised',
          selected ? cn(RING, 'border-transparent') : 'border-line hover:border-line-strong',
          stuck && cn(EDGE, 'pl-2.5', !selected && WASH),
        ),
  )}
>
  {#snippet title()}
    <span
      use:wordClamp={card.title}
      class={cn(
        'min-w-0 flex-1 break-words font-sans text-[14px] leading-5',
        done ? 'text-[color:var(--inbox-ink-soft)]' : 'font-medium text-fg',
        row ? 'line-clamp-2 @[60rem]:line-clamp-1' : 'line-clamp-2',
      )}
    ></span>
  {/snippet}
  {#if row}
    <span class="self-start pt-0.5"><StateGlyph glyph={glyphOf(card)} /></span>
    {@render title()}
    <span class={cn('font-sans text-[12px] leading-5 tabular-nums text-muted', COLUMNS)}>
      {#if cols.hasStatus}<span class={COLUMN.status}>{@render word()}</span>{/if}
      {#if cols.hasTypes}<span class={COLUMN.type}>{meta.ref.kind}</span>{/if}
      <span class={COLUMN.time}>{meta.time}</span>
      {#if cols.hasRefs}
        <span class={COLUMN.ref}>
          {#if meta.ref.ref}{@render ticket()}{:else}<span aria-hidden="true">—</span><span class="sr-only">No ref</span>{/if}
        </span>
      {/if}
      <span class={COLUMN.owner}><Owners people={meta.owners} /></span>
    </span>
    {@render opens('right-3 top-[14px]')}
  {:else}
    <span class="flex min-w-0 items-start gap-2 pr-4">
      <span class="mt-0.5"><StateGlyph glyph={glyphOf(card)} /></span>
      {@render title()}
    </span>
    {@render opens('right-2 top-3')}
    {#if lead}
      <span use:wordClamp={lead} class="line-clamp-1 break-words pl-6 font-sans text-[13px] leading-[19px] text-muted"></span>
    {/if}
  {/if}
  <span class={cn('block min-w-0 truncate font-sans text-[12px] leading-5 tabular-nums text-muted', row ? 'col-span-2 col-start-2 @[60rem]:hidden' : 'pl-6')}>
    {#if meta.statusPrefix}{@render word()}{@render dot()}{/if}{meta.time}{#if meta.ref.kind}{@render dot()}{meta.ref.kind}{/if}{#if meta.ref.ref}{@render dot()}{@render ticket()}{/if}{@render dot()}<Owners people={meta.owners} />
  </span>
</button>
