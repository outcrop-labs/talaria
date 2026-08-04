import { Check } from 'lucide-react'
import { cn } from '@/lib/cn'

// The one step indicator for wizard-style flows: numbered dots joined by
// hairlines, check-marked once passed. Gentle dew: passed = gold fill with
// dark ground glyph, current = gold outline, labels mono uppercase chrome.
// Keep flows to 2–4 steps.
export function Steps({ steps, current }: { steps: readonly string[]; current: number }) {
  return (
    <div className="flex items-center gap-2">
      {steps.map((label, i) => (
        <div key={label} className="flex items-center gap-2">
          {i > 0 && <span aria-hidden className="h-px w-6 bg-line" />}
          <span
            className={cn(
              'grid h-5 w-5 place-items-center rounded-full font-mono text-[10px] font-medium',
              i < current
                ? 'bg-accent text-surface'
                : i === current
                  ? 'border border-accent text-accent'
                  : 'border border-line text-muted',
            )}
          >
            {i < current ? <Check size={11} /> : i + 1}
          </span>
          <span
            className={cn(
              'font-mono text-[10px] uppercase tracking-[0.05em]',
              i === current ? 'font-medium text-fg' : 'text-muted',
            )}
          >
            {label}
          </span>
        </div>
      ))}
    </div>
  )
}
