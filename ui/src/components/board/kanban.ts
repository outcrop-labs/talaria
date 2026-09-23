// Shared bits of the kanban split (Kanban.svelte + KanbanCard.svelte).
// Status colours used to live here, as a second `COL_ACCENT` map; the palette
// is `STATUS_COLOR` in `@/lib/statuses` — one map for every surface.

/** Compact "4h" / "2.5h" — estimates render short or not at all. Coerces
 *  defensively: numeric columns can surface as strings at the API edge. */
export const fmtHours = (h: number | string) => {
  const n = Number(h)
  if (!Number.isFinite(n)) return null
  return `${Number.isInteger(n) ? n : n.toFixed(1)}h`
}
