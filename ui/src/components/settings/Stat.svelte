<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // One stat of a `StatStrip`: a caption, the number, one muted line. With
  // `href` the whole cell is a link to the page that owns the number.
  let {
    label,
    value,
    sub,
    href,
    class: className,
  }: {
    label: string
    value: string | number | Snippet
    sub?: string | Snippet
    href?: string
    class?: string
  } = $props()
</script>

<svelte:element
  this={href ? 'a' : 'div'}
  {href}
  class={cn(
    'block min-w-0 border-b border-r border-line-subtle px-5 py-4',
    href &&
      'transition-colors hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
    className,
  )}
>
  <div class="truncate font-sans text-xs leading-4 text-muted">{label}</div>
  <div class="mt-1 font-sans text-2xl font-semibold leading-8 tabular-nums text-fg">
    {#if typeof value === 'function'}{@render value()}{:else}{value}{/if}
  </div>
  {#if sub}
    <div class="truncate font-sans text-[13px] leading-5 text-muted">
      {#if typeof sub === 'string'}{sub}{:else}{@render sub()}{/if}
    </div>
  {/if}
</svelte:element>
