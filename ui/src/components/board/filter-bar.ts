// The filter model — the plain-TS half of the filter-bar split, importable by
// routes and views without pulling the component (the bar itself is
// FilterBar.svelte). Values are ORed within a facet, ANDed across facets;
// everything lives in the URL (the route owns encoding).

export interface BoardFilters {
  statuses: string[]
  assignees: string[]
  priorities: string[]
  labels: string[]
  due: '' | 'overdue' | 'today' | 'week' | 'none'
}

export const EMPTY_FILTERS: BoardFilters = { statuses: [], assignees: [], priorities: [], labels: [], due: '' }

export const filtersActive = (f: BoardFilters): boolean =>
  f.statuses.length > 0 || f.assignees.length > 0 || f.priorities.length > 0 || f.labels.length > 0 || f.due !== ''

export const DUE_LABEL: Record<Exclude<BoardFilters['due'], ''>, string> = {
  overdue: 'Overdue',
  today: 'Due today',
  week: 'Due this week',
  none: 'No due date',
}
