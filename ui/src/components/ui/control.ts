// The one control-size scale. Every same-row form control (Button, Input,
// Select, Combobox) draws its height from here so rows always line up —
// never hand-set h-* on a control, set size="sm" | "md".
export type ControlSize = 'sm' | 'md'

export const controlSizes: Record<ControlSize, string> = {
  sm: 'h-9 text-sm',
  md: 'h-11',
}

// The one key-affordance vocabulary (#49). Every single-line control pairs
// with one of these instead of hand-rolling `e.key === …` at the call site.

/** Field-with-a-button: Enter presses the button. */
export const submitOnEnter =
  (fn: () => void) =>
  (e: React.KeyboardEvent): void => {
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
  (e: React.KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>): void => {
    if (e.key === 'Enter') e.currentTarget.blur()
    else if (e.key === 'Escape') {
      e.stopPropagation()
      cancel()
    }
  }
