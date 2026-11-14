<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ChevronRight } from '@lucide/svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { cn } from '@/lib/cn'
  import SavedMark from './SavedMark.svelte'

  // One row of a `SettingsGroup`: what it is on the left, its control on the
  // right. Also the row of an entity list, with `icon`, `meta` and `status`.
  // Inside a group the right side is one shared column, so down a list every
  // status starts at the same place and every control ends at the card edge.
  let {
    label,
    hint,
    info,
    icon,
    lead,
    meta,
    status,
    saved,
    stacked = false,
    apart = false,
    wrap = false,
    compact = false,
    as = 'div',
    href,
    onclick,
    onOpen,
    openHref,
    openLabel,
    openTitle,
    expanded,
    disabled = false,
    class: className,
    children,
  }: {
    label: string | Snippet
    /** One sentence, or an entity's line of meta. */
    hint?: string | Snippet
    /** The longer explanation, behind an InfoTip beside the label. */
    info?: string
    /** Drawn in a 32px tile ahead of the label. */
    icon?: Snippet
    /** A bare mark ahead of the label, with no tile: a status dot, an avatar. */
    lead?: Snippet
    /** Inline after the label: chips, identifiers. */
    meta?: Snippet
    /** The state of the thing, ahead of the control: a `StatusDot` and a word. */
    status?: Snippet
    /** The row's control saves on change, and a save just landed: a check and
     *  "Saved" show in the status position for as long as this is true. Pass
     *  `autosave.saved(key)` from `useAutosave()`, which is only ever true on
     *  a page whose header shows no autosave cue. */
    saved?: boolean
    /** The control takes the full width under the label (textareas, editors). */
    stacked?: boolean
    /** The control keeps its own width instead of sizing the column the
     *  group's rows share: the add row at the foot of an entity list. */
    apart?: boolean
    /** The label is a name (an agent, a key, a person): it wraps to a second
     *  line before it is cut, and a string label is its own tooltip. */
    wrap?: boolean
    /** One line at a table row's height, for a list that is read down its
     *  columns: an alert summary, an invite, a per-agent switch. */
    compact?: boolean
    /** `label` makes the whole row the control's label. Only for a row whose
     *  control is one native input, select or textarea. */
    as?: 'div' | 'label'
    /** The whole row is a link to the page that owns it. */
    href?: string
    /** The whole row is a button. Put no other control in such a row. */
    onclick?: () => void
    /** Clicking the row opens the thing it is about, and the row can still
     *  hold a status and a kebab. The row stays a `div`: the hit area is a
     *  button laid behind its content. The row ends in a chevron, after the
     *  kebab, and shows no action button: what its state needs is in the
     *  kebab and behind the row. */
    onOpen?: () => void
    /** The same, when opening is going somewhere: the hit area is a link, so
     *  it opens in a new tab like any other. */
    openHref?: string
    /** The hit area's accessible name, when the label alone would not say
     *  what opens ("Manage GitHub"). Defaults to a string label. */
    openLabel?: string
    /** A tooltip on the hit area (an id the row no longer shows). */
    openTitle?: string
    /** With `onclick`: the row opens and closes what follows it. It draws the
     *  chevron and says which way it is. */
    expanded?: boolean
    /** With `onclick`: the row is inert and does not light on hover. */
    disabled?: boolean
    class?: string
    /** The control. */
    children?: Snippet
  } = $props()

  const opens = $derived(Boolean(onOpen || openHref))
  // A row that opens and closes what follows it already draws its own chevron.
  const pointsOnward = $derived(opens && expanded === undefined && !stacked)
  const openName = $derived(openLabel ?? (typeof label === 'string' ? label : undefined))
  const hitArea =
    'absolute inset-0 cursor-pointer transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2! focus-visible:outline-accent group-hover/row:bg-hover'
  const controlsTakeClicks =
    '[&_a]:pointer-events-auto [&_button]:pointer-events-auto [&_input]:pointer-events-auto [&_select]:pointer-events-auto [&_textarea]:pointer-events-auto [&_label]:pointer-events-auto [&_[role=switch]]:pointer-events-auto'
</script>

