<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { HTMLAttributes } from 'svelte/elements'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import { cn } from '@/lib/cn'

  // A finding, a warning or a result as a row of a `SettingsGroup`: the dot
  // carries the tone, the sentence stays in the body ink, and the one action
  // that fixes it sits at the end.
  let {
    status,
    action,
    class: className,
    children,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    status: DotStatus
    /** The one control that resolves it. */
    action?: Snippet
    class?: string
    children: Snippet
  } = $props()
</script>

<div class={cn('flex items-center gap-6 px-5 py-3.5', className)} {...rest}>
  <div class="flex min-w-0 flex-1 items-start gap-3">
    <StatusDot {status} class="mt-[7px]" />
    <p class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">{@render children()}</p>
  </div>
  {#if action}<div class="flex shrink-0 items-center gap-3">{@render action()}</div>{/if}
</div>
