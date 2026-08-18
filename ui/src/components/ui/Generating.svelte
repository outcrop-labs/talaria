<script lang="ts">
  import { cn } from '@/lib/cn'
  import { useField } from '@/lib/field-registry.svelte'
  import FieldBackdrop from './FieldBackdrop.svelte'
  import type { DitherSource } from '@/lib/dither'
  import GeneratingBars from './GeneratingBars.svelte'
  import WaitingMark from './WaitingMark.svelte'
  import type { GeneratingVariant } from './generating'
  import type { WaitingSiteKey } from '@/lib/waiting/sites'

  // An agent is writing something and the result will replace this block:
  // status line + `lines` rows of line-toned bars sweeping on the chosen
  // motif (default `scan` — the standard agent-stage tier).
  //
  // The BARS stay bars. They are shaped like the text that is coming, which is
  // a promise about the result and not a report on the agent — a waiting mark
  // cannot make it and should not try. What the rotation owns here is the mark
  // on the STATUS LINE, which is the part that says "still working".
  let {
    label,
    lines = 3,
    variant = 'scan',
    site = 'generating/block',
    class: className,
  }: {
    label?: string
    lines?: number
    variant?: GeneratingVariant
    /** Which waiting mark the status line draws. See lib/waiting/sites.ts. */
    site?: WaitingSiteKey
    class?: string
  } = $props()

  const counts = [14, 18, 11, 16, 9, 13]

  // AMBIENT ACTIVITY BEHIND THE BLOCK — two crests drifting against each other
  // at different speeds and wavelengths, so the field never visibly repeats,
  // over a lull at the top edge.
  //
  // Waves and not a fill, because a fill would be a claim. This block appears
  // when an agent is composing something whose length nobody knows, so there
  // is no percentage to report and anything that sweeps toward an end implies
  // a completion the surface cannot promise. Drift says working; it does not
  // say how far along.
  //
  // The BARS still carry the shape of what is coming — that is the promise
  // about the result. This is the room the result will arrive in.
  //
  // Under reduced motion the engine stops driving waves and the field settles,
  // which leaves the block textured and still rather than blank.
  let el = $state<HTMLElement | null>(null)

  const SOURCES: DitherSource[] = [
    { id: 'lull', kind: 'edge', side: 'top', depth: 34, strength: 0.18, gain: 0.4 },
    { id: 'drift-a', kind: 'wave', axis: 'x', wavelength: 150, speed: 26, strength: 0.4, gain: 0.4 },
    { id: 'drift-b', kind: 'wave', axis: 'x', wavelength: 64, speed: -14, strength: 0.22, gain: 0.4 },
  ]

  useField(() => el, () => SOURCES)
</script>

<div bind:this={el} class={cn('relative overflow-hidden rounded-lg border border-line p-4', className)}>
  <FieldBackdrop {el} />
  {#if label}
    <div class="relative flex items-center gap-2 font-sans text-sm text-muted">
      <WaitingMark {site} class="text-accent" />
      <span>{label}</span>
    </div>
  {/if}
  {#if lines > 0}
    <div class={cn('relative space-y-2.5 text-line', label && 'mt-3.5')}>
      {#each Array.from({ length: lines }) as _, i (i)}
        <GeneratingBars bars={counts[i % counts.length]!} {variant} delay={i * 0.15} class="flex" />
      {/each}
    </div>
  {/if}
</div>
