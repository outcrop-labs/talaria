import { cn } from '@/lib/cn'

export type PanelProps = React.HTMLAttributes<HTMLDivElement>

/** The core matte-glass surface. Reuse for cards/dialogs — don't re-style.
 *
 *  Owns the card padding (p-6) so density stays consistent app-wide — don't
 *  hand-set p-* per card; override (e.g. `p-0` for flush tables) only when the
 *  content genuinely demands it. Card internals convention: header block mb-4,
 *  tiny uppercase labels mb-2, list rows py-3, chip/meta clusters mt-2.5. */
export function Panel({ className, ...props }: PanelProps) {
  return <div className={cn('mercury-panel rounded-2xl p-6', className)} {...props} />
}
