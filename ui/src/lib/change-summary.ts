// What a document edit changed, in one line: "3 added · 1 removed lines".
// The Muse receipt (KbMuseBar) says it after an accept, and the chat's
// document card (tool-cards `summarizeChange`) says the same fact in the same
// words.

import { diffLines } from '@/components/fleet/line-diff'

export interface ChangeSummary {
  adds: number
  dels: number
  /** Line-diff sized the document: the summary is trustworthy. */
  text: string
  /** The document was too large to diff — say "changed" without numbers. */
  oversized: boolean
}

/** What changed, as line counts. Degrades to an honest "changed" when the
 *  document outgrew the client diff. */
export function summarize(before: string, after: string): ChangeSummary {
  const d = diffLines(before, after)
  if (!d) return { adds: 0, dels: 0, text: 'changed', oversized: true }
  const adds = d.filter((l) => l.type === 'add').length
  const dels = d.filter((l) => l.type === 'del').length
  const lineWord = adds === 1 && dels === 0 ? 'line' : 'lines'
  if (adds === 0 && dels === 0) return { adds, dels, text: 'no text changes', oversized: false }
  return { adds, dels, text: `${adds} added · ${dels} removed ${lineWord}`, oversized: false }
}
