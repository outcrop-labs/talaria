// Shared bits of the kanban split (Kanban.svelte + KanbanCard.svelte).

export const COL_ACCENT: Record<string, string> = {
  inbox: 'var(--theme-muted)',
  assigned: 'var(--theme-accent)',
  in_progress: 'var(--theme-warning)',
  blocked: 'var(--theme-danger)',
  quality_review: 'var(--theme-accent-secondary)',
  done: 'var(--theme-success)',
}

/** Compact "4h" / "2.5h" — estimates render short or not at all. Coerces
 *  defensively: numeric columns can surface as strings at the API edge. */
export const fmtHours = (h: number | string) => {
  const n = Number(h)
  if (!Number.isFinite(n)) return null
  return `${Number.isInteger(n) ? n : n.toFixed(1)}h`
}
