<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { ROLL_STILL_STARTING, type FleetRollRow } from '@/lib/fleet-defs'

  // One agent's place in a fresh restart (a roll). Restarting is a live state
  // and carries the activity mark; done is a quiet word; failed must say why,
  // so the failure sentence rides the title and, where the frame has the room
  // (`sentence`), is written out.
  let { roll, sentence = false, class: className }: { roll?: FleetRollRow; sentence?: boolean; class?: string } = $props()

  const state = $derived(roll?.state ?? 'idle')
  const detail = $derived(roll?.detail ?? null)
  const queued = $derived(state === 'rolling' && detail === 'queued')
  // Recreated, and slower to report healthy than the roll waits: the row
  // stays rolling with this note until the container is up (done) or the
  // late wait gives up (failed) — never a lasting "failed" for an agent that
  // then comes up (talaria-fleet-reconcile STILL_STARTING).
  const starting = $derived(state === 'rolling' && detail === ROLL_STILL_STARTING)
</script>

{#if roll && state !== 'idle'}
  {#if state === 'rolling'}
    <Chip
      class={className}
      title={queued
        ? 'Queued. It restarts when a slot frees up.'
        : starting
          ? 'The fresh container is up but has not reported healthy yet. The restart finishes when it does.'
          : 'Restarting on a fresh container. Offline for about a minute.'}
    >
      <WaitingMark site="fleet/agent-controls" size={10} class="mr-1 text-accent" />
      {queued ? 'Queued' : starting ? 'Starting' : 'Restarting'}
    </Chip>
  {:else if state === 'done'}
    <Chip tone="success" class={className} title="The fresh container took over.">Restarted</Chip>
  {:else}
    <Chip tone="danger" class={className} title={detail ?? 'The restart failed'}>Restart failed</Chip>
    {#if sentence && detail}
      <span class="min-w-0 truncate font-sans text-[13px] leading-5 text-danger" title={detail}>{detail}</span>
    {/if}
  {/if}
{/if}
