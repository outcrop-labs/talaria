// Which shape a chip renders as, decided once. ChatChips is a long if-chain
// over `kind`, and the chips a tool produces no longer map one-to-one onto it:
// an `image` chip without an upload has nothing to show, and a `link` chip
// that carries `changes` is a document card rather than a pill. Asking the
// question here keeps the template branching on an answer instead of
// re-deriving it per branch, and lets the fallbacks be tested.
import type { ChatChip, DocChange } from '@/lib/chips'
import type { DotStatus } from '@/components/ui/chip'

/** The renderings ChatChips has. `plain` is every chip whose `kind` already
 *  decides its shape — a tool, an approval, a notice, a bare link. */
export type ChipCard = 'image' | 'subagent' | 'source' | 'document' | 'plain'

/** The branch selector. Each card needs the field it draws from, so a chip
 *  missing that field falls back to `plain` rather than rendering an empty
 *  frame: an `image` with no upload has no bytes, and a `source` or a
 *  document card with no href has nothing to open. */
export function cardKindFor(chip: ChatChip): ChipCard {
  if (chip.kind === 'image') return chip.uploadId ? 'image' : 'plain'
  if (chip.kind === 'subagent') return 'subagent'
  if (chip.kind === 'source') return chip.href ? 'source' : 'plain'
  if (chip.kind === 'link' && chip.href && chip.changes?.length) return 'document'
  return 'plain'
}

/** One edit, in the document card's one-line voice:
 *  `label · N added · M removed lines`.
 *
 *  THE NUMBERS ARE LINE COUNTS, and the sentence says so, because `+6 −0`
 *  alone reads as characters to half the people who see it — a one-word swap
 *  is `1 added · 1 removed`, not `3 added · 4 removed`. The wording is lifted
 *  verbatim from `agent-refine`'s `summarize`, singular included, so the
 *  refine notice and this card report a document edit in one voice instead of
 *  looking like two different measurements. */
export function summarizeChange(c: DocChange): string {
  const lineWord = c.added === 1 && c.removed === 0 ? 'line' : 'lines'
  return `${c.label} · ${c.added} added · ${c.removed} removed ${lineWord}`
}

/** What a source pill says. A search hit's title is the label; without one the
 *  host is the next most useful thing a reader can recognize, and the raw
 *  href is the last resort.
 *
 *  The host is spelled the way `SourcesList` spells it — `hostname` without a
 *  leading `www.` — so the same page read by the same search reads as
 *  "nytimes.com" in the sources list and in the chat row. */
export function sourceLabel(chip: { label?: string; href?: string }): string {
  const label = chip.label?.trim()
  if (label) return label
  const href = chip.href?.trim()
  if (!href) return ''
  try {
    return new URL(href).hostname.replace(/^www\./, '') || href
  } catch {
    return href
  }
}

/** The second line of a subagent card, or null when there is nothing to add.
 *
 *  The producer's label is already `<role> · <goal>`, so printing `goal`
 *  underneath it said the same sentence twice. The label's copy of the goal is
 *  clipped (80 chars), which is why this compares on a PREFIX rather than on
 *  equality: a goal whose opening 60 chars are already in the label is the
 *  same goal, however the label ended. */
export function subagentGoal(chip: { label?: string; goal?: string }): string | null {
  const goal = chip.goal?.trim()
  if (!goal) return null
  const label = chip.label ?? ''
  return label.includes(goal.slice(0, 60)) ? null : goal
}

/** A delegated child's status as a dot tone. Anything else — including a chip
 *  that landed before the child reported — is `idle`: a dot that claims health
 *  nothing knows yet is worse than a grey one. */
export function subagentTone(status: ChatChip['status']): DotStatus {
  switch (status) {
    case 'running':
      return 'accent'
    case 'done':
      return 'ok'
    case 'error':
      return 'danger'
    default:
      return 'idle'
  }
}

/** How much of a tool row's label the tool list shows. A label is the tool's
 *  preview — an argument dump can run to a paragraph — and the 11px mono row
 *  reads one line; past this the row shows the tool name alone. One place,
 *  so the live row and the row that comes back from history agree by
 *  construction. */
export const TOOL_ROW_LABEL_MAX = 120
export function toolRowLabel(label: string | undefined): string {
  const text = (label ?? '').trim()
  return text.length <= TOOL_ROW_LABEL_MAX ? text : ''
}
