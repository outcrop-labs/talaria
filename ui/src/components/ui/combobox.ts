// Shared option type for Combobox.svelte (importers pick it up from here so
// the component file stays purely a component).
import type { Snippet } from 'svelte'

export interface ComboOption {
  value: string
  label: string
  sub?: string
  /** Optional leading mark (e.g. a provider logo). */
  icon?: Snippet
}

/** Subsequence fuzzy match: chars of `q` appear in order in `text`. */
export function fuzzy(q: string, text: string): boolean {
  if (!q) return true
  const s = text.toLowerCase()
  let i = 0
  for (const ch of q.toLowerCase()) {
    i = s.indexOf(ch, i)
    if (i === -1) return false
    i++
  }
  return true
}
