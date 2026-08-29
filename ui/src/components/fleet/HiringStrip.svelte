<script lang="ts">
  // The roster's hiring rows — what "creating an agent" looks like once the
  // work is a run instead of a held-open request. Each live hire shows the
  // run's own phase sentence (`rendering the fleet config`, `starting the
  // container`); a failed one shows its sentence until the window passes.
  //
  // Honesty rules inherited from <Generating>: no percentage, no sweep that
  // promises an end — the phases are the run's real words, and the row the
  // agent lands as replaces the row that hired it. A done hire renders
  // nothing at all: the real tile materializing over the strip IS the
  // completion event.
  import { CircleAlert } from '@lucide/svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { slide } from '@/lib/motion'
  import type { AgentHire } from '@/lib/fleet-defs'

  let { hires }: { hires: AgentHire[] } = $props()

  const live = $derived(hires.filter((h) => h.state === 'queued' || h.state === 'running'))
  const failed = $derived(hires.filter((h) => h.state === 'error'))
</script>

{#if live.length > 0 || failed.length > 0}
  <div transition:slide={{ duration: 150 }} class="space-y-3">
    <Panel class="divide-y divide-line">
      {#each live as h (h.id)}
        <div class="flex items-center gap-3 px-4 py-3">
          <WaitingMark site="fleet/agent-hire" size={12} class="shrink-0 text-accent" />
          <span class="truncate font-medium">{h.name}</span>
          <span class="truncate font-sans text-xs text-muted">{h.phase || 'queued'}</span>
          <span class="ml-auto shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">hiring</span>
        </div>
      {/each}
      {#each failed as h (h.id)}
        <div class="flex items-start gap-3 px-4 py-3">
          <CircleAlert size={14} class="mt-0.5 shrink-0 text-danger" />
          <span class="shrink-0 font-medium">{h.name}</span>
          <span class="min-w-0 text-sm text-danger">{h.error ?? 'the hire failed'}</span>
        </div>
      {/each}
    </Panel>
  </div>
{/if}
