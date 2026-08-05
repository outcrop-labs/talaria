// Shared option type for Segmented.svelte (importers pick it up from here so
// the component file stays purely a component).
import type { Snippet } from 'svelte'

export interface SegmentedOption<T extends string = string> {
  id: T
  label: string | Snippet
  title?: string
}
