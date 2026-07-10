// Keyboard-key chip + the composer's contextual key hint. Chats are meant to
// be keyboard-driven — these make the keys visible right where the action is,
// fading in only when the shortcut is actually available.
import { cn } from '@/lib/cn'

export function Kbd({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <kbd
      className={cn(
        'inline-grid h-4 min-w-4 place-items-center rounded border border-line-subtle bg-card px-1 font-mono text-[10px] leading-none text-muted',
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
        'flex select-none items-center gap-1 text-[10px] text-muted transition-opacity duration-200',
        visible ? 'opacity-100' : 'opacity-0',
        className,
      )}
    >
      <Kbd>{keys}</Kbd>
      {label}
    </span>
  )
}
