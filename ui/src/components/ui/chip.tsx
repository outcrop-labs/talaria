// Small shared vocabulary: kind/status chips, health dots, and the quiet
// destructive link (orange text, never a fill) that destructive actions use
// instead of a button.
import { cn } from '@/lib/cn'
import { focusGold } from '@/components/chat/chat-chrome'

export type ChipTone = 'neutral' | 'accent' | 'success' | 'warn' | 'danger'

const TONES: Record<ChipTone, string> = {
  neutral: 'border-line text-muted',
  accent: 'border-[var(--theme-accent-border)] text-accent',
  success: 'border-success/40 text-success',
  warn: 'border-warning/40 text-warning',
  danger: 'border-danger/40 text-danger',
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
 *  `onSelect`/`onRemove`, the one filter-pill and removable-token primitive.
 *  Mono chrome voice; selected = raised tile + strong hairline + readout. */
export function Chip({ children, className, title, tone = 'neutral', onSelect, selected, onRemove }: ChipProps) {
  const base = cn(
    'shrink-0 rounded border px-1.5 font-mono text-[10px] font-medium uppercase tracking-[0.05em]',
    selected ? 'border-line-strong bg-raised text-fg' : TONES[tone],
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
          className="ml-1 text-muted transition-colors hover:text-danger"
        >
          ✕
        </button>
      )}
    </>
  )
  if (onSelect) {
    return (
      <button type="button" title={title} onClick={onSelect} className={cn(base, 'inline-flex items-center', focusGold)}>
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

/** The one status dot — 6px round (spec §8), status decides the color
 *  (green healthy / gold attention / orange failure), `pulse` for live. */
export function StatusDot({ status, pulse, title, className }: { status: DotStatus; pulse?: boolean; title?: string; className?: string }) {
  return (
    <span
      title={title}
      className={cn('inline-block h-1.5 w-1.5 shrink-0 rounded-full', pulse && 'animate-pulse', className)}
      style={{ background: DOT_COLOR[status] }}
    />
  )
}

/** Destructive trigger: a quiet mono link that turns safety-orange on hover —
 *  never an orange fill, and never a button that pulls focus. Pair with the
 *  confirm() dialog for the deliberate step. */
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
        'font-mono text-[10px] uppercase tracking-[0.05em] text-muted underline-offset-2 transition-colors hover:text-danger hover:underline disabled:opacity-40',
        className,
      )}
    >
      {children}
    </button>
  )
}
