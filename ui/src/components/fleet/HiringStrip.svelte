<script lang="ts">
  // The roster's hiring rows — what "creating an agent" looks like once the
  // work is a run instead of a held-open request. Each live hire shows the
  // run's own phase sentence (`rendering the fleet config`, `downloading the
  // agent image — 412 MB so far, 36 of 38 layers ready (4 min)`, `starting
  // the container`); a failed one shows its sentence and a Retry until the
  // window passes, a newer hire of the agent starts, or the agent runs (the
  // roster filters those out — `hiresToShow`).
  //
  // Honesty rules inherited from <Generating>: no percentage, no sweep that
  // promises an end — the phases are the run's real words (a download's
  // bytes are the daemon's own count), and the row the
  // agent lands as replaces the row that hired it. A done hire renders
  // nothing at all: the real tile materializing over the strip IS the
  // completion event.
  import { CircleAlert } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import Button from '@/components/ui/Button.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { slide } from '@/lib/motion'
  import { retryFleetHire, type AgentHire } from '@/lib/fleet-defs'

  let { hires }: { hires: AgentHire[] } = $props()

  const live = $derived(hires.filter((h) => h.state === 'queued' || h.state === 'running'))
  const failed = $derived(hires.filter((h) => h.state === 'error'))

  // RETRY runs the failed hire again on the same agent: its first run already
  // created the agent, so a second hire from the modal is refused (the handle
  // and the department are taken). The new run's row replaces this one.
  const qc = useQueryClient()
  let retrying = $state<string | null>(null)
  let retryErr = $state<Record<string, string>>({})
  const retry = async (h: AgentHire) => {
    retrying = h.id
    retryErr = { ...retryErr, [h.id]: '' }
    try {
      const r = await retryFleetHire(h.id)
      if (r.error) retryErr = { ...retryErr, [h.id]: r.error }
      await qc.invalidateQueries({ queryKey: ['fleet-hires'] })
    } finally {
      retrying = null
    }
  }
</script>

{#if live.length > 0 || failed.length > 0}
  <div transition:slide={{ duration: 150 }}>
    <SettingsGroup>
      {#each live as h (h.id)}
        <div class="flex min-h-[52px] items-center gap-3 px-5 py-3">
          <WaitingMark site="fleet/agent-hire" size={12} class="shrink-0 text-accent" />
          <span class="shrink-0 font-sans text-sm font-medium leading-5 text-fg">Hiring {h.name}</span>
          <span class="min-w-0 truncate font-sans text-[13px] leading-5 text-muted">{h.phase || 'Queued'}</span>
        </div>
      {/each}
      {#each failed as h (h.id)}
        <div class="flex min-h-[52px] items-start gap-3 px-5 py-3">
          <CircleAlert size={16} class="mt-0.5 shrink-0 text-danger" />
          <div class="min-w-0 flex-1">
            <div class="font-sans text-sm font-medium leading-5 text-fg">{h.name} was not hired</div>
            <div class="font-sans text-[13px] leading-5 text-danger">{h.error ?? 'The hire failed'}</div>
            {#if retryErr[h.id]}<div class="font-sans text-[13px] leading-5 text-muted">{retryErr[h.id]}</div>{/if}
          </div>
          <Button variant="outline" size="sm" class="shrink-0" disabled={retrying !== null} onclick={() => void retry(h)}>
            {retrying === h.id ? 'Retrying' : 'Retry'}
          </Button>
        </div>
      {/each}
    </SettingsGroup>
  </div>
{/if}
