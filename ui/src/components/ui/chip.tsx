// Small shared vocabulary: kind/status chips, health dots, and the quiet red
// link that destructive actions use instead of a button.
import { cn } from '@/lib/cn'

export type ChipTone = 'neutral' | 'accent' | 'success' | 'warn' | 'danger'

const TONES: Record<ChipTone, string> = {
  neutral: 'border-line-subtle text-muted',
  accent: 'border-[var(--theme-accent-border)] text-accent',
  success: 'border-[color:var(--theme-success)]/40 text-[color:var(--theme-success)]',
  warn: 'border-[color:var(--theme-warning)]/40 text-[color:var(--theme-warning)]',
  danger: 'border-[color:var(--theme-danger)]/40 text-[color:var(--theme-danger)]',
}

export interface ChipProps {
  children: React.ReactNode
  className?: string
  title?: string
  /** Tint — replaces ad-hoc recoloring by className. */
  tone?: ChipTone
  /** Renders as a toggle button (filter pills); `selected` is the active state. */
  onSelect?: () => void
  selected?: boolean
  /** Trailing ✕ that removes the token (renders as part of the chip). */
  onRemove?: () => void
}

/** Bordered micro-chip for kinds/modes ("DOC", "Brief", "custom") — and, with
 *  `onSelect`/`onRemove`, the one filter-pill and removable-token primitive. */
export function Chip({ children, className, title, tone = 'neutral', onSelect, selected, onRemove }: ChipProps) {
  const base = cn(
    'shrink-0 rounded border px-1.5 text-[10px] font-semibold uppercase tracking-wide',
    selected ? 'border-[var(--theme-accent-border)] bg-card text-fg' : TONES[tone],
    onSelect && !selected && 'transition-colors hover:text-fg',
    className,
  )
  const body = (
    <>
      {children}
      {onRemove && (
        <button
          type="button"
          aria-label="Remove"
          onClick={(e) => {
            e.stopPropagation()
            onRemove()
          }}
          className="ml-1 text-muted transition-colors hover:text-[color:var(--theme-danger)]"
        >
          ✕
        </button>
      )}
    </>
  )
  if (onSelect) {
    return (
      <button type="button" title={title} onClick={onSelect} className={cn(base, 'inline-flex items-center')}>
        {body}
      </button>
    )
  }
  return (
    <span title={title} className={cn(base, onRemove && 'inline-flex items-center')}>
      {body}
    </span>
  )
}

export type DotStatus = 'ok' | 'warn' | 'danger' | 'idle' | 'accent'

const DOT_COLOR: Record<DotStatus, string> = {
  ok: 'var(--theme-success)',
  warn: 'var(--theme-warning)',
  danger: 'var(--theme-danger)',
  idle: 'var(--theme-line)',
  accent: 'var(--theme-accent)',
}

/** The one status dot — h-2, status decides the color, `pulse` for live. */
export function StatusDot({ status, pulse, title, className }: { status: DotStatus; pulse?: boolean; title?: string; className?: string }) {
  return (
    <span
      title={title}
      className={cn('inline-block h-2 w-2 shrink-0 rounded-full', pulse && 'animate-pulse', className)}
      style={{ background: DOT_COLOR[status] }}
    />
  )
}

/** Destructive trigger: a quiet red link, never a button — it must not pull
 *  focus. Pair with the confirm() dialog for the deliberate step. */
export function DangerLink({
  onClick,
  children,
  className,
  disabled,
}: {
  onClick: () => void
  children: React.ReactNode
  className?: string
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'text-xs text-muted underline-offset-2 transition-colors hover:text-[color:var(--theme-danger)] hover:underline disabled:opacity-40',
        className,
      )}
    >
      {children}
    </button>
  )
}
