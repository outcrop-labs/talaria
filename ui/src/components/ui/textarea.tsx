import { forwardRef, useLayoutEffect, useRef } from 'react'
import { cn } from '@/lib/cn'

export type TextareaProps = React.TextareaHTMLAttributes<HTMLTextAreaElement> & {
  /** Grow downward with content: starts at `rows`, expands as the user types,
   *  capped by any `max-h-*` in className (then scrolls). */
  autoGrow?: boolean
}

/** The one textarea. Reuse everywhere — do not re-style textareas inline. */
export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(({ className, autoGrow, ...props }, ref) => {
  const inner = useRef<HTMLTextAreaElement | null>(null)

  // Fit height to content before paint (controlled and uncontrolled both pass
  // through here on every value change).
  useLayoutEffect(() => {
    const el = inner.current
    if (!autoGrow || !el) return
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight + 2}px` // +2: box border
  })

  return (
    <textarea
      ref={(el) => {
        inner.current = el
        if (typeof ref === 'function') ref(el)
        else if (ref) ref.current = el
      }}
      className={cn(
        // Spec §8: raised tile bg, hairline border, radius 6, gold focus ring.
        'w-full resize-none rounded-md border border-line bg-[var(--theme-input)] px-3 py-2.5 font-sans text-sm text-fg outline-none transition-colors',
        'placeholder:text-muted focus:border-accent focus:ring-2 focus:ring-accent-soft',
        autoGrow && 'overflow-y-auto',
        className,
      )}
      {...props}
    />
  )
})
Textarea.displayName = 'Textarea'
