// Shared option shape for <ComposerPicker> (see ComposerPicker.svelte).

export interface ComposerOption {
  value: string
  label: string
  /** Right-aligned mono meta in the popover (tagline, role, timing). */
  sub?: string
}
