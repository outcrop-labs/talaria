// THE CONTEXT MENU'S KEYBOARD, as data: one key in, one decision out. The
// menu component (ContextMenu.svelte) owns focus and the DOM; what a key
// MEANS lives here, pure, so it is tested without a browser. The WAI-ARIA
// menu pattern:
//
//   ArrowDown / ArrowUp   next / previous item, wrapping; from nothing, the
//                         first / last
//   Home / End            first / last
//   Enter / Space         activate the active item; on a parent, open its
//                         submenu
//   ArrowRight            open the active item's submenu (its first item
//                         takes focus)
//   ArrowLeft             in a submenu: back to its parent
//   Escape                in a submenu: back to its parent; else close
//   Tab                   close (focus returns to where the menu came from)
//   a printable letter    the next item whose label starts with it
//
// Disabled items and separators are never part of `labels` — the caller
// passes the selectable items only, so an index here is an index into those.

/** Is this the key that opens a context menu from the keyboard: the Menu
 *  key, or Shift+F10 (the keyboard's right-click on every desktop OS)? */
export function isMenuKey(e: Pick<KeyboardEvent, 'key' | 'shiftKey'>): boolean {
  return e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')
}

export type MenuKeyResult =
  | { kind: 'move'; active: number }
  | { kind: 'activate'; index: number }
  | { kind: 'open-sub'; index: number }
  | { kind: 'close-sub' }
  | { kind: 'close' }
  | { kind: 'none' }

export interface MenuKeyContext {
  /** The active item's index among the selectable items, or -1 for none. */
  active: number
  /** The selectable items' labels, in order. */
  labels: readonly string[]
  /** Whether the item at an index opens a submenu. */
  hasChildren: (index: number) => boolean
  /** True while the keys drive a submenu rather than the top level. */
  inSub: boolean
}

export function menuKeyStep(key: string, ctx: MenuKeyContext): MenuKeyResult {
  const n = ctx.labels.length
  switch (key) {
    case 'ArrowDown':
      return n ? { kind: 'move', active: ctx.active < 0 ? 0 : (ctx.active + 1) % n } : { kind: 'none' }
    case 'ArrowUp':
      return n ? { kind: 'move', active: ctx.active < 0 ? n - 1 : (ctx.active - 1 + n) % n } : { kind: 'none' }
    case 'Home':
      return n ? { kind: 'move', active: 0 } : { kind: 'none' }
    case 'End':
      return n ? { kind: 'move', active: n - 1 } : { kind: 'none' }
    case 'Enter':
    case ' ':
      if (ctx.active < 0) return { kind: 'none' }
      return ctx.hasChildren(ctx.active) ? { kind: 'open-sub', index: ctx.active } : { kind: 'activate', index: ctx.active }
    case 'ArrowRight':
      return ctx.active >= 0 && ctx.hasChildren(ctx.active) ? { kind: 'open-sub', index: ctx.active } : { kind: 'none' }
    case 'ArrowLeft':
      return ctx.inSub ? { kind: 'close-sub' } : { kind: 'none' }
    case 'Escape':
      return ctx.inSub ? { kind: 'close-sub' } : { kind: 'close' }
    case 'Tab':
      return { kind: 'close' }
    default:
      return typeahead(key, ctx)
  }
}

/** A single printable character jumps to the next item (after the active
 *  one, wrapping) whose label starts with it, case-insensitively. */
function typeahead(key: string, ctx: MenuKeyContext): MenuKeyResult {
  if ([...key].length !== 1 || key === ' ') return { kind: 'none' }
  const ch = key.toLocaleLowerCase()
  const n = ctx.labels.length
  for (let step = 1; step <= n; step++) {
    const i = (Math.max(ctx.active, -1) + step + n) % n
    if (ctx.labels[i]?.trimStart().toLocaleLowerCase().startsWith(ch)) return { kind: 'move', active: i }
  }
  return { kind: 'none' }
}

/** Where a menu panel goes so it stays inside the viewport. `anchor` (a
 *  keyboard or button open) puts it under the element, or above it when the
 *  room below is short; a point (a right-click, a long-press) puts its corner
 *  at the pointer and flips it left/up when it would cross an edge. Either
 *  way it is finally clamped `margin` inside every edge. Pure, for tests. */
export function placeMenu(
  size: { width: number; height: number },
  viewport: { width: number; height: number },
  at: { x: number; y: number; anchor?: { top: number; bottom: number; left: number; right: number } },
  margin = 8,
): { left: number; top: number } {
  let left: number
  let top: number
  if (at.anchor) {
    const a = at.anchor
    left = a.left
    top = a.bottom + 4
    if (top + size.height > viewport.height - margin && a.top - 4 - size.height >= margin) top = a.top - 4 - size.height
  } else {
    left = at.x + size.width > viewport.width - margin ? at.x - size.width : at.x
    top = at.y + size.height > viewport.height - margin ? at.y - size.height : at.y
  }
  left = Math.min(left, viewport.width - margin - size.width)
  top = Math.min(top, viewport.height - margin - size.height)
  return { left: Math.max(margin, left), top: Math.max(margin, top) }
}

/** Which side a submenu opens on: the right of its parent row unless that
 *  would cross the viewport's right edge and the left has more room. */
export function submenuSide(
  parent: { left: number; right: number },
  subWidth: number,
  viewportWidth: number,
  margin = 8,
): 'right' | 'left' {
  if (parent.right + subWidth <= viewportWidth - margin) return 'right'
  return parent.left - subWidth >= margin || parent.left > viewportWidth - parent.right ? 'left' : 'right'
}
