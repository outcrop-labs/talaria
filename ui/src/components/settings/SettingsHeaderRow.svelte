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
    menu = false,
    class: className,
    children,
  }: {
    /** Names the rows' label column ("Name", "Decision"). */
    label: string
    /** The rows end in a `SettingsRowMenu`: hold its cell. */
    menu?: boolean
    class?: string
    /** One caption for each cell a row puts in its control slot, in order. */
    children?: Snippet
  } = $props()
</script>

<div
  data-settings-row=""
  data-header-row
  class={cn('flex h-10 items-center gap-x-6 px-5 font-sans text-xs font-medium leading-4 text-muted', className)}
>
  <span class="min-w-0 flex-1 basis-48">{label}</span>
  <div data-row-controls class="flex shrink-0 items-center justify-end gap-3">
    {@render children?.()}
    {#if menu}<span data-row-menu class="w-8 shrink-0 last:-mr-2"></span>{/if}
  </div>
</div>
