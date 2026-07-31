import { cn } from '@/lib/cn'

// The Talaria brand — gold "A"-delta mark + spaced-caps wordmark
// (Gentle dew spec §4, exact SVG from the design file).
export function Brand({ className, showTag = false }: { className?: string; showTag?: boolean }) {
  return (
    <div className={cn('flex items-center gap-2.5', className)}>
      <WingMark className="h-5 w-5" />
      <div className="leading-tight">
        <div className="font-sans text-[13px] leading-4 font-semibold tracking-[0.18em] text-fg">
          TALARIA
        </div>
        {showTag && (
          <div className="theme-muted text-[11px] tracking-wide">the winged fleet cockpit</div>
        )}
      </div>
    </div>
  )
}

// The notched-triangle "A"-delta glyph in flat warm gold.
export function WingMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 20 20" className={className} aria-hidden>
      <path d="M10 2L18 18H12.6L10 12.4L7.4 18H2L10 2Z" fill="var(--theme-accent)" />
    </svg>
  )
}
