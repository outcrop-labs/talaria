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
    meta,
    stacked = false,
    as = 'div',
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
    /** Inline after the label: chips, identifiers. */
    meta?: Snippet
    /** The control takes the full width under the label (textareas, editors). */
    stacked?: boolean
    /** `label` makes the whole row the control's label. Only for a row whose
     *  control is one native input, select or textarea. */
    as?: 'div' | 'label'
    class?: string
    /** The control. */
    children?: Snippet
  } = $props()
</script>

<svelte:element
  this={as}
  class={cn('flex min-h-[60px] px-5 py-4', stacked ? 'flex-col gap-3' : 'items-center gap-6', className)}
>
  <div class="flex min-w-0 flex-1 items-center gap-3">
    {#if icon}
      <div class="grid h-8 w-8 shrink-0 place-items-center rounded-md border border-line bg-raised text-muted">
        {@render icon()}
      </div>
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
