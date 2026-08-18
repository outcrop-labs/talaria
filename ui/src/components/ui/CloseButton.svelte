<script lang="ts">
  import { X } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusRing } from './control'

  let {
    onClick,
    class: className,
    size = 16,
    label = 'Close',
  }: {
    onClick: () => void
    class?: string
    size?: number
    label?: string
  } = $props()
</script>

<!-- The one close (✕) button. Padded hit area so you don't have to click
     dead-on the glyph. Reuse for modals, panels, dismissible surfaces. -->
<button
  type="button"
  onclick={onClick}
  aria-label={label}
  title={label}
  class={cn(
    // Ghost icon affordance (spec §8): muted → readout, hover-token fill,
    // radius 6, gold focus ring.
    'grid place-items-center rounded-md p-1.5 text-muted transition-colors hover:dither-fill hover:text-fg',
    focusRing,
    className,
  )}
>
  <X {size} strokeWidth={2} />
</button>
