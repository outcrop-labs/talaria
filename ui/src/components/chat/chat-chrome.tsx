import { useEffect, useRef } from 'react'
import { Search } from 'lucide-react'
import { cn } from '@/lib/cn'

// Gentle dew chat chrome vocabulary (spec §7 + §10) — the shared class
// strings and micro-components every composer/chat surface leans on, so the
// rail chips, icon tiles, and picker popovers stay pixel-identical across
// chat, channels, comms, and any prompt surface.

/** Dashed gold keyboard focus — the chip focus treatment from board `3Q-0`. */
export const focusGold =
  'focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-offset-2 focus-visible:outline-accent'

/** Control-rail chip base: 36px tall, radius 6, mono 10/12 uppercase. */
export const chipBase = cn(
  'flex h-9 shrink-0 items-center gap-2 rounded-md border px-2.5 font-mono text-[10px] uppercase leading-3 tracking-[0.05em] transition-colors',
  focusGold,
)
/** Primary chips (agent/model) read brighter: strong border + readout text. */
export const chipPrimary = cn(chipBase, 'border-line-strong text-fg hover:bg-hover')
/** Secondary chips: hairline border + muted text → readout on hover. */
export const chipSecondary = cn(chipBase, 'border-line text-muted hover:bg-hover hover:text-fg')

/** 36×36 radius-6 icon tile (attach `+`, emoji, stop). */
export const tileBase = cn(
  'grid h-9 w-9 shrink-0 place-items-center rounded-md border border-line text-muted transition-colors hover:bg-hover hover:text-fg disabled:opacity-40',
  focusGold,
)

/** Picker popover panel (model-menu pattern, `3Q-0`). */
export const popPanel = 'rounded-[10px] border border-line bg-panel p-1 shadow-[var(--theme-shadow-2)]'
/** 10px mono ink-dim section header inside a popover. */
export const popHeader = 'px-2 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'
/** Popover option row: 13px sans, hover fill; selected adds a dashed gold outline. */
export const popRow =
  'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left font-sans text-[13px] transition-colors hover:bg-hover'
export const popRowSelected = 'text-fg outline outline-1 outline-dashed outline-accent -outline-offset-1'

/** The §7 popover search row: hairline field on top, mono placeholder, ⌘K
 *  hint. The hint is functional, not decorative (spec §7: no fabricated
 *  affordances) — while the popover is open, ⌘K refocuses the field. */
export function PopSearch({
  value,
  onChange,
  placeholder,
  className,
}: {
  value: string
  onChange: (v: string) => void
  placeholder: string
  className?: string
}) {
  const inputRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        inputRef.current?.focus()
        inputRef.current?.select()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])
  return (
    <div className={cn('mb-1 flex items-center gap-1.5 rounded-md border border-line px-2', className)}>
      <Search size={12} className="shrink-0 text-muted" />
      <input
        ref={inputRef}
        autoFocus
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="h-7 w-full min-w-0 bg-transparent font-mono text-[11px] tracking-[0.05em] text-fg outline-none placeholder:text-muted"
      />
      <span aria-hidden className="shrink-0 font-mono text-[10px] tracking-[0.05em] text-ink-dim">
        ⌘K
      </span>
    </div>
  )
}

/** Chip meter: 3×12 bars, ~2px gap — lit bars gold, unlit raised-tone. */
export function MeterBars({ total, lit, className }: { total: number; lit: number; className?: string }) {
  return (
    <span aria-hidden className={cn('flex items-center gap-[2px]', className)}>
      {Array.from({ length: total }, (_, i) => (
        <span key={i} className={cn('h-3 w-[3px] rounded-full', i < lit ? 'bg-accent' : 'bg-raised')} />
      ))}
    </span>
  )
}

/** Small square avatar for flattened message rows (spec §10 / §8): raised
 *  tile, strong ring, mono initials. */
export function MessageAvatar({ name, className }: { name?: string | null; className?: string }) {
  const words = (name ?? '').trim().split(/\s+/).filter(Boolean)
  const initials =
    words.length >= 2 ? `${words[0]![0]}${words[1]![0]}` : (words[0]?.slice(0, 2) ?? '?')
  return (
    <span
      className={cn(
        'grid h-6 w-6 shrink-0 place-items-center rounded border border-line-strong bg-raised font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-muted',
        className,
      )}
    >
      {initials}
    </span>
  )
}
