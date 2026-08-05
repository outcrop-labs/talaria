// Shared helpers for the focus inbox surface (FocusInbox.svelte and its
// section components).
import type { FocusItem } from '@/lib/inbox-focus.svelte'

export const PIPELINE = ['Signal', 'Triage', 'Execute', 'Validate', 'Close'] as const

export function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)
}

export function stageFor(item: FocusItem): number {
  if (item.sourceType === 'approval') return 2
  if (item.statusLabel.includes('REVIEW')) return 3
  if (item.statusLabel.includes('BLOCKED') || item.statusLabel.includes('TRIAGE')) return 1
  if (item.sourceType === 'channel') return 1
  return 0
}

export function metadataValue(value: string | number | boolean | null): string {
  if (value === null) return '—'
  if (typeof value === 'boolean') return value ? 'Yes' : 'No'
  return String(value).replaceAll('_', ' ')
}

export function priorityClass(priority: FocusItem['priority']): string {
  if (priority === 'p0') return 'text-accent'
  if (priority === 'p1') return 'text-danger'
  if (priority === 'p2') return 'text-muted'
  return 'text-success'
}

export function sourceLabel(item: FocusItem): string {
  if (item.sourceType === 'channel') return 'Comms'
  return item.sourceType[0]!.toUpperCase() + item.sourceType.slice(1)
}
