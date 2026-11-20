// Where a token was served, as every chart and bar on Cost, Compute and the
// Overview colours it. One table, so a class keeps its colour from the split
// bar to the daily chart to the model rows. The two real classes differ in
// lightness as well as hue, so they still separate without colour, and
// neither sits near the danger orange. Cloud is the chart hue drawn back
// toward the panel, so the one hue from outside the palette sits quietly
// beside the gold. The remainder nobody attributed is a hairline grey.
export type ServedBy = 'local' | 'cloud' | 'other'

export const SERIES: Record<ServedBy, { label: string; color: string }> = {
  local: { label: 'Self-hosted', color: 'var(--theme-text)' },
  cloud: { label: 'Cloud', color: 'color-mix(in srgb, var(--theme-chart-1) 80%, var(--theme-panel))' },
  other: { label: 'Unattributed', color: 'var(--theme-border-strong)' },
}

export const SERVED_ORDER: readonly ServedBy[] = ['local', 'cloud', 'other']
