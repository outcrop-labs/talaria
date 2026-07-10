// The composer's send/stop affordances — icons, not wide labeled buttons.
// Enter submits everywhere; the tooltip says so.
import { ArrowUp, Square } from 'lucide-react'
import { cn } from '@/lib/cn'

export function SendButton({ onClick, disabled, title = 'Send — Enter' }: { onClick: () => void; disabled?: boolean; title?: string }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-accent text-surface transition-opacity',
        'hover:opacity-90 disabled:opacity-30',
      )}
    >
      <ArrowUp size={16} strokeWidth={2.5} />
    </button>
  )
}

export function StopButton({ onClick, title = 'Stop generating' }: { onClick: () => void; title?: string }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="grid h-9 w-9 shrink-0 place-items-center rounded-xl border border-line text-muted transition-colors hover:text-fg"
    >
      <Square size={13} fill="currentColor" />
    </button>
  )
}
