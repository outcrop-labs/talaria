// TALA-4 (onboarding surface): the decisions behind the new-agent refine
// feedback. The component (CreateAgentModal / RefineBar) wires these in; the
// pure functions here own the math so every branch is pinnable in tests.

import { diffLines } from '@/components/fleet/line-diff'

export interface AppliedField {
  /** The field as the review step labels it. */
  label: string
  /** True when the refine REPLACED the whole value (muse redrafts fields in
   *  full), false when it arrived as new content this run. */
  changed: boolean
}

/** Which review-step fields a refine actually touched, in display order.
 *  Identity fields (name, handle) are never invented by the refine — a draft
   *  the model declined to rename leaves them untouched and the receipt says
   *  so by omission. */
export function appliedFields(
  before: Record<string, unknown>,
  after: Record<string, unknown>,
  order: Array<{ label: string; key: string }>,
): AppliedField[] {
  const out: AppliedField[] = []
  for (const { label, key } of order) {
    const b = before[key]
    const a = after[key]
    if (typeof a === 'string' && a !== b) out.push({ label, changed: true })
    else if (Array.isArray(a) && JSON.stringify(a) !== JSON.stringify(b)) out.push({ label, changed: true })
  }
  return out
}

export interface SoulSummary {
  adds: number
  dels: number
  /** Line-diff sized the soul: the summary is trustworthy. */
  text: string
  /** The soul was too large to diff — say "changed" without numbers. */
  oversized: boolean
}

/** What the refine did to the soul, for the receipt line: "3 added · 1 removed
 *  line". Degrades to an honest "changed" when the soul outgrew the client
 *  diff. Empty before (a role-template soul being replaced by a designed one,
 *  or a first design) reads as "wrote N lines" rather than a diff from
 *  nothing. */
export function summarizeSoul(before: string, after: string): SoulSummary {
  if (!before.trim()) {
    const lines = after.trim() ? after.split('\n').length : 0
    return {
      adds: lines,
      dels: 0,
      text: lines ? `wrote ${lines} ${lines === 1 ? 'line' : 'lines'}` : 'empty',
      oversized: false,
    }
  }
  const d = diffLines(before, after)
  if (!d) return { adds: 0, dels: 0, text: 'changed', oversized: true }
  const adds = d.filter((l) => l.type === 'add').length
  const dels = d.filter((l) => l.type === 'del').length
  const lineWord = adds === 1 && dels === 0 ? 'line' : 'lines'
  if (adds === 0 && dels === 0) return { adds, dels, text: 'no text changes', oversized: false }
  return { adds, dels, text: `${adds} added · ${dels} removed ${lineWord}`, oversized: false }
}
