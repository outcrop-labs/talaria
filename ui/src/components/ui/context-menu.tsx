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
  onSelect: () => void
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
        selectable[active]?.onSelect()
        onClose()
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
        return (
          <button
            key={`${item.label}${i}`}
            type="button"
            role="menuitem"
            disabled={item.disabled}
            onMouseEnter={() => !item.disabled && setActive(idx)}
            onClick={() => {
              item.onSelect()
              onClose()
            }}
            className={cn(
              'flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-sm transition-colors',
              item.disabled
                ? 'cursor-default text-muted opacity-50'
                : item.danger
                  ? cn('text-[color:var(--theme-danger)]', active === idx && 'bg-[color:var(--theme-danger)]/10')
                  : cn('text-fg', active === idx && 'bg-sidebar'),
            )}
          >
            {item.icon && <span className="grid w-4 shrink-0 place-items-center text-muted">{item.icon}</span>}
            <span className="min-w-0 flex-1 truncate">{item.label}</span>
          </button>
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
  className,
}: {
  /** Renders the trigger; `open` lets it style its active state. */
  trigger: (open: boolean) => ReactNode
  items: ContextMenuEntry[] | (() => ContextMenuEntry[])
  align?: 'left' | 'right'
  className?: string
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const entries = typeof items === 'function' ? (open ? items() : []) : items
  return (
    <div ref={ref} className={cn('relative', className)}>
      <span onClick={() => setOpen((o) => !o)}>{trigger(open)}</span>
      {open && (
        <div
          role="menu"
          className={cn('absolute top-full z-40 mt-1 min-w-44 rounded-xl border border-line bg-card p-1 shadow-lg', align === 'right' ? 'right-0' : 'left-0')}
        >
          {entries.map((item, i) => {
            if (item === 'sep') return <div key={`s${i}`} className="mx-2 my-1 border-t border-line-subtle" />
            return (
              <button
                key={`${item.label}${i}`}
                type="button"
                role="menuitem"
                disabled={item.disabled}
                onClick={() => {
                  item.onSelect()
                  setOpen(false)
                }}
                className={cn(
                  'flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-sm transition-colors',
                  item.disabled
                    ? 'cursor-default text-muted opacity-50'
                    : item.danger
                      ? 'text-[color:var(--theme-danger)] hover:bg-[color:var(--theme-danger)]/10'
                      : 'text-fg hover:bg-sidebar',
                )}
              >
                {item.icon && <span className="grid w-4 shrink-0 place-items-center text-muted">{item.icon}</span>}
                <span className="min-w-0 flex-1 truncate">{item.label}</span>
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
