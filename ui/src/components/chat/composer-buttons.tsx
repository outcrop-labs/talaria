// The composer's send/stop affordances — icons, not wide labeled buttons.
// Geometry contract for composer rows (flex items-end): h-9 controls carry
// `self-end mb-1`, pill controls `self-end mb-[7px]` — everything sits on the
// same optical line against the resting 2.75rem textarea and stays
// bottom-anchored as it grows. Keyboard first: Enter sends, Esc stops, and
// the KeyHint chips beside these buttons say so.
import { ArrowUp, Square } from 'lucide-react'
import { cn } from '@/lib/cn'

export function SendButton({ onClick, disabled, title = 'Send (Enter)' }: { onClick: () => void; disabled?: boolean; title?: string }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'group grid h-9 w-9 shrink-0 place-items-center self-end rounded-xl bg-accent text-surface transition-opacity mb-1',
        'hover:opacity-90 disabled:opacity-30',
      )}
    >
      <ArrowUp size={16} strokeWidth={2.5} className="transition-transform duration-150 group-hover:-translate-y-0.5 group-disabled:translate-y-0" />
    </button>
  )
}

/** Stop pulses while a reply streams — motion marks the live, interruptible
 *  moment. */
export function StopButton({ onClick, title = 'Stop (Esc)' }: { onClick: () => void; title?: string }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="relative grid h-9 w-9 shrink-0 place-items-center self-end rounded-xl border border-line text-muted transition-colors mb-1 hover:border-accent hover:text-fg"
    >
      <span className="absolute inset-0 animate-pulse rounded-xl border border-accent/40" />
      <Square size={13} fill="currentColor" className="animate-pulse" />
    </button>
  )
}
