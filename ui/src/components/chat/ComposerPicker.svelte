<script lang="ts">
  // The composer's compact choice chip — ONE component for every decision a
  // composer (or a header beside one) needs at hand: an optional mark + the
  // current value, opening a portaled popover. Use for anything a composer
  // needs decided at send time (model tier, reasoning effort, research depth,
  // acting agent) instead of parking wide controls in rails or headers.
  //
  // The two chips that were once components of their own — the tier chip and
  // the effort chip — render through here now, so the shape is parameterized
  // rather than forked: `chipVariant` picks the rail chip's border weight,
  // `meter` puts the 3×12 ladder on the trigger AND on every catalog row,
  // `autoOption` adds the ingress row for "leave it to the model", `label`
  // overrides the trigger readout, and `searchable`/`searchPlaceholder` and
  // `menuClass` cover the ladders too short for a filter and the panels drawn
  // at their own width.
  //
  // Mercury (spec §7): a secondary 36px mono chip (hairline border, muted →
  // readout on hover) or its primary sibling (strong border, readout) over the
  // §7 popover pattern — search row with ⌘K hint, panel bg, mono section
  // header, right-aligned mono meta, hover fill, dashed-gold selected row. The
  // shell (ui/Popover) owns the portal/outside-click/Esc mechanics and follows
  // the chip on scroll.
  import type { LucideIcon as IconType } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import Popover from '@/components/ui/Popover.svelte'
  import MeterBars from '@/components/chat/MeterBars.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { chipPrimary, chipSecondary, popHeader, popRow, popRowSelected } from '@/components/chat/chat-chrome'
  import type { ComposerOption } from './composer-picker'

  let {
    icon: Icon,
    value,
    options,
    onChange,
    title,
    menuLabel,
    label,
    placement = 'top',
    chipVariant = 'secondary',
    meter,
    autoOption,
    searchable = true,
    searchPlaceholder,
    disabled = false,
    menuClass = 'min-w-56',
    class: className,
  }: {
    /** The chip's leading mark — a Lucide image, or a text glyph for a chip
     *  that draws one in the mono face ('✳', the tier chip). Absent on the
     *  effort chip, which leads with its value. */
    icon?: IconType | string
    value: string
    options: ComposerOption[]
    onChange: (v: string) => void
    title: string
    menuLabel: string
    /** The trigger's readout, for a chip that must read differently from the
     *  row it is standing on: the tier chip says 'main', its row says 'main
     *  model'. Defaults to the selected row's label, then to the raw value. */
    label?: string
    /** Which side of the trigger the popover anchors to. Composers sit at
     *  the bottom of their surface, so they open UP ('top'); a picker living
     *  in a header at the top of a stage opens DOWN, or the panel would fly
     *  off-screen. */
    placement?: 'top' | 'bottom'
    /** Rail chips (primary) read brighter than the ones introducing their
     *  value with an icon. The primary ones also budget less for their label,
     *  which shares the row with a meter. */
    chipVariant?: 'primary' | 'secondary'
    /** The ladder this pick sits on: a 3×12 bar meter in the trigger and on
     *  every catalog row. `lit` is the caller's because the empty value reads
     *  differently per chip — the tier chip lights its bottom rung for 'main',
     *  the effort chip lights nothing for the model's own default. Row rungs
     *  come from an option's place in `options`, never from its place in the
     *  filtered view, so a search cannot relight the ladder. */
    meter?: { total: number; lit: number }
    /** The ingress row, above `options`, for the value that means "leave it to
     *  the model". It is not a rung of the ladder: no meter, and its `sub` is a
     *  fixed caption in the chip's own uppercase mono rather than catalog meta.
     *  `value`/`onChange` address it exactly like a catalog row. */
    autoOption?: ComposerOption
    /** The §7 search row. Off for ladders short enough that a filter over them
     *  is furniture. */
    searchable?: boolean
    /** The search row's placeholder, where the chip's own words read better
     *  than the derived one ('Search tiers' beats 'Search model tier' under a
     *  'Model tier' header). Defaults to `Search <menuLabel>`. */
    searchPlaceholder?: string
    disabled?: boolean
    /** The popover panel's classes (Popover's own `class`): the panel's min
     *  width is a per-chip call, so a call site overrides the width here —
     *  `overflow-hidden` stays the panel's. */
    menuClass?: string
    class?: string
  } = $props()

  let open = $state(false)
  let q = $state('')

  // Fresh search on every open.
  $effect(() => {
    if (open) q = ''
  })

  const current = $derived(options.find((o) => o.value === value))
  const readout = $derived(label ?? current?.label ?? value)
  // The two chip lineages drew their own label budget and each still does: the
  // primary chips cap at 24, the secondary icon chips at 28. A tier or template
  // name is long enough for the cap to show, so it rides the variant.
  const labelClass = $derived(chipVariant === 'primary' ? 'max-w-24 truncate' : 'max-w-28 truncate')
  // Row meta: right-aligned mono 10px. The ingress caption is the exception —
  // a fixed label ("model default") rather than a value, so it wears the
  // chip's uppercase voice.
  const metaClass = 'max-w-44 shrink-0 truncate text-right font-mono text-[10px] tracking-[0.05em] text-ink-dim'
  const ingressClass = 'shrink-0 text-right font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim'

  const needle = $derived(q.trim().toLowerCase())
  const matches = (o: ComposerOption) =>
    !needle || o.label.toLowerCase().includes(needle) || !!o.sub?.toLowerCase().includes(needle)
  const rows = $derived([
    ...(autoOption && matches(autoOption) ? [{ o: autoOption, lit: 0, ingress: true }] : []),
    ...options.map((o, i) => ({ o, lit: i + 1, ingress: false })).filter(({ o }) => matches(o)),
  ])
</script>

<Popover bind:open follow up={placement === 'top'} offset={6} class={cn('overflow-hidden', menuClass)}>
  {#snippet trigger()}
    <button
      type="button"
      {disabled}
      class={cn(chipVariant === 'primary' ? chipPrimary : chipSecondary, className)}
      {title}
      aria-haspopup="listbox"
      aria-expanded={open}
    >
      {#if typeof Icon === 'string'}
        <span aria-hidden="true" class="text-[10px] leading-none">{Icon}</span>
      {:else if Icon}
        <Icon size={12} />
      {/if}
      <span class={labelClass}>{readout}</span>
      {#if meter}
        <MeterBars total={meter.total} lit={meter.lit} />
      {/if}
    </button>
  {/snippet}
  {#snippet content(close)}
    {#if searchable}
      <PopSearch value={q} onChange={(v) => (q = v)} placeholder={searchPlaceholder ?? `Search ${menuLabel.toLowerCase()}`} />
    {/if}
    <div class={popHeader}>{menuLabel}</div>
    {#each rows as { o, lit, ingress } (o.value)}
      <button
        type="button"
        onclick={() => {
          onChange(o.value)
          close()
        }}
        class={cn(popRow, o.value === value ? popRowSelected : 'text-muted')}
      >
        <span class="min-w-0 flex-1 truncate">{o.label}</span>
        {#if o.sub}
          <span class={ingress ? ingressClass : metaClass}>{o.sub}</span>
        {/if}
        {#if meter && !ingress}
          <MeterBars total={meter.total} lit={lit} class="shrink-0" />
        {/if}
      </button>
    {/each}
    {#if searchable && rows.length === 0}
      <div class="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</div>
    {/if}
  {/snippet}
</Popover>