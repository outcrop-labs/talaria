// The composer's send + stop affordances (Mercury, spec §7). Send is the
// gold 36×36 tile that lives INSIDE the prompt well, top-right; Enter still
// sends everywhere and the KeyHint chip on the control rail says so. Stop is
// a 36×36 rail tile that pulses while a reply streams.
import { ArrowUp, Square } from 'lucide-react'
import { cn } from '@/lib/cn'
import { focusGold } from '@/components/chat/chat-chrome'

/** The gold submit tile: 36×36, radius 6, dark up-arrow on gold. Disabled
 *  reads as a raised tile with a muted glyph (spec §7). */
export function SendButton({
  onClick,
  enabled,
  title = 'Send (⏎)',
  className,
}: {
  onClick: () => void
  enabled: boolean
  title?: string
  className?: string
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={!enabled}
      onClick={onClick}
      className={cn(
        'grid h-9 w-9 shrink-0 place-items-center rounded-md transition-colors',
        enabled
          ? 'bg-accent text-[color:var(--theme-bg)] hover:bg-[color:var(--theme-accent-secondary)]'
          : 'border border-line bg-raised text-muted',
        focusGold,
        className,
      )}
    >
      <ArrowUp size={16} strokeWidth={2.25} />
    </button>
  )
}

/** Stop pulses while a reply streams — motion marks the live, interruptible
 *  moment. Esc triggers it from the keyboard. */
export function StopButton({ onClick, title = 'Stop (Esc)' }: { onClick: () => void; title?: string }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className={cn(
        'relative grid h-9 w-9 shrink-0 place-items-center rounded-md border border-line-strong text-muted transition-colors hover:bg-hover hover:text-fg',
        focusGold,
      )}
    >
      <span className="gd-pulse absolute inset-0 rounded-md border border-[color:var(--theme-accent-border)]" />
      <Square size={12} fill="currentColor" className="gd-pulse" />
    </button>
  )
}
