<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'

  /** Compact header action: icon-only, tooltip via title. */
  let { title, onClick, children }: { title: string; onClick: () => void; children: Snippet } = $props()
</script>

<button
  type="button"
  {title}
  aria-label={title}
  onclick={onClick}
  class={cn('grid h-7 w-7 shrink-0 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg', focusGold)}
>
  {@render children()}
</button>
