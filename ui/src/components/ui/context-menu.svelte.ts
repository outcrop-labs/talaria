import type { Component, Snippet } from 'svelte'
import { copyAppLink } from '@/lib/links'
import { isMenuKey } from './context-menu-keys'

// The one context menu. Right-click a row/card/tile → its actions at the
// cursor. Same shell as dropdown menus (the §7 popover pattern: panel surface,
// hairline, radius 10, matte shadow — `popPanel` from chat-chrome); portaled
// to <body> so backdrop-filter surfaces can't trap the fixed positioning.
// Esc / outside-click / scroll close it.
//
// KEYBOARD AND TOUCH, not only the mouse: `contextMenuTarget` (below) opens a
// row's menu from the Menu key or Shift+F10 and from a long-press, and the
// panel takes focus — arrows, Home/End, typeahead, Right/Left through a
// submenu, Esc and Tab close it and hand focus back (context-menu-keys.ts
// says what each key means). A row also carries a visible "…" button that
// opens the same menu with `openAt(button, items)`, so the actions are found
// without knowing to right-click.
//
// Usage:
//   const menu = useContextMenu()
//   <div oncontextmenu={(e) => menu.openMenu(e, [
//     { label: 'Open', onSelect: ... },
//     'sep',
//     { label: 'Delete', danger: true, onSelect: ... },
//   ])}>…</div>
//   <ContextMenu {menu} />   (ContextMenu.svelte renders the open menu)
//
// A row menu leads with the same two entries everywhere; those have builders,
// so nobody spells them (or the deep link) out again:
//   menu.openMenu(e, [
//     ...openCopyItems(`/boards/${t.boardId}/${t.id}`, () => open(t)),
//     ...copyTextItems(t.description),
//   ])
//
// Items should mirror actions the surface already offers — a context menu is
// a shortcut, never the only home of an action.

/** Item icons are data, not markup: pass a snippet, or a `[Component, props]`
 *  tuple (`[Flag, { size: 12 }]`, `[Avatar, { name, class: 'h-5 w-5' }]`) so
 *  entry arrays can be built in plain TS, outside a template. */
export type MenuIcon = Snippet | [Component<any>, Record<string, unknown>?]

export interface ContextMenuItem {
  label: string
  icon?: MenuIcon
  danger?: boolean
  disabled?: boolean
  /** Marks the current choice in a submenu (renders a leading check). */
  checked?: boolean
  /** A keyboard shortcut, shown at the row's end (`Ctrl+R`). Display only:
   *  whatever binds the key does so itself. */
  hint?: string
  /** Keep the menu open after selecting (multi-toggle pickers). */
  keepOpen?: boolean
  /** Submenu — hover opens a flyout of these entries. `onSelect` is ignored
   *  on items that carry children. */
  children?: ContextMenuEntry[]
  onSelect?: () => void
}
export type ContextMenuEntry = ContextMenuItem | 'sep'

export interface MenuState {
  x: number
  y: number
  items: ContextMenuEntry[]
  /** Opened from the keyboard (the Menu key, Shift+F10, Enter on a "…"
   *  button): the first item takes focus, as the WAI-ARIA menu pattern has
   *  it. A pointer open focuses the panel and highlights nothing. */
  keyboard: boolean
  /** The element the menu was opened against, when it was (a keyboard open,
   *  a "…" button): the panel sits under it, or above it when the room below
   *  is short. Absent, the panel's corner is at (x, y). */
  anchor?: { top: number; bottom: number; left: number; right: number }
  /** Where focus goes back when the menu closes from inside — Escape, Tab,
   *  choosing an item. An outside click keeps the focus it moved. */
  returnFocus: HTMLElement | null
}

export interface OpenOptions {
  /** Focus the first item (see `MenuState.keyboard`). */
  keyboard?: boolean
}

export interface ContextMenuController {
  /** The open menu (position + items), or null when closed. */
  readonly state: MenuState | null
  /** Open from an event: a right-click opens at the pointer; the Menu key,
   *  Shift+F10, or a keyboard-activated click (`detail === 0`) opens against
   *  the event's element with the first item focused. */
  openMenu: (e: MouseEvent | KeyboardEvent, items: ContextMenuEntry[]) => void
  /** Open against an element (a "…" button) or at a point (a long-press). */
  openAt: (target: Element | { x: number; y: number }, items: ContextMenuEntry[], opts?: OpenOptions) => void
  closeMenu: () => void
}

