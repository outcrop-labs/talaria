// Checkbox / Radio / Toggle — the accent-styled selection controls. Native
// inputs underneath (keyboard + a11y for free), warm-gold accent on top; the
// label is part of the control so the hit target is the whole row.
// Shared bits live here; the components are Checkbox/Radio/Toggle.svelte.
import type { Snippet } from 'svelte'

// Spec §8 focus-visible: solid gold ring on controls.
export const focusRing = 'focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2'

export interface BaseProps {
  checked: boolean
  onChange: (checked: boolean) => void
  label?: string | Snippet
  disabled?: boolean
  title?: string
  class?: string
}
