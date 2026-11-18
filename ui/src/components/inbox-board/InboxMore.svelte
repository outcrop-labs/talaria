<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ChevronDown } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'
  import { LINK, LINK_HOVER } from './inbox-board.svelte'

  let {
    expanded,
    onclick,
    class: className,
    children,
  }: { expanded: boolean; onclick: () => void; class?: string; children: Snippet } = $props()
</script>

<Button
  size="xs"
  variant="link"
  class={cn(LINK, LINK_HOVER, 'gap-1', className)}
  aria-expanded={expanded}
  {onclick}
>
  {@render children()}
  <ChevronDown size={13} aria-hidden="true" class={cn('shrink-0 text-muted transition-transform motion-reduce:transition-none', expanded && 'rotate-180')} />
</Button>