const isKeyboardEvent = (e: Event): e is KeyboardEvent =>
  typeof KeyboardEvent !== 'undefined' && e instanceof KeyboardEvent

export function useContextMenu(): ContextMenuController {
  let state = $state<MenuState | null>(null)

  const show = (next: Omit<MenuState, 'returnFocus' | 'items'>, items: ContextMenuEntry[]) => {
    const real = items.filter((i) => i !== 'sep')
    if (real.length === 0) return
    // The element that had focus BEFORE the menu took it — kept across a
    // re-open from inside the menu's own surface.
    const prior = state?.returnFocus ?? null
    const active = typeof document !== 'undefined' ? (document.activeElement as HTMLElement | null) : null
    // Focus inside an open menu is the menu's, not a place to come back to.
    const own = active && active !== document.body && !active.closest('[role="menu"]')
    const returnFocus = own ? active : prior
    state = { ...next, items, returnFocus }
  }

  const openAt: ContextMenuController['openAt'] = (target, items, opts) => {
    if ('getBoundingClientRect' in target) {
      const r = target.getBoundingClientRect()
      show(
        {
          x: r.left,
          y: r.bottom,
          keyboard: opts?.keyboard ?? false,
          anchor: { top: r.top, bottom: r.bottom, left: r.left, right: r.right },
        },
        items,
      )
    } else {
      show({ x: target.x, y: target.y, keyboard: opts?.keyboard ?? false }, items)
    }
  }

  return {
    get state() {
      return state
    },
    openMenu(e: MouseEvent | KeyboardEvent, items: ContextMenuEntry[]) {
      e.preventDefault()
      e.stopPropagation()
      const el = (e.currentTarget ?? e.target) as Element | null
      // A click with detail 0 came from Enter/Space on a button, not a pointer.
      const fromKeys = isKeyboardEvent(e) || (e.type === 'click' && (e as MouseEvent).detail === 0)
      if (fromKeys && el && 'getBoundingClientRect' in el) {
        // currentTarget may be a wrapper; the focused element is the anchor.
        const focused = isKeyboardEvent(e) && e.target instanceof Element ? e.target : el
        openAt(focused, items, { keyboard: true })
        return
      }
      const m = e as MouseEvent
      show({ x: m.clientX, y: m.clientY, keyboard: false }, items)
    },
    openAt,
    closeMenu() {
      state = null
    },
  }
}

/** How long a touch must hold before it counts as a right-click. */
export const LONG_PRESS_MS = 500
/** How far a held touch may drift before it is a scroll, not a press. */
const LONG_PRESS_SLOP_PX = 10

/** A Svelte action that gives an element the whole context-menu contract:
 *  right-click, the Menu key or Shift+F10 while it (or anything in it) has
 *  focus, and a long-press on touch — which also swallows the click that
 *  would otherwise follow the press. `items` is read at open time, so it
 *  always reflects current state; an empty list opens nothing.
 *
 *    <div use:contextMenuTarget={{ menu, items: () => rowMenu(row) }}>…</div>
 *
 *  Editable fields inside keep the browser's own menu (paste, spellcheck). */
