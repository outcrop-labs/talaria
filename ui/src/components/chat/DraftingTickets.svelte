<script lang="ts">
  // The plan modal's drafting state, in the surface's own language: the agent
  // is PRINTING TICKETS. Ghost cards in ProposalCard's image (checkbox, title
  // pill, priority chips, description lines, blocked-by chips) emerge from a
  // slot and ride down — and each card is drawn ENTIRELY in dither (see
  // TicketGhost: the dots are the paths), with crests travelling through the
  // geometry so the lit pattern shifts along the bars while it prints.
  //
  // Honesty rules inherited from <Generating>: no percentage, no sweep that
  // promises an end — a press that keeps printing until the real cards land
  // and replace it. Reduced motion parks the conveyor and the engine settles
  // the crests, leaving tickets that are still dither, just still.
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import TicketGhost, { type GhostSpec } from './TicketGhost.svelte'

  /** Whose tickets these will be — "{label} is reading…". */
  let { label }: { label: string } = $props()

  // Three silhouettes so the loop doesn't visibly repeat: same card grammar,
  // different proportions per ghost.
  const SPECS: GhostSpec[] = [
    { title: 'w-2/5', lines: ['w-full', 'w-11/12', 'w-3/5'], chips: ['w-14', 'w-10'] },
    { title: 'w-1/2', lines: ['w-full', 'w-4/5'], chips: ['w-12'] },
    { title: 'w-1/3', lines: ['w-full', 'w-5/6', 'w-2/3'], chips: ['w-14', 'w-9', 'w-8'] },
  ]
</script>

<div class="space-y-4">
  <div class="flex items-center gap-2 text-sm text-muted">
    <WaitingMark site="plan/draft" class="text-accent" />
    <span class="font-medium text-fg">{label}</span> is reading the conversation and drafting tickets
  </div>

  <!-- The press: slot above, tray below. The tray itself paints nothing —
       every mark in it belongs to a card. -->
  <div aria-hidden="true">
    <div class="h-1.5 rounded-full bg-line-strong shadow-[inset_0_1px_2px_rgba(0,0,0,0.35)]"></div>
    <div class="relative h-80 overflow-hidden [mask-image:linear-gradient(to_bottom,black_84%,transparent)]">
      <div class="conveyor space-y-3">
        {#each [...SPECS, ...SPECS] as g, i (i)}
          <TicketGhost spec={g} seed={i % SPECS.length} />
        {/each}
      </div>
    </div>
  </div>

  <p class="text-center font-sans text-xs text-muted">
    You can close this and keep working; drafts stay paired to the plan.
  </p>
</div>

<style>
  /* Seamless loop: the ghosts and their duplicates make a strip exactly twice
     the period, so sliding one period and snapping back is invisible. DOWN,
     because that is the direction a press prints. Period = 3 × (h-32 + gap-3)
     = 3 × 140px. */
  .conveyor {
    animation: press 7.5s linear infinite;
  }
  @keyframes press {
    from {
      transform: translateY(-420px);
    }
    to {
      transform: translateY(0);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .conveyor {
      animation: none;
      transform: translateY(-140px);
    }
  }
</style>
