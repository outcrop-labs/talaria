<script lang="ts">
  import { formatTokens, type CostTotals } from '@/lib/cost.svelte'

  /** Daily strip, stacked local (bottom) + cloud (top). Unattributed tokens fall
   *  into the cloud segment's hue at reduced opacity via the remainder. */
  let { days }: { days: Array<CostTotals & { day: string; local: number; cloud: number }> } = $props()

  const max = $derived(Math.max(...days.map((d) => d.prompt + d.completion), 1))
  const px = (n: number) => `${Math.max((n / max) * 100, n > 0 ? 2 : 0)}%`
</script>

<div class="flex items-end gap-1" style:height="5.5rem" role="img" aria-label="Tokens per day, local vs cloud">
  {#each days as d (d.day)}
    {@const v = d.prompt + d.completion}
    {@const other = Math.max(v - d.local - d.cloud, 0)}
    {@const title = `${d.day}: ${formatTokens(v)} tokens (${formatTokens(d.local)} local, ${formatTokens(d.cloud)} cloud${
      other ? `, ${formatTokens(other)} unattributed` : ''
    }, ${d.generations} generations)`}
    <div class="group flex h-full min-w-0 flex-1 flex-col justify-end text-center" {title}>
      {#if other > 0}
        <div class="mx-auto w-full max-w-7 rounded-t opacity-40" style:height={px(other)} style:background="var(--theme-chart-1)"></div>
      {/if}
      {#if d.cloud > 0}
        <div
          class={`mx-auto w-full max-w-7 ${other ? 'mt-px' : 'rounded-t'}`}
          style:height={px(d.cloud)}
          style:background="var(--theme-chart-1)"
        ></div>
      {/if}
      {#if d.local > 0}
        <div
          class={`mx-auto w-full max-w-7 ${d.cloud || other ? 'mt-px' : 'rounded-t'}`}
          style:height={px(d.local)}
          style:background="var(--theme-success)"
        ></div>
      {/if}
      {#if v === 0}<div class="mx-auto w-full max-w-7 rounded-t bg-card2" style:height="1%"></div>{/if}
      <div class="mt-1 truncate font-mono text-[9px] text-muted">
        {new Date(`${d.day}T00:00:00`).toLocaleDateString([], { weekday: 'narrow' })}
      </div>
    </div>
  {/each}
</div>
