<script lang="ts">
  import { slide } from '@/lib/motion'
  import type { GuardFinding } from './guard-caveat'

  // The confab guard's caveat on a flagged agent reply (annotate/strict modes).
  // Renders from message metadata — the findings are never part of the reply
  // text, so they can't leak back into any transcript.
  let { findings }: { findings?: GuardFinding[] | null } = $props()
</script>

{#if findings?.length}
  <!-- Attention panel (spec §8): gold hairline + mono uppercase label; body
       stays readable sans on the readout tone. -->
  <div transition:slide={{ duration: 150 }} class="mt-2 space-y-1 rounded-md border px-2.5 py-2 text-xs" style="border-color: var(--theme-warning)">
    <div class="font-mono text-[10px] uppercase tracking-[0.08em]" style="color: var(--theme-warning)">
      Unverified · confab guard flagged this reply
    </div>
    {#each findings as f, i (`${f.check}-${i}`)}
      <div class="font-sans text-fg">
        <span class="font-mono text-[10px] uppercase tracking-[0.05em]" style="color: var(--theme-warning)">
          {f.check.replace(/_/g, ' ')}
        </span>
        {' · '}
        {f.message}
        {#if f.snippet}<span class="text-muted"> ({f.snippet})</span>{/if}
      </div>
    {/each}
  </div>
{/if}
