<script lang="ts" module>
  import type { HTMLAttributes } from 'svelte/elements'

  export type PanelProps = HTMLAttributes<HTMLElement> & {
    /** Semantic element — `section` for page sections, default `div`. */
    as?: 'div' | 'section' | 'article' | 'aside'
    /**
     * A dithered corridor along the top edge.
     *
     * `selected` marks a card in a set you pick from — it carries the choice
     * by field intensity instead of a heavier border or a fill, so the card
     * does not change weight and the row does not reflow as you click along
     * it. `ambient` is a quieter version for a card that is merely prominent.
     * Default `none`: a surface where everything has texture says the same as
     * one where nothing does.
     */
    field?: 'none' | 'ambient' | 'selected'
  }
</script>

<script lang="ts">
  import { cn } from '@/lib/cn'
  import DitherLayer from './DitherLayer.svelte'
  import type { DitherSource } from '@/lib/dither'

  // The core Mercury surface (spec §8): panel fill on ground, 1px hairline,
  // radius 8 — matte, no glow. Reuse for cards/dialogs — don't re-style.
  //
  // Owns the card padding (p-6) so density stays consistent app-wide — don't
  // hand-set p-* per card; override (e.g. `p-0` for flush tables) only when the
  // content genuinely demands it. Card internals convention: header block mb-4,
  // tiny uppercase labels mb-2, list rows py-3, chip/meta clusters mt-2.5.
  let { class: className, as = 'div', field = 'none', children, ...rest }: PanelProps = $props()

  // A CORRIDOR ALONG THE TOP EDGE, carrying state by intensity.
  //
  // Opt-in, and that is the point: 98 panels light up nothing by default,
  // because a surface where everything has texture says the same as one where
  // nothing does. `selected` is for a card in a set you pick from, where the
  // field replaces a heavier border or a fill — it marks the choice without
  // changing the card's weight, so the row does not reflow as you click along
  // it. `ambient` is for a card that is simply prominent.
  const sources = $derived<DitherSource[]>(
    field === 'none'
      ? []
      : [
          { id: 'base', kind: 'edge', side: 'top', depth: 40, strength: 0.12 },
          ...(field === 'selected'
            ? [{ id: 'mark', kind: 'edge', side: 'top', depth: 44, strength: 0.5, tone: 'accent' } as DitherSource]
            : []),
        ],
  )
</script>

<svelte:element
  this={as}
  class={cn('rounded-lg border border-line bg-panel p-6', field !== 'none' && 'relative overflow-hidden', className)}
  {...rest}
>
  {#if field !== 'none'}<DitherLayer {sources} organic={0.5} />{/if}
  {#if field !== 'none'}
    <!-- The field is decoration behind the content, so the content needs its
         own stacking context to stay above it. -->
    <div class="relative">{@render children?.()}</div>
  {:else}
    {@render children?.()}
  {/if}
</svelte:element>
