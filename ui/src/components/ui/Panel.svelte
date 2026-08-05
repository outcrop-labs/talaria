<script lang="ts" module>
  import type { HTMLAttributes } from 'svelte/elements'

  export type PanelProps = HTMLAttributes<HTMLElement> & {
    /** Semantic element — `section` for page sections, default `div`. */
    as?: 'div' | 'section' | 'article' | 'aside'
  }
</script>

<script lang="ts">
  import { cn } from '@/lib/cn'

  // The core Mercury surface (spec §8): panel fill on ground, 1px hairline,
  // radius 8 — matte, no glow. Reuse for cards/dialogs — don't re-style.
  //
  // Owns the card padding (p-6) so density stays consistent app-wide — don't
  // hand-set p-* per card; override (e.g. `p-0` for flush tables) only when the
  // content genuinely demands it. Card internals convention: header block mb-4,
  // tiny uppercase labels mb-2, list rows py-3, chip/meta clusters mt-2.5.
  let { class: className, as = 'div', children, ...rest }: PanelProps = $props()
</script>

<svelte:element this={as} class={cn('rounded-lg border border-line bg-panel p-6', className)} {...rest}>
  {@render children?.()}
</svelte:element>
