// The one control-size scale. Every same-row form control (Button, Input,
// Select, Combobox) draws its height from here so rows always line up —
// never hand-set h-* on a control, set size="sm" | "md".
export type ControlSize = 'sm' | 'md'

export const controlSizes: Record<ControlSize, string> = {
  sm: 'h-9 text-sm',
  md: 'h-11',
}
