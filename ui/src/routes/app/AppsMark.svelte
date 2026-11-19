<script lang="ts">
  import { cn } from '@/lib/cn'

  // An app's mark: the one glyph its manifest names. Every mark is drawn at
  // one size and without colour, so a column of them reads as a set even when
  // one manifest picked an emoji and the next a box-drawing character.
  let {
    icon,
    tile = false,
    class: className,
  }: {
    icon: string
    /** Draw the 32px tile too. Leave off inside a `SettingsRow` icon slot,
     *  which supplies its own. */
    tile?: boolean
    class?: string
  } = $props()
</script>

{#if tile}
  <span class={cn('grid h-8 w-8 shrink-0 place-items-center rounded-md border border-line bg-raised text-fg', className)}>
    <span aria-hidden="true" class="text-base leading-none grayscale">{icon}</span>
  </span>
{:else}
  <span aria-hidden="true" class={cn('text-base leading-none text-fg grayscale', className)}>{icon}</span>
{/if}
