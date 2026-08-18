<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import DitherLayer from './DitherLayer.svelte'
  import type { DitherSource } from '@/lib/dither'

  // The one empty/zero state. Centered mark + short line + optional single
  // action. Reuse for every no-data view — don't hand-roll bare "No X yet"
  // strings. `compact` for zero states inside panels/lists (no full-height
  // centering, smaller mark); `inline` for a single quiet line.
  let {
    icon = '◇',
    title,
    hint,
    action,
    variant = 'full',
    vignette,
    class: className,
  }: {
    icon?: string | Snippet
    title: string
    hint?: string
    action?: Snippet
    variant?: 'full' | 'compact' | 'inline'
    /**
     * A dithered vignette behind the words — texture where there is nothing.
     *
     * ON BY DEFAULT FOR `full`, which is the variant that owns a whole pane.
     * A large surface with three lines centred in it reads as a dead void; the
     * field gives it material without faking content. `compact` and `inline`
     * sit inside something that already has texture of its own, so they stay
     * plain and cost nothing.
     *
     * THIS IS THE RESOLVED-EMPTY RENDERING AND ONLY THAT. Empty, broken and
     * loading are three different answers — a failed read renders a
     * QueryError, and a read still in flight renders a skeleton (which now
     * carries its own field, SIGNAL STATIC). Putting a vignette on this one
     * must not make it look like either of those.
     */
    vignette?: boolean
    class?: string
  } = $props()

  const wantsVignette = $derived(vignette ?? variant === 'full')

  // Four edges inward, plus one very slow crest drifting across them.
  //
  // The centre stays clear on purpose: density belongs at the boundary, where
  // it says "this surface is real", and away from the words, which have to
  // stay the most legible thing in the frame.
  //
  // The crest is the daily brief hero's motif — a 320px wavelength at 9px/s,
  // so a band takes the better part of a minute to cross. That is the point:
  // an empty pane should look inhabited rather than animated, and anything
  // fast enough to notice competes with the one sentence in the frame. It also
  // must not read as PROGRESS — nothing is loading here, this is the resolved
  // answer — which is why it drifts sideways forever instead of sweeping.
  //
  // Under reduced motion the engine stops driving waves and the field settles
  // at its current phase: still textured, just still.
  const sources: DitherSource[] = [
    { id: 'n', kind: 'edge', side: 'top', depth: 60, strength: 0.28 },
    { id: 's', kind: 'edge', side: 'bottom', depth: 60, strength: 0.28 },
    { id: 'w', kind: 'edge', side: 'left', depth: 80, strength: 0.2 },
    { id: 'e', kind: 'edge', side: 'right', depth: 80, strength: 0.2 },
    { id: 'drift', kind: 'wave', axis: 'x', wavelength: 320, speed: 9, strength: 0.22 },
  ]
</script>

<!-- Spec §2: empty-state copy is reading voice — sans, never the mono chrome
     voice the app shell inherits from the base font. -->
{#if variant === 'inline'}
  <div class={cn('font-sans text-xs text-muted', className)}>
    {title}{hint ? ` — ${hint}` : ''}
  </div>
{:else}
  <!-- EDGE TO EDGE. The padding moved OFF this box and onto the content
       inside it, because the vignette fills this box — and any padding here is
       a band of container the treatment cannot reach, so the zero state reads
       as a textured card floating inside an untextured one. The words are
       still inset by exactly as much as before; it is the field that grew. -->
  <div
    class={cn(
      // `h-full` is `height: 100%`, which resolves against the parent ONLY if
      // the parent has a definite height — otherwise it computes to `auto` and
      // the zero state collapses to the height of its own three lines, taking
      // the vignette with it. That is most containers: a Panel, a branch of an
      // `{#if}` inside an auto-height column. `min-h-48` is the floor that
      // makes `full` read as a region it owns wherever it lands, and `h-full`
      // still fills a container that does have a height.
      variant === 'full' ? 'grid h-full min-h-48 place-items-center' : '',
      'text-center',
      wantsVignette && 'relative overflow-hidden',
      className,
    )}
  >
    {#if wantsVignette}<DitherLayer {sources} organic={0.55} alphaFloor={0.04} maxAlpha={0.38} />{/if}
    <div
      class={cn(
        'relative max-w-xs',
        variant === 'full' ? 'p-6' : 'px-2 py-6',
        variant === 'compact' && 'mx-auto',
      )}
    >
      <div class={cn('mx-auto text-ink-dim', variant === 'full' ? 'mb-3 text-3xl' : 'mb-2 text-xl')}>
        {#if typeof icon === 'string'}{icon}{:else}{@render icon()}{/if}
      </div>
      <div class={cn('font-sans font-medium text-fg', variant === 'full' ? 'text-sm' : 'text-xs')}>{title}</div>
      {#if hint}<div class="mt-1 font-sans text-xs leading-5 text-muted">{hint}</div>{/if}
      {#if action}<div class={variant === 'full' ? 'mt-4' : 'mt-3'}>{@render action()}</div>{/if}
    </div>
  </div>
{/if}
