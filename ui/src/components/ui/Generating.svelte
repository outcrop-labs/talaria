<script lang="ts">
  import { cn } from '@/lib/cn'
  import GeneratingBars from './GeneratingBars.svelte'
  import GeneratingDots from './GeneratingDots.svelte'
  import type { GeneratingVariant } from './generating'

  // An agent is writing something and the result will replace this block:
  // status line + `lines` rows of line-toned bars sweeping on the chosen
  // motif (default `scan` — the standard agent-stage tier).
  let {
    label,
    lines = 3,
    variant = 'scan',
    class: className,
  }: {
    label?: string
    lines?: number
    variant?: GeneratingVariant
    class?: string
  } = $props()

  const counts = [14, 18, 11, 16, 9, 13]
</script>

<div class={cn('rounded-lg border border-line p-4', className)}>
  {#if label}
    <div class="flex items-center gap-2 font-sans text-sm text-muted">
      <GeneratingDots />
      <span>{label}</span>
    </div>
  {/if}
  {#if lines > 0}
    <div class={cn('space-y-2.5 text-line', label && 'mt-3.5')}>
      {#each Array.from({ length: lines }) as _, i (i)}
        <GeneratingBars bars={counts[i % counts.length]!} {variant} delay={i * 0.15} class="flex" />
      {/each}
    </div>
  {/if}
</div>
