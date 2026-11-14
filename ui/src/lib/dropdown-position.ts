/** Horizontal placement for a portaled dropdown. Keeps the panel inside the
 *  viewport: a left-edge trigger with `align: 'right'` would otherwise paint
 *  off-screen to the left (the desktop instance switcher in the nav rail). */
export function dropdownHorizStyle(
  align: 'left' | 'right',
  trigger: { left: number; right: number },
  viewportWidth: number,
  panelWidth: number,
  margin = 8,
): string {
  const maxLeft = Math.max(margin, viewportWidth - panelWidth - margin)
  if (align === 'right') {
    const right = Math.max(margin, viewportWidth - trigger.right)
    const leftEdge = viewportWidth - right - panelWidth
    if (leftEdge < margin) return `left: ${Math.min(Math.max(margin, trigger.left), maxLeft)}px`
    return `right: ${right}px`
  }
  const left = Math.max(margin, trigger.left)
  if (left + panelWidth > viewportWidth - margin) {
    const right = Math.max(margin, viewportWidth - trigger.right)
    const leftEdge = viewportWidth - right - panelWidth
    if (leftEdge >= margin) return `right: ${right}px`
    return `left: ${maxLeft}px`
  }
  return `left: ${left}px`
}

/** `max-w-72` (18rem) at the default 16px root — the dropdown panel's ceiling. */
export const DROPDOWN_PANEL_MAX_WIDTH_PX = 288

/** Where an anchored panel's trigger sat when the panel opened. */
export interface AnchorSpot {
  top: number
  left: number
}

/** Whether the trigger has moved from where it sat at open. An anchored panel
 *  closes on scroll because it cannot follow its trigger, so a scroll that
 *  leaves the trigger in place is no reason to close. That is the scroll event
 *  for a move that finished just before the click: it is dispatched a frame
 *  later, after the panel is already open. */
export function anchorMoved(now: AnchorSpot | undefined, atOpen: AnchorSpot | null): boolean {
  if (!now || !atOpen) return true
  return Math.abs(now.top - atOpen.top) >= 1 || Math.abs(now.left - atOpen.left) >= 1
}
