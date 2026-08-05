import type { Component, Snippet } from 'svelte'

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

// Re-export from its real home (lib) — kit files hold no app URL logic.
export { copyAppLink } from '@/lib/links'
