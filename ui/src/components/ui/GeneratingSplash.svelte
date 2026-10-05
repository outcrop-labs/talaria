<script lang="ts">
  import { cn } from '@/lib/cn'
  import DitherLayer from './DitherLayer.svelte'
  import WaitingMark from './WaitingMark.svelte'
  import { splashLabel, splashSources } from './generating-splash'
  import type { WaitingSiteKey } from '@/lib/waiting/sites'

  // The SPLASH form of the wait: when generating is what the WHOLE modal
  // body is doing, the inline <Generating> block reads as one row of a
  // form that is still looking at you. This fills the body with the same
  // material — lull + two crossing waves (generating-splash.ts) — at panel
  // scale, with the status line centred in it.
  //
  // The honesty rules are the block's, unchanged: no percentage, no sweep
  // toward an end. Drift says working; it never says how far along. The
  // elapsed count (appended by `splashLabel` after 10s) is the one number
  // the surface can actually measure — the difference between "working,
  // slowly" and "wedged".
  //
  // Reduced motion is the engine's business, not this component's:
  // DitherLayer stops driving waves and shimmer under
  // prefers-reduced-motion, leaving the splash textured and still, and
  // WaitingMark's rotation is likewise dealt by the waiting system.
  let {
    label,
    seconds = 0,
    site = 'fleet/agent-design',
    class: className,
  }: {
    label: string
    /** Elapsed seconds, from the caller's tick — drives the honest count. */
    seconds?: number
    /** Which waiting mark the status line draws. See lib/waiting/sites.ts. */
    site?: WaitingSiteKey
    class?: string
  } = $props()

  const text = $derived(splashLabel(label, seconds))
</script>

<!-- Full-panel and tall: this owns the modal body's height for the whole
     generation, so min-h floors it well past a status line's own height —
     the same reason EmptyState's `full` variant carries one. grid (not
     flex) so the DitherLayer canvas below the centre line stays symmetric
     about the status row. -->
<div class={cn('relative flex min-h-72 items-center justify-center overflow-hidden rounded-lg', className)}>
  <DitherLayer sources={splashSources} alphaFloor={0.03} maxAlpha={0.32} />
  <div class="relative flex items-center gap-2 px-4 font-sans text-sm text-muted">
    <WaitingMark {site} class="text-accent" />
    <span role="status">{text}</span>
  </div>
</div>
