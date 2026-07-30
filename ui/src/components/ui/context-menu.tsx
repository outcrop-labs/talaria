import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/cn'

// The one context menu. Right-click a row/card/tile → its actions at the
// cursor. Same shell as dropdown menus (rounded-xl border bg-card p-1
// shadow-lg); portaled to <body> so backdrop-filter surfaces can't trap the
// fixed positioning. Esc / outside-click / scroll close it.
//
// Usage:
//   const { openMenu, menu } = useContextMenu()
//   <div onContextMenu={(e) => openMenu(e, [
//     { label: 'Open', onSelect: ... },
//     'sep',
//     { label: 'Delete', danger: true, onSelect: ... },
//   ])}>…</div>
//   {menu}
//
// Items should mirror actions the surface already offers — a context menu is
// a shortcut, never the only home of an action.

export interface ContextMenuItem {
  label: string
  icon?: ReactNode
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

interface MenuState {
  x: number
  y: number
  items: ContextMenuEntry[]
}

export function useContextMenu() {
  const [state, setState] = useState<MenuState | null>(null)

  const openMenu = useCallback((e: React.MouseEvent, items: ContextMenuEntry[]) => {
    e.preventDefault()
    e.stopPropagation()
    const real = items.filter((i) => i !== 'sep')
    if (real.length === 0) return
    setState({ x: e.clientX, y: e.clientY, items })
  }, [])

  const close = useCallback(() => setState(null), [])

  return {
    openMenu,
    closeMenu: close,
    menu: state ? <Menu state={state} onClose={close} /> : null,
  }
}

function Menu({ state, onClose }: { state: MenuState; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  const [active, setActive] = useState(-1)
  const [openSub, setOpenSub] = useState<number | null>(null)
  const selectable = state.items.filter((i): i is ContextMenuItem => i !== 'sep' && !i.disabled)

  // Clamp inside the viewport once we know our size.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const r = el.getBoundingClientRect()
    if (state.x + r.width > window.innerWidth - 8) el.style.left = `${Math.max(8, window.innerWidth - r.width - 8)}px`
    if (state.y + r.height > window.innerHeight - 8) el.style.top = `${Math.max(8, window.innerHeight - r.height - 8)}px`
  }, [state])

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setActive((a) => (a + 1) % selectable.length)
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setActive((a) => (a - 1 + selectable.length) % selectable.length)
      } else if (e.key === 'Enter' && active >= 0) {
        e.preventDefault()
        const it = selectable[active]
        if (it && !it.children?.length) {
          it.onSelect?.()
          onClose()
        }
      }
    }
    const onScroll = () => onClose()
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey, true)
    document.addEventListener('scroll', onScroll, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey, true)
      document.removeEventListener('scroll', onScroll, true)
    }
  }, [onClose, active, selectable])

  if (typeof document === 'undefined') return null
  let selIdx = -1
  return createPortal(
    <div
      ref={ref}
      role="menu"
      style={{ position: 'fixed', left: state.x, top: state.y, zIndex: 80 }}
      className="min-w-44 rounded-xl border border-line bg-card p-1 shadow-lg"
      onContextMenu={(e) => e.preventDefault()}
    >
      {state.items.map((item, i) => {
        if (item === 'sep') return <div key={`s${i}`} className="mx-2 my-1 border-t border-line-subtle" />
        if (!item.disabled) selIdx += 1
        const idx = selIdx
        const hasKids = !!item.children?.length
        return (
          <div key={`${item.label}${i}`} className="relative" onMouseLeave={() => hasKids && setOpenSub((s) => (s === i ? null : s))}>
            <button
              type="button"
              role="menuitem"
              disabled={item.disabled}
              aria-haspopup={hasKids || undefined}
              onMouseEnter={() => {
                if (item.disabled) return
                setActive(idx)
                setOpenSub(hasKids ? i : null)
              }}
              onClick={() => {
                if (hasKids) return setOpenSub((s) => (s === i ? null : i))
                item.onSelect?.()
                onClose()
              }}
              className={cn(
                'flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-sm transition-colors',
                item.disabled
                  ? 'cursor-default text-muted opacity-50'
                  : item.danger
                    ? cn('text-[color:var(--theme-danger)]', active === idx && 'bg-[color:var(--theme-danger)]/10')
                    : cn('text-fg', (active === idx || openSub === i) && 'bg-sidebar'),
              )}
            >
              {item.checked !== undefined ? (
                <span className="grid w-4 shrink-0 place-items-center text-accent">{item.checked ? '✓' : ''}</span>
              ) : (
                item.icon && <span className="grid w-4 shrink-0 place-items-center text-muted">{item.icon}</span>
              )}
              <span className="min-w-0 flex-1 truncate">{item.label}</span>
              {hasKids && <span className="shrink-0 text-xs text-muted">▸</span>}
            </button>
            {hasKids && openSub === i && (
              <div className="absolute left-full top-0 z-10 -ml-0.5 min-w-40 rounded-xl border border-line bg-card p-1 shadow-lg">
                {item.children!.map((kid, k) => {
                  if (kid === 'sep') return <div key={`ks${k}`} className="mx-2 my-1 border-t border-line-subtle" />
                  return (
                    <button
                      key={`${kid.label}${k}`}
                      type="button"
                      role="menuitem"
                      disabled={kid.disabled}
                      onClick={() => {
                        kid.onSelect?.()
                        onClose()
                      }}
                      className={cn(
                        'flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-sm transition-colors',
                        kid.disabled
                          ? 'cursor-default text-muted opacity-50'
                          : kid.danger
                            ? 'text-[color:var(--theme-danger)] hover:bg-[color:var(--theme-danger)]/10'
                            : 'text-fg hover:bg-sidebar',
                      )}
                    >
                      {kid.checked !== undefined ? (
                        <span className="grid w-4 shrink-0 place-items-center text-accent">{kid.checked ? '✓' : ''}</span>
                      ) : (
                        kid.icon && <span className="grid w-4 shrink-0 place-items-center text-muted">{kid.icon}</span>
                      )}
                      <span className="min-w-0 flex-1 truncate">{kid.label}</span>
                    </button>
                  )
                })}
              </div>
            )}
          </div>
        )
      })}
    </div>,
    document.body,
  )
}

