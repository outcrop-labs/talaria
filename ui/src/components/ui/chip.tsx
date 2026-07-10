// Small shared vocabulary: kind/status chips, health dots, and the quiet red
// link that destructive actions use instead of a button.
import { cn } from '@/lib/cn'

/** Bordered micro-chip for kinds/modes ("DOC", "Brief", "custom"). */
export function Chip({ children, className, title }: { children: React.ReactNode; className?: string; title?: string }) {
  return (
    <span
      title={title}
      className={cn(
        'shrink-0 rounded border border-line-subtle px-1.5 text-[10px] font-semibold uppercase tracking-wide text-muted',
        className,
      )}
    >
      {children}
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
