<script lang="ts">
  import type { Snippet } from 'svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { cn } from '@/lib/cn'

  // One row of a `SettingsGroup`: what it is on the left, its control on the
  // right. Also the row of an entity list, with `icon` and `meta`.
  let {
    label,
    hint,
    info,
    icon,
    lead,
    meta,
    stacked = false,
    as = 'div',
    href,
    onclick,
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
    /** The control takes the full width under the label (textareas, editors). */
    stacked?: boolean
    /** `label` makes the whole row the control's label. Only for a row whose
     *  control is one native input, select or textarea. */
    as?: 'div' | 'label'
    /** The whole row is a link to the page that owns it. */
    href?: string
    /** The whole row is a button. Put no other control in such a row. */
    onclick?: () => void
    /** With `onclick`: the row is inert and does not light on hover. */
    disabled?: boolean
    class?: string
    /** The control. */
    children?: Snippet
  } = $props()
</script>

<svelte:element
  this={href ? 'a' : onclick ? 'button' : as}
  {href}
  {onclick}
  type={onclick ? 'button' : undefined}
  disabled={onclick ? disabled : undefined}
  class={cn(
    'flex min-h-[60px] px-5 py-4',
    stacked ? 'flex-col gap-3' : 'items-center gap-6',
    (href || onclick) &&
      'w-full text-left transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent enabled:hover:bg-hover [a&]:hover:bg-hover',
    className,
  )}
>
  <div class="flex min-w-0 flex-1 items-center gap-3">
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
  {#if children}
    <div class={cn('flex items-center gap-3', stacked ? 'w-full' : 'shrink-0')}>{@render children()}</div>
  {/if}
</svelte:element>
