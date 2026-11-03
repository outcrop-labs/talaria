import { inDesktopShell } from './desktop-shell'
import { desktopSettings, desktopWindow } from './desktop-settings.svelte'

export type ControlsSide = 'left' | 'right'

/** Where the window controls sit: left on macOS, right everywhere else. */
export const controlsSide = (): ControlsSide =>
  /Mac|iPhone|iPad/.test(navigator.userAgent) ? 'left' : 'right'

export const themedTitlebar = (): boolean =>
  inDesktopShell() && desktopSettings().titlebar === 'themed'

const CONTROLS = 'a, button, input, textarea, select'

// A press drags only when nothing between it and the bar is a control or a
// layer that hangs out of the bar's box, such as a dropdown panel rendered
// inside the bar rather than portaled out of it.
const onDragSurface = (e: MouseEvent, bar: HTMLElement): boolean => {
  if (e.button !== 0) return false
  const { top, bottom } = bar.getBoundingClientRect()
  for (let el = e.target as Element | null; el && el !== bar; el = el.parentElement) {
    if (el.matches(CONTROLS)) return false
    const r = el.getBoundingClientRect()
    if (r.top < top - 0.5 || r.bottom > bottom + 0.5) return false
  }
  return true
}

/** Inert unless the titlebar is themed, checked per event: the mode can change while the bar is mounted. */
export function windowDrag(node: HTMLElement) {
  if (!inDesktopShell()) return
  const down = (e: MouseEvent) => {
    if (!themedTitlebar() || !onDragSurface(e, node)) return
    e.preventDefault()
    desktopWindow('startDragging')
  }
  const double = (e: MouseEvent) => {
    if (!themedTitlebar() || !onDragSurface(e, node)) return
    desktopWindow('toggleMaximize')
  }
  node.addEventListener('mousedown', down)
  node.addEventListener('dblclick', double)
  return {
    destroy() {
      node.removeEventListener('mousedown', down)
      node.removeEventListener('dblclick', double)
    },
  }
}
