<script lang="ts">
  import type { Snippet } from 'svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import { cn } from '@/lib/cn'

  // The state of the thing a row is about: a dot and a word.
  let {
    status,
    title,
    class: className,
    children,
  }: { status: DotStatus; title?: string; class?: string; children: Snippet } = $props()
</script>

<span {title} class={cn('flex shrink-0 items-center gap-2 font-sans text-[13px] leading-5 text-muted', className)}>
  <StatusDot {status} />{@render children()}
</span>
