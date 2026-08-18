<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ChevronRight } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // A collapsible group label. Title-case at 11px rather than the rail's uppercase
  // micro-label — that treatment is reserved for the top-level sections, and long
  // team names ("Marketing + Tech Collab") are unreadable in wide-tracked caps.
  let {
    label,
    icon,
    count,
    open,
    onToggle,
  }: {
    label: string
    icon?: Snippet
    count: number
    open: boolean
    onToggle: () => void
  } = $props()
</script>

<button
  type="button"
  onclick={onToggle}
  aria-expanded={open}
  class="flex w-full items-center gap-1 rounded-md py-1 pl-1 pr-2 text-left text-[11px] text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
>
  <ChevronRight size={11} class={cn('shrink-0 transition-transform duration-150', open && 'rotate-90')} />
  {@render icon?.()}
  <span class="min-w-0 flex-1 truncate">{label}</span>
  <span class="shrink-0 text-[10px] tabular-nums opacity-50">{count}</span>
</button>