{#snippet savedFlash(at: string)}
  <!-- The fill covers a long hint for the moment. -->
  <SavedMark on={saved === true} class={cn('pointer-events-none absolute top-1/2 -translate-y-1/2 bg-panel pl-2', at)} />
{/snippet}

<svelte:element
  this={href ? 'a' : onclick ? 'button' : as}
  {href}
  {onclick}
  type={onclick ? 'button' : undefined}
  disabled={onclick ? disabled : undefined}
  aria-expanded={onclick ? expanded : undefined}
  data-settings-row={stacked ? 'stacked' : apart ? 'apart' : ''}
  data-row-opens={opens ? '' : undefined}
  class={cn(
    compact ? 'flex min-h-11 px-5 py-1.5' : 'flex min-h-[52px] px-5 py-3',
    // Not stacked still wraps: on a phone a wide control drops under its label.
    stacked ? 'flex-col gap-3' : 'flex-wrap items-center gap-x-6 gap-y-3',
    // The ring is forced inside: the app's global focus rule is unlayered, so it beats a plain utility, and a ring
    // outside the row is cut off by the card.
    (href || onclick) &&
      'w-full text-left transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2! focus-visible:outline-accent enabled:hover:bg-hover [a&]:hover:bg-hover',
    opens && 'group/row relative',
    className,
  )}
>
  {#if openHref}
    <a href={openHref} title={openTitle} aria-label={openName} class={hitArea}></a>
  {:else if onOpen}
    <button type="button" onclick={onOpen} title={openTitle} aria-label={openName} aria-expanded={expanded} class={hitArea}></button>
  {/if}
  <div
    class={cn(
      'flex min-w-0 flex-1 items-center gap-3',
      stacked ? 'relative' : 'basis-48',
      // The hit area sits behind: the label lets clicks through, and anything interactive inside it takes them back.
      opens && 'pointer-events-none relative [&_a]:pointer-events-auto [&_button]:pointer-events-auto',
    )}
  >
    {#if icon}
      <div class="grid h-8 w-8 shrink-0 place-items-center rounded-md border border-line bg-raised text-muted">
        {@render icon()}
      </div>
    {:else if lead}
      <div class="grid shrink-0 place-items-center">{@render lead()}</div>
    {/if}
    <div class="min-w-0 flex-1">
      <div class="flex min-w-0 items-center gap-2">
        <span
          class={wrap ? 'line-clamp-2 min-w-0 break-words font-sans text-sm font-medium leading-5 text-fg' : 'min-w-0 truncate font-sans text-sm font-medium leading-5 text-fg'}
          title={wrap && typeof label === 'string' ? label : undefined}
        >
          {#if typeof label === 'string'}{label}{:else}{@render label()}{/if}
        </span>
        {#if info}<InfoTip text={info} />{/if}
        {@render meta?.()}
      </div>
      {#if hint}
        <div class="font-sans text-[13px] leading-5 text-muted">
          {#if typeof hint === 'string'}{hint}{:else}{@render hint()}{/if}
        </div>
      {/if}
    </div>
    <!-- A stacked control fills the row, so its flash sits on the label's line. -->
    {#if stacked && saved !== undefined}{@render savedFlash('right-0')}{/if}
  </div>
  {#if children || status || saved !== undefined || expanded !== undefined || pointsOnward}
    <!-- empty:hidden: a control only some viewers get must not hold the row's gap open for the rest. -->
    <div
      data-row-controls
      class={cn(
        'flex max-w-full items-center gap-3 empty:hidden',
        stacked ? 'w-full' : 'shrink-0 justify-end',
        // Only real controls take the click back: a read-only value in this slot still opens the row.
        opens && cn('pointer-events-none relative', controlsTakeClicks),
      )}
    >
      {#if status}
        <span data-row-status class="relative mr-auto flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
          <span class={cn('flex items-center gap-2', saved && 'invisible')}>{@render status()}</span>
          {#if saved !== undefined}{@render savedFlash('right-0')}{/if}
        </span>
      {:else if saved !== undefined && !stacked}
        <!-- Takes no room, so the flash moves nothing: it hangs to the left of the control, over the row's gap. -->
        <span class="relative -mr-3 w-0 self-stretch">{@render savedFlash('right-3')}</span>
      {/if}
      <!-- No space before the chevron's block: a text node there would keep an empty control slot from collapsing. -->
      {@render children?.()}{#if expanded !== undefined}<span data-row-expander aria-hidden="true" class="-mr-1 grid w-4 shrink-0 place-items-center text-muted/70 transition-colors group-hover/row:text-fg"><ChevronRight size={16} class={cn('transition-transform duration-150', expanded && 'rotate-90')} /></span>{:else if pointsOnward}<span data-row-chevron aria-hidden="true" class="-mr-1 grid w-4 shrink-0 place-items-center text-muted/70 transition-colors group-hover/row:text-fg"><ChevronRight size={16} /></span>{/if}
    </div>
  {/if}
</svelte:element>

<style>
  /* One thing at the end of a row at rest. A row that opens ends in its
     chevron, and a row that names its action (`SettingsRowAction`) ends in
     that button: either shows its kebab when the row is hovered, holds
     focus, or has its menu open. Opacity, not visibility, so the kebab keeps
     its cell, its place in the tab order and its name. A device that cannot
     hover always shows it. */
  @media (hover: hover) {
    :global {
      :is([data-row-opens], [data-settings-row]:has(> [data-row-controls] [data-row-action])) {
        > [data-row-controls] [data-row-menu] {
          opacity: 0;
          transition: opacity 120ms;
        }
        &:hover > [data-row-controls] [data-row-menu],
        &:focus-within > [data-row-controls] [data-row-menu],
        > [data-row-controls] [data-row-menu]:has([data-menu-open]) {
          opacity: 1;
        }
      }
    }
  }
  /* A row that names its action ends in the button alone, whether it also
     expands or opens: the row still takes the click, and its menu says "Show
     details" or "Manage". The kebab is then the last thing drawn before the
     button, so it takes the inset a last cell has. */
  :global([data-settings-row]:has(> [data-row-controls] [data-row-action]) > [data-row-controls]) {
    > :global([data-row-expander]),
    > :global([data-row-chevron]) {
      display: none;
    }
    > :global([data-row-menu]:has(+ [data-row-expander])),
    > :global([data-row-menu]:has(+ [data-row-chevron])) {
      margin-right: -0.5rem;
    }
    /* The named action is what the row ends in, so it sits at the row's edge
       as a chevron does, and the kebab's cell is held on its inner side. */
    > :global([data-row-action]) {
      order: 1;
    }
  }
</style>
