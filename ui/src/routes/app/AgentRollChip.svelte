<script lang="ts">
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { ROLL_STILL_STARTING, type FleetRollRow } from '@/lib/fleet-defs'

  // One agent's place in a roll, as the roster shows it: the same outline
  // chip idiom as AgentDraftChip and AgentBrainChip (spec §8). Rolling is a
  // live state and carries the activity mark; done is a quiet word; failed
  // is the one that must say WHY, so the failure sentence rides the title
  // and, where the frame has the room (`sentence`), is written out.
  let { roll, sentence = false }: { roll?: FleetRollRow; sentence?: boolean } = $props()

  const state = $derived(roll?.state ?? 'idle')
  const detail = $derived(roll?.detail ?? null)
  const queued = $derived(state === 'rolling' && detail === 'queued')
  // Recreated, and slower to report healthy than the roll waits: the row
  // stays rolling with this note until the container is up (done) or the
  // late wait gives up (failed) — never a lasting "failed" for an agent that
  // then comes up (talaria-fleet-reconcile STILL_STARTING). Its roll's steps
  // are over, so the Roll controls are not held for it (rollInProgress).
  const starting = $derived(state === 'rolling' && detail === ROLL_STILL_STARTING)
</script>

{#if roll && state !== 'idle'}
  {#if state === 'rolling'}
    <span
      class="inline-flex shrink-0 items-center gap-1 rounded border border-line px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted"
      title={queued
        ? 'Queued: rolls when a slot frees up'
        : starting
          ? 'Still starting: the fresh container is up but has not reported healthy yet, past the roll’s three-minute wait. The roll finishes when it does.'
          : 'Rolling: the container is restarting on the current render, offline for about a minute'}
    >
      <WaitingMark site="fleet/agent-controls" size={9} class="text-accent" />
      {queued ? 'queued' : starting ? 'starting' : 'rolling'}
    </span>
  {:else if state === 'done'}
    <span class="shrink-0 rounded border border-success/40 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-success" title="Rolled: the fresh container took over">
      rolled
    </span>
  {:else}
    <span class="shrink-0 rounded border border-danger/40 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-danger" title={detail ?? 'The roll failed'}>
      roll failed
    </span>
    {#if sentence && detail}
      <span class="min-w-0 truncate font-sans text-xs text-danger" title={detail}>{detail}</span>
    {/if}
  {/if}
{/if}
