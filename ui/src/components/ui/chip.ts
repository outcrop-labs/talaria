// Small shared vocabulary: kind/status chips, health dots, and the quiet
// destructive link (orange text, never a fill) that destructive actions use
// instead of a button. Components: Chip/StatusDot/DangerLink.svelte; the
// shared tone/status tables and prop types live here.
import type { Snippet } from 'svelte'

export type ChipTone = 'neutral' | 'accent' | 'success' | 'warn' | 'danger'

export const TONES: Record<ChipTone, string> = {
  neutral: 'border-line text-muted',
  accent: 'border-[var(--theme-accent-border)] text-accent',
  success: 'border-success/40 text-success',
  warn: 'border-warning/40 text-warning',
  danger: 'border-danger/40 text-danger',
}

export interface ChipProps {
  children: Snippet
  class?: string
  title?: string
  /** Tint — replaces ad-hoc recoloring by className. */
  tone?: ChipTone
  /** Renders as a toggle button (filter pills); `selected` is the active state. */
  onSelect?: () => void
  selected?: boolean
  /** Trailing ✕ that removes the token (renders as part of the chip). */
  onRemove?: () => void
}

export type DotStatus = 'ok' | 'warn' | 'danger' | 'idle' | 'accent'

export const DOT_COLOR: Record<DotStatus, string> = {
  ok: 'var(--theme-success)',
  warn: 'var(--theme-warning)',
  danger: 'var(--theme-danger)',
  idle: 'var(--theme-line)',
  accent: 'var(--theme-accent)',
}