export function contextMenuTarget(
  node: HTMLElement,
  params: { menu: ContextMenuController; items: () => ContextMenuEntry[] },
) {
  let p = params
  let timer: ReturnType<typeof setTimeout> | null = null
  let start: { x: number; y: number } | null = null
  let swallowClick = false

  const editable = (t: EventTarget | null) =>
    t instanceof HTMLElement && (t.isContentEditable || t.closest('input, textarea, select, [contenteditable="true"]') !== null)
  const cancel = () => {
    if (timer) clearTimeout(timer)
    timer = null
    start = null
  }

  const onContextMenu = (e: MouseEvent) => {
    if (editable(e.target)) return
    // A touch press in flight (Android fires contextmenu on a long-press of
    // its own): this event IS the press, and the click after it is not a tap.
    if (timer) {
      cancel()
      swallowClick = true
      swallowUntil = 0
    }
    p.menu.openMenu(e, p.items())
  }
  const onKeydown = (e: KeyboardEvent) => {
    if (!isMenuKey(e) || editable(e.target)) return
    p.menu.openMenu(e, p.items())
  }
  const onPointerdown = (e: PointerEvent) => {
    if (e.pointerType === 'mouse' || editable(e.target)) return
    cancel()
    start = { x: e.clientX, y: e.clientY }
    timer = setTimeout(() => {
      const at = start
      cancel()
      if (!at) return
      swallowClick = true
      swallowUntil = 0
      p.menu.openAt(at, p.items())
    }, LONG_PRESS_MS)
  }
  const onPointermove = (e: PointerEvent) => {
    if (start && Math.hypot(e.clientX - start.x, e.clientY - start.y) > LONG_PRESS_SLOP_PX) cancel()
  }
  // The release of a press that opened the menu may still be followed by the
  // browser's compatibility mouse events — a mousedown the open menu would
  // read as an outside click, and the click itself. Both are swallowed, and
  // only for a moment: a device that sends neither must not lose its next tap.
  let swallowUntil = 0
  const swallow = (e: MouseEvent) => {
    if (!swallowClick) return false
    if (performance.now() > swallowUntil && swallowUntil !== 0) {
      swallowClick = false
      return false
    }
    e.preventDefault()
    e.stopPropagation()
    return true
  }
  const onMousedownCapture = (e: MouseEvent) => void swallow(e)
  const onClickCapture = (e: MouseEvent) => {
    if (swallow(e)) swallowClick = false
  }
  const onPointerup = () => {
    // A press that ended without opening leaves nothing to swallow.
    if (timer) cancel()
    else if (swallowClick) swallowUntil = performance.now() + 600
  }

  // No iOS callout (the link/image sheet) under a held finger.
  node.style.setProperty('-webkit-touch-callout', 'none')
  node.addEventListener('contextmenu', onContextMenu)
  node.addEventListener('keydown', onKeydown)
  node.addEventListener('pointerdown', onPointerdown)
  node.addEventListener('pointermove', onPointermove)
  node.addEventListener('pointerup', onPointerup)
  node.addEventListener('pointercancel', cancel)
  node.addEventListener('mousedown', onMousedownCapture, true)
  node.addEventListener('click', onClickCapture, true)
  return {
    update(next: typeof params) {
      p = next
    },
    destroy() {
      cancel()
      node.removeEventListener('contextmenu', onContextMenu)
      node.removeEventListener('keydown', onKeydown)
      node.removeEventListener('pointerdown', onPointerdown)
      node.removeEventListener('pointermove', onPointermove)
      node.removeEventListener('pointerup', onPointerup)
      node.removeEventListener('pointercancel', cancel)
      node.removeEventListener('mousedown', onMousedownCapture, true)
      node.removeEventListener('click', onClickCapture, true)
    },
  }
}

/** The pair every row menu leads with: open the record, copy its deep link
 *  (the §Context menus order — primary action first, `copyAppLink` second).
 *
 *  `open` is the site's OWN primary action — a `navigate`, a rail selection, a
 *  modal — because a context menu is a shortcut for what clicking the row
 *  already does. `path` is the deep link this record is shared by; the site
 *  spells it once, here. `icons` is for the menus that carry the icon register
 *  (the board's tickets and the boards rail): the plain form is the default, so
 *  adopting this pair never adds chrome a surface did not have. */
export function openCopyItems(
  path: string,
  open: () => void,
  icons?: { open?: MenuIcon; copy?: MenuIcon },
): ContextMenuEntry[] {
  return [
    { label: 'Open', icon: icons?.open, onSelect: open },
    { label: 'Copy link', icon: icons?.copy, onSelect: () => copyAppLink(path) },
  ]
}

/** The "Copy text" item — the raw string on the clipboard, disabled when there
 *  is nothing to copy. For the selection-or-body menus, pass the resolved
 *  value: `disabled` is `!value`, which is the same test they each spelled. */
export function copyTextItems(value: string): ContextMenuEntry[] {
  return [{ label: 'Copy text', disabled: !value, onSelect: () => void navigator.clipboard.writeText(value) }]
}

// Re-export from its real home (lib) — kit files hold no app URL logic.
export { copyAppLink }
export { isMenuKey }
