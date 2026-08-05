// Shared item type for Tabs.svelte (importers pick it up from here so the
// component file stays purely a component).
import type { Snippet } from 'svelte'

export interface TabItem<T extends string = string> {
  id: T
  label: string | Snippet
}
