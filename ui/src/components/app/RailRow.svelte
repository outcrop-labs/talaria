<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  /** The rail row — one hover/active treatment everywhere. */
  let {
    active,
    onClick,
    class: className,
    children,
  }: {
    active?: boolean
    onClick: () => void
    class?: string
    children: Snippet
  } = $props()
</script>

<li>
  <button
    type="button"
    onclick={onClick}
    class={cn(
      'flex w-full min-w-0 items-center gap-1.5 rounded-md px-2 py-1.5 text-left font-sans text-sm transition-colors hover:bg-card2 hover:text-fg',
      active ? 'bg-raised text-fg' : 'text-muted',
      className,
    )}
  >
    {@render children()}
  </button>
</li>
