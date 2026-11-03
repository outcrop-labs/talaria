import { inDesktopShell } from './desktop-shell'
import { desktopSettings, desktopWindow } from './desktop-settings.svelte'

export type ControlsSide = 'left' | 'right'

/** Where the window controls sit: left on macOS, right everywhere else. */
export const controlsSide = (): ControlsSide =>
  /Mac|iPhone|iPad/.test(navigator.userAgent) ? 'left' : 'right'

export const themedTitlebar = (): boolean =>
  inDesktopShell() && desktopSettings().titlebar === 'themed'

const CONTROLS = 'a, button, input, textarea, select'

type BarEvent = Pick<MouseEvent, 'button'> & { target: EventTarget | null }

/** A primary press on the bar itself, not on a control inside it. */
export const onDragSurface = (e: BarEvent): boolean =>
  e.button === 0 && !(e.target as Element | null)?.closest(CONTROLS)

/** Inert unless the titlebar is themed, checked per event: the mode can change while the bar is mounted. */
export function windowDrag(node: HTMLElement) {
  if (!inDesktopShell()) return
  const down = (e: MouseEvent) => {
    if (!themedTitlebar() || !onDragSurface(e)) return
    e.preventDefault()
    desktopWindow('startDragging')
  }
  const double = (e: MouseEvent) => {
    if (!themedTitlebar() || !onDragSurface(e)) return
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
