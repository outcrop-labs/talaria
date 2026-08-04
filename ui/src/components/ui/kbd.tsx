// Keyboard-key chip + the composer's contextual key hint. Chats are meant to
// be keyboard-driven — these make the keys visible right where the action is,
// fading in only when the shortcut is actually available.
// Gentle dew: raised tile + hairline + 10px mono chrome voice.
import { cn } from '@/lib/cn'

export function Kbd({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <kbd
      className={cn(
        'inline-grid h-5 min-w-5 place-items-center rounded-md border border-line bg-raised px-1.5 font-mono text-[10px] leading-none tracking-[0.05em] text-muted',
        className,
      )}
    >
      {children}
    </kbd>
  )
}

/** A "⏎ send" style hint beside a composer control. Renders always (so layout
 *  never jumps) and fades with availability. */
export function KeyHint({
  keys,
  label,
  visible,
  className,
}: {
  keys: string
  label?: string
  visible: boolean
  className?: string
}) {
  return (
    <span
      aria-hidden={!visible}
      className={cn(
        'flex select-none items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-opacity duration-200',
        visible ? 'opacity-100' : 'opacity-0',
        className,
      )}
    >
      <Kbd>{keys}</Kbd>
      {label}
    </span>
  )
}
