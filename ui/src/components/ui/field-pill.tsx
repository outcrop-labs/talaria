// FieldPill — the clickable-inline-value affordance. A property shown as a
// quiet pill that REVEALS its editability: border + chevron surface on hover,
// cursor says clickable, active state while its picker is open. Use it as the
// trigger for DropdownMenu pickers on rows, cards, and property rails —
// never a bare <select>/<input> masquerading as text.
import { forwardRef } from 'react'
import { ChevronDown } from 'lucide-react'
import { cn } from '@/lib/cn'

export const FieldPill = forwardRef<
  HTMLButtonElement,
  Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'children'> & {
    /** Leading dot color (status/priority) — omit for none. */
    dot?: string
    icon?: React.ReactNode
    /** Picker-open state (from DropdownMenu's trigger render prop). */
    active?: boolean
    /** Muted placeholder styling when the field is unset. */
    empty?: boolean
    /** Always look like a control: visible border + chevron at rest. Use on
     *  cards, where hover-reveal made editability a mystery. */
    persistent?: boolean
    children: React.ReactNode
  }
>(function FieldPill({ dot, icon, active, empty, persistent, className, children, onClick, ...rest }, ref) {
  return (
    <button
      ref={ref}
      type="button"
      onClick={onClick}
      // Stop drag-start on draggable cards; the CLICK is left to bubble one
      // level — DropdownMenu's trigger wrapper owns it (and stops it there,
      // so rows/cards underneath never fire).
      onMouseDown={(e) => e.stopPropagation()}
      className={cn(
        'group/pill inline-flex max-w-full cursor-pointer items-center gap-1.5 rounded-md border px-1.5 py-0.5 text-left text-[11px] transition-colors',
        active
          ? 'border-line bg-card text-fg'
          : persistent
            ? 'border-line-subtle bg-card/40 text-muted hover:border-line hover:bg-card hover:text-fg'
            : 'border-transparent text-muted hover:border-line hover:bg-card hover:text-fg',
        empty && 'italic',
        className,
      )}
      {...rest}
    >
      {dot && <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: dot }} />}
      {icon && <span className="grid w-3.5 shrink-0 place-items-center">{icon}</span>}
      <span className="min-w-0 truncate">{children}</span>
      <ChevronDown
        size={11}
        className={cn(
          'shrink-0 transition-opacity',
          active ? 'opacity-100' : persistent ? 'opacity-50 group-hover/pill:opacity-100' : 'opacity-0 group-hover/pill:opacity-100',
        )}
      />
    </button>
  )
})
