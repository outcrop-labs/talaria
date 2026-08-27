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
  /** Checkbox only: render the bare input, no label element. For cells and
   *  rows where the surrounding markup is already the label (a table row, a
   *  clickable card) and nesting a <label> would either double-handle clicks
   *  or wrap interactive elements. `title` (or aria-label) carries the name. */
  bare?: boolean
  class?: string
}
