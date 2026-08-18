<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // One 28px icon button on a message row's hover toolbar (see MessageRow.svelte).
  let {
    title,
    danger,
    onClick,
    children,
  }: {
    title: string
    danger?: boolean
    onClick: () => void
    children: Snippet
  } = $props()
</script>

<button data-dither-fill
  type="button"
  {title}
  onclick={onClick}
  class={cn(
    'grid h-7 w-7 place-items-center rounded-md text-muted transition-colors',
    danger ? 'hover:text-[color:var(--theme-danger)]' : 'hover:text-fg',
  )}
>
  {@render children()}
</button>
