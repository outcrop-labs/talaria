<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ChevronRight } from '@lucide/svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { cn } from '@/lib/cn'

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
    stacked = false,
    apart = false,
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
    /** The control takes the full width under the label (textareas, editors). */
    stacked?: boolean
    /** The control keeps its own width instead of sizing the column the
     *  group's rows share: the add row at the foot of an entity list. */
    apart?: boolean
    /** `label` makes the whole row the control's label. Only for a row whose
     *  control is one native input, select or textarea. */
    as?: 'div' | 'label'
    /** The whole row is a link to the page that owns it. */
    href?: string
    /** The whole row is a button. Put no other control in such a row. */
    onclick?: () => void
    /** Clicking the row opens the thing it is about, and the row can still
     *  hold a status, a fix button and a kebab. The row stays a `div`: the hit
     *  area is a button laid behind its content. */
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
  const openName = $derived(openLabel ?? (typeof label === 'string' ? label : undefined))
  const hitArea =
    'absolute inset-0 cursor-pointer transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2! focus-visible:outline-accent group-hover/row:bg-hover'
  const controlsTakeClicks =
    '[&_a]:pointer-events-auto [&_button]:pointer-events-auto [&_input]:pointer-events-auto [&_select]:pointer-events-auto [&_textarea]:pointer-events-auto [&_label]:pointer-events-auto [&_[role=switch]]:pointer-events-auto'
</script>

<svelte:element
  this={href ? 'a' : onclick ? 'button' : as}
  {href}
  {onclick}
  type={onclick ? 'button' : undefined}
  disabled={onclick ? disabled : undefined}
  aria-expanded={onclick ? expanded : undefined}
  data-settings-row={stacked ? 'stacked' : apart ? 'apart' : ''}
  class={cn(
    'flex min-h-[60px] px-5 py-4',
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
      !stacked && 'basis-48',
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
        <span class="min-w-0 truncate font-sans text-sm font-medium leading-5 text-fg">
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
  </div>
  {#if children || status || expanded !== undefined}
    <!-- empty:hidden: a control only some viewers get must not hold the row's gap open for the rest. -->
    <!-- A trailing kebab is pulled out by its own inset, so its dots end where a button in the next row ends. -->
    <div
      data-row-controls
      class={cn(
        'flex max-w-full items-center gap-3 empty:hidden [&>[data-row-menu]:last-child]:-mr-2',
        stacked ? 'w-full' : 'shrink-0 justify-end',
        // Only real controls take the click back: a read-only value in this slot still opens the row.
        opens && cn('pointer-events-none relative', controlsTakeClicks),
      )}
    >
      {#if status}
        <span data-row-status class="mr-auto flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">{@render status()}</span>
      {/if}
      <!-- No space before the chevron's block: a text node there would keep an empty control slot from collapsing. -->
      {@render children?.()}{#if expanded !== undefined}<ChevronRight size={16} aria-hidden="true" class={cn('text-muted transition-transform duration-150', expanded && 'rotate-90')} />{/if}
    </div>
  {/if}
</svelte:element>
