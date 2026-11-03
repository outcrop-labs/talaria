// The themed titlebar's behavior, for whichever bar is the top of the window:
// the tab bar in the app, a plain band on screens without one.
import { inDesktopShell } from './desktop-shell'
import { desktopSettings, desktopWindow } from './desktop-settings.svelte'

export type ControlsSide = 'left' | 'right'

/** Where the window controls sit: left on macOS, right everywhere else. */
export const controlsSide = (): ControlsSide =>
  /Mac|iPhone|iPad/.test(navigator.userAgent) ? 'left' : 'right'

/** The page draws the window chrome: the desktop shell with the themed titlebar. */
export const themedTitlebar = (): boolean =>
  inDesktopShell() && desktopSettings().titlebar === 'themed'

const CONTROLS = 'a, button, input, textarea, select'

type BarEvent = Pick<MouseEvent, 'button'> & { target: EventTarget | null }

/** A primary press on the bar itself, not on a control inside it. */
export const onDragSurface = (e: BarEvent): boolean =>
  e.button === 0 && !(e.target as Element | null)?.closest?.(CONTROLS)

/** Makes `node` the window's drag surface while the titlebar is themed:
 *  press and drag moves the window, a double-click toggles maximize. */
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
