<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import IconButton from './IconButton.svelte'
  import { COG, SettingsGlyph, type CogSize } from './settings-cog'

  // The icon-only door into a settings surface: the one gear, on IconButton's
  // tile. It is never silent and never resized at the call site:
  //   · `label` is WHICH settings ("Board settings", never a bare "Settings").
  //     It is the tooltip AND the accessible name, and its type refuses a
  //     label that does not end in " settings".
  //   · no `class` prop: the target is IconButton's sm (28px) or md (32px)
  //     tile and the glyph is half of it, so no call site stretches, clips or
  //     shrinks it below the 24px floor (WCAG 2.5.8).
  //   · `focus:text-fg`: the dither field behind the tile lights on focusin as
  //     well as on hover (lib/dither-surface.ts), so a focused gear brightens
  //     with it rather than sitting muted on a lit field.
  // A popover or menu trigger passes `active={open} aria-expanded={open}`; a
  // dialog opener passes `aria-haspopup="dialog"`.
  interface Props extends Omit<HTMLButtonAttributes, 'title' | 'class' | 'children'> {
    /** WHICH settings — "Board settings", "Column settings". Tooltip and accessible name. */
    label: `${string} settings`
    size?: CogSize
    /** The surface it opens is open (a popover or menu trigger). */
    active?: boolean
    ref?: HTMLButtonElement | null
  }

  let { label, size = 'sm', active, ref = $bindable(null), ...rest }: Props = $props()
</script>

<IconButton bind:ref title={label} {size} {active} class="focus:text-fg" data-settings-door {...rest}>
  <SettingsGlyph size={COG[size]} />
</IconButton>
