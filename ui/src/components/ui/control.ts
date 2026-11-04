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
// opacity in both themes. Button's non-fill variants draw it from here, and
// the field primitives and the chat chrome's rail chips (`chat-chrome`) through
// `disabledControl` below, so a disabled field or chip reads the way a
// disabled button does. The primary gold fill must NOT use it:
// dimming the fill and its surface-toned label together leaves a ~1.5:1
// washed-gold/near-white pair in light mode.
export const dimDisabled = 'disabled:opacity-50'

// The disabled reading for a control the pointer still reaches: the dim
// above, plus a not-allowed cursor. Input, Textarea, Select, Combobox's
// trigger and the chat chrome's rail chips take it. Without it a disabled
// field drew exactly like an editable one: the field primitives had no
// disabled state at all, so a viewer's task title, a locked setting, or a
// field held during a save all looked like they took typing. Button keeps
// `dimDisabled` alone, because it also sets `pointer-events-none`.
export const disabledControl = `${dimDisabled} disabled:cursor-not-allowed`

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