// Re-export from its real home (lib) — kit files hold no app URL logic.
export { copyAppLink } from '@/lib/links'

/** Anchored dropdown menu — the SAME shell and item grammar as the context
 *  menu, attached to a trigger instead of the cursor. Replaces every ad-hoc
 *  `absolute top-full … rounded-xl border bg-card p-1` panel. */
export function DropdownMenu({
  trigger,
  items,
  align = 'right',
  up = false,
  className,
  footer,
}: {
  /** Renders the trigger; `open` lets it style its active state. */
  trigger: (open: boolean) => ReactNode
  items: ContextMenuEntry[] | (() => ContextMenuEntry[])
  align?: 'left' | 'right'
  /** Open upward (triggers docked at the bottom of the viewport). */
  up?: boolean
  className?: string
  /** Rendered under the items (e.g. a date input) — clicks inside stay open. */
  footer?: (close: () => void) => ReactNode
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  // Fixed-position style computed from the trigger at open time. The panel
  // PORTALS to <body>: cards/panels carry backdrop-filter (a stacking
  // context), so an absolutely-positioned sibling could never stack above
  // neighboring cards, no z-index would save it.
  const [pos, setPos] = useState<React.CSSProperties | null>(null)

  const toggle = () => {
    if (open) return setOpen(false)
    const r = ref.current?.getBoundingClientRect()
    if (!r) return
    setPos({
      position: 'fixed',
      zIndex: 80,
      ...(up ? { bottom: window.innerHeight - r.top + 4 } : { top: r.bottom + 4 }),
      ...(align === 'right' ? { right: Math.max(8, window.innerWidth - r.right) } : { left: Math.max(8, r.left) }),
    })
    setOpen(true)
  }

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (!ref.current?.contains(t) && !panelRef.current?.contains(t)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    // Reposition-on-scroll is guesswork; closing matches the context menu.
    const onScroll = (e: Event) => {
      if (panelRef.current?.contains(e.target as Node)) return
      setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    document.addEventListener('scroll', onScroll, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
      document.removeEventListener('scroll', onScroll, true)
    }
  }, [open])

  const entries = typeof items === 'function' ? (open ? items() : []) : items
  return (
    <div ref={ref} className={cn('relative', className)}>
      {/* The trigger click is the menu's — never the row/card underneath. */}
      <span
        onClick={(e) => {
          e.stopPropagation()
          toggle()
        }}
      >
        {trigger(open)}
      </span>
      {open &&
        pos &&
        typeof document !== 'undefined' &&
        createPortal(
          <div
            ref={panelRef}
            role="menu"
            style={pos}
            onClick={(e) => e.stopPropagation()}
            className="min-w-44 max-w-72 rounded-xl border border-line bg-card p-1 shadow-lg"
          >
            <div className="max-h-80 overflow-y-auto">
              {entries.map((item, i) => {
                if (item === 'sep') return <div key={`s${i}`} className="mx-2 my-1 border-t border-line-subtle" />
                return (
                  <button
                    key={`${item.label}${i}`}
                    type="button"
                    role="menuitem"
                    disabled={item.disabled}
                    onClick={(e) => {
                      e.stopPropagation()
                      item.onSelect?.()
                      if (!item.keepOpen) setOpen(false)
                    }}
                    className={cn(
                      'flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-sm transition-colors',
                      item.disabled
                        ? 'cursor-default text-muted opacity-50'
                        : item.danger
                          ? 'text-[color:var(--theme-danger)] hover:bg-[color:var(--theme-danger)]/10'
                          : cn('hover:bg-sidebar', item.checked === false ? 'text-muted' : 'text-fg'),
                    )}
                  >
                    {/* checked state: leading ✓ slot (kept when unchecked so rows
                        align); falls back to the icon slot otherwise. */}
                    {item.checked !== undefined ? (
                      <span className="grid w-4 shrink-0 place-items-center text-accent">{item.checked ? '✓' : ''}</span>
                    ) : null}
                    {item.icon && <span className="grid w-4 shrink-0 place-items-center text-muted">{item.icon}</span>}
                    <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  </button>
                )
              })}
            </div>
            {footer && <div className="mt-1 border-t border-line-subtle px-2 pb-1 pt-2">{footer(() => setOpen(false))}</div>}
          </div>,
          document.body,
        )}
    </div>
  )
}
