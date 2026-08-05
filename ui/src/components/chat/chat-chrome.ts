import { cn } from '@/lib/cn'

// Mercury chat chrome vocabulary (spec §7 + §10) — the shared class
// strings and micro-components every composer/chat surface leans on, so the
// rail chips, icon tiles, and picker popovers stay pixel-identical across
// chat, channels, comms, and any prompt surface.
//
// Components: PopSearch / MeterBars / MessageAvatar (.svelte); the shared
// class strings live here.

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

/** Picker popover panel (model-menu pattern, `3Q-0`). `gd-enter` gives every
 *  popover the §9 entrance (180ms fade + 4px rise; fade-only under
 *  prefers-reduced-motion) — exits stay instant, i.e. shorter. */
export const popPanel = 'gd-enter rounded-[10px] border border-line bg-panel p-1 shadow-[var(--theme-shadow-2)]'
/** 10px mono ink-dim section header inside a popover. */
export const popHeader = 'px-2 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'
/** Popover option row: 13px sans, hover fill; selected adds a dashed gold outline. */
export const popRow =
  'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left font-sans text-[13px] transition-colors hover:bg-hover'
export const popRowSelected = 'text-fg outline outline-1 outline-dashed outline-accent -outline-offset-1'
