import { X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { focusRing } from './control'

/** The one close (✕) button. Padded hit area so you don't have to click dead-on
 *  the glyph. Reuse for modals, panels, dismissible surfaces. */
export function CloseButton({
  onClick,
  className,
  size = 16,
  label = 'Close',
}: {
  onClick: () => void
  className?: string
  size?: number
  label?: string
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className={cn(
        // Ghost icon affordance (spec §8): muted → readout, hover-token fill,
        // radius 6, gold focus ring.
        'grid place-items-center rounded-md p-1.5 text-muted transition-colors hover:bg-hover hover:text-fg',
        focusRing,
        className,
      )}
    >
      <X size={size} strokeWidth={2} />
    </button>
  )
}
