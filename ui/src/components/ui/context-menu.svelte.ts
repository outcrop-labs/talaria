import type { Component, Snippet } from 'svelte'
import { copyAppLink } from '@/lib/links'

// The one context menu. Right-click a row/card/tile → its actions at the
// cursor. Same shell as dropdown menus (the §7 popover pattern: panel surface,
// hairline, radius 10, matte shadow — `popPanel` from chat-chrome); portaled
// to <body> so backdrop-filter surfaces can't trap the fixed positioning.
// Esc / outside-click / scroll close it.
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
}

export interface ContextMenuController {
  /** The open menu (position + items), or null when closed. */
  readonly state: MenuState | null
  openMenu: (e: MouseEvent, items: ContextMenuEntry[]) => void
  closeMenu: () => void
}

export function useContextMenu(): ContextMenuController {
  let state = $state<MenuState | null>(null)

  return {
    get state() {
      return state
    },
    openMenu(e: MouseEvent, items: ContextMenuEntry[]) {
      e.preventDefault()
      e.stopPropagation()
      const real = items.filter((i) => i !== 'sep')
      if (real.length === 0) return
      state = { x: e.clientX, y: e.clientY, items }
    },
    closeMenu() {
      state = null
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
