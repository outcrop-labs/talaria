// Where a token was served, as the Cost tab's charts colour it. One table, so
// a class keeps its colour from the split bar to the daily chart to the model
// rows. Two chart tokens for the two real classes, a hairline grey for the
// remainder nobody attributed.
export type ServedBy = 'local' | 'cloud' | 'other'

export const SERIES: Record<ServedBy, { label: string; color: string }> = {
  local: { label: 'Self-hosted', color: 'var(--theme-chart-2)' },
  cloud: { label: 'Cloud', color: 'var(--theme-chart-1)' },
  other: { label: 'Unattributed', color: 'var(--theme-border-strong)' },
}

export const SERVED_ORDER: readonly ServedBy[] = ['local', 'cloud', 'other']
