// The one control-size scale. Every same-row form control (Button, Input,
// Select, Combobox) draws its height from here so rows always line up —
// never hand-set h-* on a control, set size="sm" | "md".
export type ControlSize = 'sm' | 'md'

export const controlSizes: Record<ControlSize, string> = {
  sm: 'h-9 text-sm',
  md: 'h-11',
}

// Mercury focus-visible (spec §8): gold — SOLID 2px ring on controls
// (buttons, icon tiles, segmented cells, inputs). Chips use the dashed
// variant from `chat-chrome` (`focusGold`) instead.
export const focusRing =
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent'

// The one disabled reading for a control that is not a gold fill: dim the
// whole control. Text on a transparent or raised ground stays legible at half
// opacity in both themes. Button's non-fill variants and the chat chrome's
// rail chips (`chat-chrome`) both draw it from here, so a disabled chip reads
// the way a disabled button does. The primary gold fill must NOT use it:
// dimming the fill and its surface-toned label together leaves a ~1.5:1
// washed-gold/near-white pair in light mode.
export const dimDisabled = 'disabled:opacity-50'

// The one key-affordance vocabulary (#49). Every single-line control pairs
// with one of these instead of hand-rolling `e.key === …` at the call site.

/** Field-with-a-button: Enter presses the button. */
export const submitOnEnter =
  (fn: () => void) =>
  (e: KeyboardEvent): void => {
    if (e.key !== 'Enter') return
    e.preventDefault()
    fn()
  }

/** Inline rename/edit that commits on blur: Enter commits (via blur),
 *  Escape reverts without committing — and stays INSIDE the field (shielded
 *  from modal/document Escape handlers, which would close the surface out
 *  from under the edit). `cancel` must reset state so a later blur sees
 *  nothing changed to save. */
export const inlineEditKeys =
  (cancel: () => void) =>
  (e: KeyboardEvent): void => {
    if (e.key === 'Enter') (e.currentTarget as HTMLInputElement | HTMLTextAreaElement).blur()
    else if (e.key === 'Escape') {
      e.stopPropagation()
      cancel()
    }
  }
