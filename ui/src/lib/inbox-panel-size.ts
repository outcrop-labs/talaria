export const DEFAULT_INBOX_PANEL_WIDTH = 520
export const MIN_INBOX_PANEL_WIDTH = 320
export const MAX_INBOX_PANEL_WIDTH = 720

export function clampInboxPanelWidth(width: number): number {
  if (!Number.isFinite(width)) return DEFAULT_INBOX_PANEL_WIDTH
  return Math.min(MAX_INBOX_PANEL_WIDTH, Math.max(MIN_INBOX_PANEL_WIDTH, Math.round(width)))
}

export function shouldCollapseInboxPanel(width: number): boolean {
  return width < MIN_INBOX_PANEL_WIDTH
}
