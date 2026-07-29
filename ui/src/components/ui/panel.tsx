import { cn } from '@/lib/cn'

export type PanelProps = React.HTMLAttributes<HTMLElement> & {
  /** Semantic element — `section` for page sections, default `div`. */
  as?: 'div' | 'section' | 'article' | 'aside'
}

/** The core matte-glass surface. Reuse for cards/dialogs — don't re-style.
 *
 *  Owns the card padding (p-6) so density stays consistent app-wide — don't
 *  hand-set p-* per card; override (e.g. `p-0` for flush tables) only when the
 *  content genuinely demands it. Card internals convention: header block mb-4,
 *  tiny uppercase labels mb-2, list rows py-3, chip/meta clusters mt-2.5. */
export function Panel({ className, as: Tag = 'div', ...props }: PanelProps) {
  return <Tag className={cn('mercury-panel rounded-2xl p-6', className)} {...props} />
}
