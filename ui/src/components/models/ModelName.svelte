<script lang="ts">
  import { cn } from '@/lib/cn'

  // A model as a row names it: the catalog's human name leads and the id is
  // quiet mono beside or under it. A model the catalog does not name is its id.
  let {
    id,
    name,
    stacked = false,
    quiet = false,
    class: className,
  }: {
    id: string
    /** From `useModelNames()`. */
    name?: string
    /** The id sits under the name instead of beside it. */
    stacked?: boolean
    /** One secondary line with no id: a card footer, a summary. */
    quiet?: boolean
    class?: string
  } = $props()

  const named = $derived(!!name && name !== id)
</script>

{#if quiet}
  <span class={cn('min-w-0 truncate text-muted', named ? 'font-sans text-[13px] leading-5' : 'font-mono text-xs', className)} title={id}>
    {named ? name : id}
  </span>
{:else if named}
  <span class={cn('min-w-0', stacked ? 'block' : 'inline-flex max-w-full items-baseline gap-2', className)}>
    <span class={cn('truncate font-sans text-sm font-medium leading-5 text-fg', stacked && 'block')}>{name}</span>
    <span class={cn('truncate font-mono text-xs font-normal leading-5 text-muted', stacked && 'block')} title={id}>{id}</span>
  </span>
{:else}
  <span class={cn('min-w-0 truncate font-mono text-[13px] font-normal leading-5 text-fg', stacked && 'block', className)} title={id}>{id}</span>
{/if}
