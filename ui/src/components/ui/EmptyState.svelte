<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { useField } from '@/lib/field-registry.svelte'
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

  let el = $state<HTMLElement | null>(null)

  // A VIGNETTE AND NOTHING ELSE — four edges inward, no travelling source.
  //
  // The centre stays clear on purpose: density belongs at the boundary, where
  // it says "this surface is real", and away from the words, which have to
  // stay the most legible thing in the frame.
  //
  // A drifting crest was tried here for the movement and it was wrong: a wave
  // is a SHAPE, so it necessarily paints bands across the middle of the pane,
  // and bands in an empty pane read as content. The vignette has to stay a
  // vignette.
  //
  // THE MOVEMENT IS THE CLUMP MORPH NOW, not the old per-cell `shimmer`. The
  // surface morphs its clump field continuously at 0.08Hz, which is shapeless
  // in exactly the way shimmer was and costs nothing extra here — the vignette
  // breathes because the whole field does. Reduced motion stops it and leaves
  // the texture, which is the right half to keep.
  const SOURCES: DitherSource[] = [
    { id: 'n', kind: 'edge', side: 'top', depth: 60, strength: 0.28, gain: 0.45 },
    { id: 's', kind: 'edge', side: 'bottom', depth: 60, strength: 0.28, gain: 0.45 },
    { id: 'w', kind: 'edge', side: 'left', depth: 80, strength: 0.2, gain: 0.45 },
    { id: 'e', kind: 'edge', side: 'right', depth: 80, strength: 0.2, gain: 0.45 },
  ]

  useField(
    () => el,
    (): DitherSource[] => (wantsVignette ? SOURCES : []),
  )
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
    bind:this={el}
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
