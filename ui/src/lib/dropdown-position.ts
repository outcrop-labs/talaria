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
