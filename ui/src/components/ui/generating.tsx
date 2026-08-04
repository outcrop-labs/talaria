import { cn } from '@/lib/cn'

// Generation-in-progress — Mercury motion grammar (spec §9). Loaders are
// rows of small rounded bars (3×12px, like the chip meters) on the gd-*
// motifs: weave (submitting burst), scan (agent stage / tool activity),
// breathe (ambient background monitor), helix (reasoning / generating loop),
// pulse (two-state fallback — also the reduced-motion swap, handled in
// styles.css). Calmer than a spinner, honest about indeterminate work. Use
// `lines={0}` for just the status line.

export type GeneratingVariant = 'weave' | 'scan' | 'breathe' | 'helix' | 'pulse'

const MOTIF: Record<GeneratingVariant, string> = {
  weave: 'gd-weave',
  scan: 'gd-scan',
  breathe: 'gd-breathe',
  helix: 'gd-helix',
  pulse: 'gd-pulse',
}

/** One row of 3×12 motif bars. Colour rides on `currentColor` (set a text-*
 *  class): line-tone for content placeholders, accent gold for signal. */
export function GeneratingBars({
  bars = 5,
  variant = 'scan',
  delay = 0,
  step = 0.08,
  className,
}: {
  bars?: number
  variant?: GeneratingVariant
  /** Stagger offset for the whole row, seconds. */
  delay?: number
  /** Per-bar stagger, seconds (the cascade across the row). */
  step?: number
  className?: string
}) {
  return (
    <span aria-hidden className={cn('inline-flex items-center gap-[3px]', className)}>
      {Array.from({ length: bars }, (_, i) => (
        <span
          key={i}
          className={cn('gd-bar', MOTIF[variant])}
          style={{ animationDelay: `${(delay + i * step).toFixed(2)}s`, animationFillMode: 'backwards' }}
        />
      ))}
    </span>
  )
}

/** An agent is writing something and the result will replace this block:
 *  status line + `lines` rows of line-toned bars sweeping on the chosen
 *  motif (default `scan` — the standard agent-stage tier). */
export function Generating({
  label,
  lines = 3,
  variant = 'scan',
  className,
}: {
  label?: string
  lines?: number
  variant?: GeneratingVariant
  className?: string
}) {
  const counts = [14, 18, 11, 16, 9, 13]
  return (
    <div className={cn('rounded-lg border border-line p-4', className)}>
      {label && (
        <div className="flex items-center gap-2 font-sans text-sm text-muted">
          <GeneratingDots />
          <span>{label}</span>
        </div>
      )}
      {lines > 0 && (
        <div className={cn('space-y-2.5 text-line', label && 'mt-3.5')}>
          {Array.from({ length: lines }, (_, i) => (
            <GeneratingBars key={i} bars={counts[i % counts.length]!} variant={variant} delay={i * 0.15} className="flex" />
          ))}
        </div>
      )}
    </div>
  )
}

/** The inline activity signal on its own (status rows, buttons): three gold
 *  bars on the fast weave burst. */
export function GeneratingDots({ className, variant = 'weave' }: { className?: string; variant?: GeneratingVariant }) {
  return <GeneratingBars bars={3} variant={variant} step={0.15} className={cn('text-accent', className)} />
}

/** CONTEXT HELIX (spec §9): the reasoning / assistant-generating loop — a
 *  staggered multi-colour bar helix (chart-1 / gold / chart-2) orbiting on
 *  the standard budget. Reduced motion degrades it to the two-state pulse
 *  (styles.css) with the same colours. */
const HELIX_TONES = ['text-chart-1', 'text-accent', 'text-chart-2']
export function GeneratingHelix({ bars = 7, className }: { bars?: number; className?: string }) {
  return (
    <span aria-hidden className={cn('inline-flex items-center gap-[3px]', className)}>
      {Array.from({ length: bars }, (_, i) => (
        <span
          key={i}
          className={cn('gd-bar gd-helix', HELIX_TONES[i % HELIX_TONES.length])}
          style={{ animationDelay: `${(i * 0.12).toFixed(2)}s`, animationFillMode: 'backwards' }}
        />
      ))}
    </span>
  )
}

/** A veil over content being rewritten in place (e.g. the plan document while
 *  the agent resyncs it). Parent must be `relative`. */
export function GeneratingOverlay({ label }: { label: string }) {
  return (
    <div className="absolute inset-0 z-10 grid place-items-center bg-surface/60 backdrop-blur-[2px]">
      <Generating label={label} lines={4} className="w-72 bg-panel shadow-[var(--theme-shadow-2)]" />
    </div>
  )
}
