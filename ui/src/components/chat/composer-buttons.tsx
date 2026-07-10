// The composer's stop affordance. There is deliberately NO send button —
// Enter sends everywhere and the KeyHint chip beside the input says so; the
// composer stays a quiet input line. Geometry contract (flex items-end rows):
// h-9 controls carry `self-end mb-1`, pill controls `self-end mb-[7px]`.
import { Square } from 'lucide-react'

/** Stop pulses while a reply streams — motion marks the live, interruptible
 *  moment. Esc triggers it from the keyboard. */
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
