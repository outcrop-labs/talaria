<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // The caption row of a `SettingsGroup` whose rows are aligned columns of
  // different facts. It is a row of the group's own grid, so its captions sit
  // in the column the rows' controls share, and it holds the kebab and
  // chevron cells the rows end in. Give each caption the width class its
  // cells have.
  let {
    label,
    lead,
    leadClass,
    menu = false,
    expands = false,
    class: className,
    children,
  }: {
    /** Names the rows' label column ("Name", "Decision"). */
    label: string
    /** Names the column the rows' `lead` makes ("Severity"). */
    lead?: string
    /** The width class the rows' lead has. */
    leadClass?: string
    /** The rows end in a `SettingsRowMenu`: hold its cell. */
    menu?: boolean
    /** The rows end in the chevron that opens and closes them (`expanded`):
     *  hold its cell, marked as a chevron so the group does not hold a second
     *  one. A chevron that points onward needs nothing, the group holds that
     *  cell itself. */
    expands?: boolean
    class?: string
    /** One caption for each cell a row puts in its control slot, in order. */
    children?: Snippet
  } = $props()
</script>

<div
  data-settings-row=""
  data-header-row
  class={cn('flex h-9 items-center gap-x-6 px-5 font-sans text-xs font-medium leading-4 text-muted', className)}
>
  {#if lead}
    <span class="flex min-w-0 flex-1 basis-48 items-center gap-3">
      <span class={cn('shrink-0', leadClass)}>{lead}</span>
      <span class="min-w-0">{label}</span>
    </span>
  {:else}
    <span class="min-w-0 flex-1 basis-48">{label}</span>
  {/if}
  <div data-row-controls class="flex shrink-0 items-center justify-end gap-3">
    {@render children?.()}
    {#if menu}<span data-row-menu class="w-8 shrink-0 last:-mr-2"></span>{/if}
    {#if expands}<span data-row-chevron class="-mr-1 w-4 shrink-0"></span>{/if}
  </div>
</div>
